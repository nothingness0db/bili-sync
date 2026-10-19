use std::sync::LazyLock;
use std::time::Duration;

use anyhow::Result;
use bili_sync_entity::{video, video_availability, video_stat};
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue::Set;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, QueryFilter, Statement,
};

use crate::bilibili::{BiliClient, BiliError, Credential, Video, VideoInfo};
use crate::utils::model::mark_video_models_unavailable;

// 所有新增的视频采样串行节流，避免多个动态源的手动同步叠加请求速度。
static SAMPLE_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

#[derive(Debug, PartialEq, Eq)]
enum SampleDecision {
    Fetch,
    Deleted,
    Invisible,
}

async fn sample_decision(bvid: &str, now: DateTime<Utc>, db: &DatabaseConnection) -> Result<SampleDecision> {
    // 兼容此前手动检测已经确认删除的记录，动态源也不能重新请求它。
    if video::Entity::find()
        .filter(video::Column::Bvid.eq(bvid))
        .filter(video::Column::DeletedAt.is_not_null())
        .one(db)
        .await?
        .is_some()
    {
        return Ok(SampleDecision::Deleted);
    }
    if let Some(status) = video_availability::Entity::find_by_id(bvid).one(db).await? {
        return Ok(match status.retry_at {
            None => SampleDecision::Deleted,
            Some(retry_at) if retry_at > now.naive_utc() => SampleDecision::Invisible,
            Some(_) => SampleDecision::Fetch,
        });
    }
    Ok(SampleDecision::Fetch)
}

/// 日常采样、详情扫描和手动检查共享不可用状态，按 BV 号跨源生效。
pub async fn record_video_unavailable(bvid: &str, error: &BiliError, db: &impl ConnectionTrait) -> Result<u64> {
    let BiliError::ErrorResponse { code, .. } = error else {
        anyhow::bail!("video availability requires an API error");
    };
    anyhow::ensure!(
        error.is_video_unavailable(),
        "error is not a video availability failure"
    );
    let now = Utc::now();
    let deleted = error.is_video_not_found();
    // 手动复查仍不可见时，不能把此前已确认删除降级成可自动重试。
    if !deleted
        && video_availability::Entity::find_by_id(bvid)
            .one(db)
            .await?
            .is_some_and(|status| status.retry_at.is_none())
    {
        return mark_video_models_unavailable(video::Column::Bvid.eq(bvid), false, db).await;
    }
    video_availability::Entity::insert(video_availability::ActiveModel {
        bvid: Set(bvid.into()),
        error_code: Set(*code),
        checked_at: Set(now.naive_utc()),
        retry_at: Set((!deleted).then(|| (now + chrono::Duration::hours(24)).naive_utc())),
    })
    .on_conflict(
        OnConflict::column(video_availability::Column::Bvid)
            .update_columns([
                video_availability::Column::ErrorCode,
                video_availability::Column::CheckedAt,
                video_availability::Column::RetryAt,
            ])
            .to_owned(),
    )
    .exec(db)
    .await?;
    mark_video_models_unavailable(video::Column::Bvid.eq(bvid), deleted, db).await
}

/// 成功复查后清除暂停状态，并按实际下载权限恢复有效性，保留下载和过滤规则。
pub async fn record_video_available(
    bvid: &str,
    info: &VideoInfo,
    try_upower_anyway: bool,
    db: &impl ConnectionTrait,
) -> Result<()> {
    let cleared = video_availability::Entity::delete_by_id(bvid).exec(db).await?;
    if cleared.rows_affected > 0 {
        video::Entity::update_many()
            .filter(video::Column::Bvid.eq(bvid))
            .col_expr(
                video::Column::Valid,
                Expr::value(info.is_downloadable(try_upower_anyway)),
            )
            .col_expr(
                video::Column::DeletedAt,
                Expr::value(Option::<chrono::NaiveDateTime>::None),
            )
            .exec(db)
            .await?;
        info!("视频 {} 已恢复可见，恢复统计采样，保留本地文件和历史统计", bvid);
    }
    Ok(())
}

pub async fn track_source_videos(upper_id: i64, bvids: &[String], db: &DatabaseConnection) -> Result<()> {
    for batch in bvids.chunks(100) {
        let values = batch
            .iter()
            .flat_map(|b| [upper_id.into(), b.clone().into()])
            .collect::<Vec<sea_orm::Value>>();
        let sql = format!(
            "INSERT INTO video_stat_source (upper_id,bvid) VALUES {} ON CONFLICT DO NOTHING",
            vec!["(?,?)"; batch.len()].join(",")
        );
        db.execute(Statement::from_sql_and_values(DbBackend::Sqlite, sql, values))
            .await?;
    }
    Ok(())
}

pub fn snapshot_from_info(info: &VideoInfo, recorded_at: DateTime<Utc>) -> Option<video_stat::ActiveModel> {
    let VideoInfo::Detail {
        aid,
        stat: Some(stat),
        bvid,
        title,
        cover,
        upper,
        pubtime,
        ..
    } = info
    else {
        return None;
    };
    if [
        stat.view,
        stat.like,
        stat.coin,
        stat.favorite,
        stat.share,
        stat.reply,
        stat.danmaku,
    ]
    .iter()
    .all(Option::is_none)
    {
        return None;
    }
    Some(video_stat::ActiveModel {
        bvid: Set(bvid.clone()),
        aid: Set(*aid),
        upper_id: Set(upper.mid),
        title: Set(title.clone()),
        cover: Set(cover.clone()),
        pubtime: Set(pubtime.naive_utc()),
        view_count: Set(stat.view),
        like_count: Set(stat.like),
        coin_count: Set(stat.coin),
        favorite_count: Set(stat.favorite),
        share_count: Set(stat.share),
        reply_count: Set(stat.reply),
        danmaku_count: Set(stat.danmaku),
        recorded_at: Set(recorded_at.naive_utc()),
        ..Default::default()
    })
}

/// 同一轮中复用已获取的详情；返回 Some(false) 表示确认删除，None 表示无法确认或仍在不可见暂停期。
/// 明确不可用时顺带标记本地视频失效，不写伪造的快照；风控立即向上抛出终止本轮。
pub async fn sample_video(
    bvid: &str,
    since: DateTime<Utc>,
    client: &BiliClient,
    credential: &Credential,
    db: &DatabaseConnection,
    try_upower_anyway: bool,
) -> Result<Option<bool>> {
    let _guard = SAMPLE_LOCK.lock().await;
    match sample_decision(bvid, Utc::now(), db).await? {
        SampleDecision::Deleted => return Ok(Some(false)),
        SampleDecision::Invisible => return Ok(None),
        SampleDecision::Fetch => {}
    }
    if video_stat::Entity::find()
        .filter(video_stat::Column::Bvid.eq(bvid))
        .filter(video_stat::Column::RecordedAt.gte(since.naive_utc()))
        .one(db)
        .await?
        .is_some()
    {
        return Ok(Some(true));
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    save_sample_result(
        bvid,
        Video::new(client, bvid, credential).get_view_info().await,
        db,
        try_upower_anyway,
    )
    .await
}

async fn save_sample_result(
    bvid: &str,
    result: Result<VideoInfo>,
    db: &DatabaseConnection,
    try_upower_anyway: bool,
) -> Result<Option<bool>> {
    match result {
        Ok(info) => {
            record_video_available(bvid, &info, try_upower_anyway, db).await?;
            if let Some(snapshot) = snapshot_from_info(&info, Utc::now()) {
                snapshot.insert(db).await?;
            } else {
                warn!("视频 {} 未返回可用统计，保留历史快照", bvid);
            }
            Ok(Some(true))
        }
        Err(e) if matches!(e.downcast_ref::<BiliError>(), Some(inner) if inner.is_risk_control_related()) => Err(e),
        Err(e) if matches!(e.downcast_ref::<BiliError>(), Some(inner) if inner.is_video_unavailable()) => {
            let error = e.downcast_ref::<BiliError>().unwrap();
            let deleted = error.is_video_not_found();
            let changed = record_video_unavailable(bvid, error, db).await?;
            if changed > 0 {
                info!("视频 {} 当前不可用，标记失效，保留本地文件和历史统计：{:#}", bvid, e);
            } else {
                debug!("视频 {} 仍不可用，保留本地文件和历史统计：{:#}", bvid, e);
            }
            // 不可见不代表已删除，账号视频计数仍需跳过这轮快照。
            Ok(if deleted { Some(false) } else { None })
        }
        Err(e) => {
            warn!("采样视频 {} 统计失败，跳过：{:#}", bvid, e);
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use bili_sync_entity::page;
    use bili_sync_migration::{Migrator, MigratorTrait};
    use sea_orm::{Database, IntoActiveModel, PaginatorTrait};
    use serde_json::json;

    use super::*;

    async fn save_sample_result(
        bvid: &str,
        result: Result<VideoInfo>,
        db: &DatabaseConnection,
    ) -> Result<Option<bool>> {
        super::save_sample_result(bvid, result, db, false).await
    }

    fn detail(stat: serde_json::Value) -> VideoInfo {
        serde_json::from_value(json!({
            "aid": 123, "bvid": "BV1xx411c7mD", "title": "test", "desc": "", "pic": "",
            "owner": {"mid": 42, "name": "test", "face": ""}, "ctime": 1700000000,
            "pubdate": 1700000000, "is_upower_exclusive": false, "is_upower_play": false,
            "pages": [], "state": 0, "stat": stat
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn unchanged_samples_are_retained_and_missing_values_are_not_zero() -> Result<()> {
        let db = Database::connect("sqlite::memory:").await?;
        Migrator::up(&db, None).await?;
        for _ in 0..2 {
            save_sample_result(
                "BV1xx411c7mD",
                Ok(detail(json!({"view": 100, "like": 0, "coin": "--", "favorite": -1}))),
                &db,
            )
            .await?;
        }
        let rows = video_stat::Entity::find().all(&db).await?;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].view_count, Some(100));
        assert_eq!(rows[0].like_count, Some(0));
        assert_eq!(rows[0].coin_count, None);
        assert_eq!(rows[0].favorite_count, None);
        assert_eq!(rows[0].reply_count, None);
        assert_eq!(rows[0].aid, Some(123));
        assert!(rows[0].recorded_at <= rows[1].recorded_at);
        // 同一轮另一视频源再遇到这个 BV 号时，复用快照而不发请求。
        let since = rows[0].recorded_at.and_utc() - chrono::Duration::seconds(1);
        assert_eq!(
            sample_video(
                "BV1xx411c7mD",
                since,
                &BiliClient::new(),
                &Credential::default(),
                &db,
                false
            )
            .await?,
            Some(true)
        );
        assert_eq!(video_stat::Entity::find().count(&db).await?, 2);
        Ok(())
    }

    #[tokio::test]
    async fn failed_and_missing_statistics_preserve_history_and_risk_control_propagates() -> Result<()> {
        let db = Database::connect("sqlite::memory:").await?;
        Migrator::up(&db, None).await?;
        save_sample_result("BV1xx411c7mD", Ok(detail(json!({"view": 42}))), &db).await?;
        assert_eq!(
            save_sample_result("BV1xx411c7mD", Ok(detail(json!({}))), &db).await?,
            Some(true)
        );
        assert_eq!(
            save_sample_result("BV1xx411c7mD", Err(anyhow::anyhow!("temporary failure")), &db).await?,
            None
        );
        let missing = BiliError::ErrorResponse {
            code: -404,
            message: None,
            response: "{}".into(),
        };
        assert_eq!(
            save_sample_result("BV1xx411c7mD", Err(missing.into()), &db).await?,
            Some(false)
        );
        assert!(
            save_sample_result(
                "BV1xx411c7mD",
                Err(BiliError::RiskControlOccurred("blocked".into()).into()),
                &db
            )
            .await
            .is_err()
        );
        assert_eq!(video_stat::Entity::find().count(&db).await?, 1);
        Ok(())
    }

    #[tokio::test]
    async fn sampling_marks_unavailable_videos_without_losing_local_history() -> Result<()> {
        let db = Database::connect("sqlite::memory:").await?;
        Migrator::up(&db, None).await?;
        let bvid = "BV1xx411c7mD";
        // 同一 BV 号在不同源中的本地记录都应被标记，其他视频不受影响。
        for (id, video_bvid) in [(1, bvid), (2, bvid), (3, "BV1other")] {
            video::Model {
                id,
                submission_id: Some(id),
                bvid: video_bvid.into(),
                name: "local title".into(),
                path: "/saved/video".into(),
                valid: true,
                should_download: true,
                download_status: 123,
                ..Default::default()
            }
            .into_active_model()
            .insert(&db)
            .await?;
        }
        let saved_page = page::Model {
            id: 1,
            video_id: 1,
            path: Some("/saved/video.mp4".into()),
            download_status: 456,
            ..Default::default()
        }
        .into_active_model()
        .insert(&db)
        .await?;
        save_sample_result(bvid, Ok(detail(json!({"view": 42}))), &db).await?;
        let error = |code| {
            Err(BiliError::ErrorResponse {
                code,
                message: None,
                response: "{}".into(),
            }
            .into())
        };
        // 临时错误只跳过，风控仍向上传递，不标记任何视频。
        assert_eq!(save_sample_result(bvid, error(-503), &db).await?, None);
        assert!(save_sample_result(bvid, error(-403), &db).await.is_err());
        assert!(video::Entity::find().all(&db).await?.iter().all(|v| v.valid));

        // 不可见顺带进入失效筛选，但不伪造删除时间或账号视频计数。
        for _ in 0..2 {
            assert_eq!(save_sample_result(bvid, error(62002), &db).await?, None);
            let rows = video::Entity::find()
                .filter(video::Column::Bvid.eq(bvid))
                .all(&db)
                .await?;
            assert_eq!(rows.len(), 2);
            assert!(rows.iter().all(|v| !v.valid && v.deleted_at.is_none()));
        }
        let status = video_availability::Entity::find_by_id(bvid).one(&db).await?.unwrap();
        let retry_at = status.retry_at.unwrap().and_utc();
        assert_eq!(retry_at.naive_utc() - status.checked_at, chrono::Duration::hours(24));
        assert_eq!(
            sample_decision(bvid, retry_at - chrono::Duration::seconds(1), &db).await?,
            SampleDecision::Invisible
        );
        assert_eq!(sample_decision(bvid, retry_at, &db).await?, SampleDecision::Fetch);
        // 暂停状态优先于旧快照，不发网络请求，跨视频源也生效。
        assert_eq!(
            sample_video(bvid, Utc::now(), &BiliClient::new(), &Credential::default(), &db, false).await?,
            None
        );
        // 后续确认已删除时，即使此前失效也补记时间，重复检测保留最初时间。
        assert_eq!(save_sample_result(bvid, error(-404), &db).await?, Some(false));
        let deleted_at = video::Entity::find_by_id(1).one(&db).await?.unwrap().deleted_at;
        assert!(deleted_at.is_some());
        assert_eq!(save_sample_result(bvid, error(62012), &db).await?, Some(false));
        assert_eq!(save_sample_result(bvid, error(62002), &db).await?, None);
        assert_eq!(
            sample_decision(bvid, Utc::now() + chrono::Duration::days(365), &db).await?,
            SampleDecision::Deleted
        );
        assert_eq!(
            sample_video(bvid, Utc::now(), &BiliClient::new(), &Credential::default(), &db, false).await?,
            Some(false)
        );
        for row in video::Entity::find()
            .filter(video::Column::Bvid.eq(bvid))
            .all(&db)
            .await?
        {
            assert!(!row.valid);
            assert_eq!(row.deleted_at, deleted_at);
            assert_eq!(row.name, "local title");
            assert_eq!(row.path, "/saved/video");
            assert_eq!(row.download_status, 123);
            assert!(row.should_download);
        }
        assert!(video::Entity::find_by_id(3).one(&db).await?.unwrap().valid);
        assert_eq!(page::Entity::find_by_id(1).one(&db).await?, Some(saved_page));
        assert_eq!(video_stat::Entity::find().count(&db).await?, 1);
        // 手动检查成功可解除确认删除的暂停状态，不需要重新创建本地记录。
        save_sample_result(bvid, Ok(detail(json!({"view": 43}))), &db).await?;
        assert_eq!(sample_decision(bvid, Utc::now(), &db).await?, SampleDecision::Fetch);
        for row in video::Entity::find()
            .filter(video::Column::Bvid.eq(bvid))
            .all(&db)
            .await?
        {
            assert!(row.valid);
            assert!(row.deleted_at.is_none());
            assert_eq!(row.path, "/saved/video");
            assert_eq!(row.download_status, 123);
        }
        assert_eq!(video_stat::Entity::find().count(&db).await?, 2);
        Ok(())
    }

    #[tokio::test]
    async fn availability_covers_dynamic_only_videos_and_preserves_download_permissions() -> Result<()> {
        let db = Database::connect("sqlite::memory:").await?;
        Migrator::up(&db, None).await?;
        let missing = BiliError::ErrorResponse {
            code: 62012,
            message: None,
            response: "{}".into(),
        };
        // 动态源里没有下载记录的视频，也应在确认删除后停止采样。
        record_video_unavailable("BV1dynamic", &missing, &db).await?;
        assert_eq!(
            sample_decision("BV1dynamic", Utc::now(), &db).await?,
            SampleDecision::Deleted
        );

        let bvid = "BV1xx411c7mD";
        video::Model {
            id: 1,
            bvid: bvid.into(),
            valid: false,
            ..Default::default()
        }
        .into_active_model()
        .insert(&db)
        .await?;
        // 没有明确不可用返回码的失效记录（例如充电权限不足）仍可采样。
        assert_eq!(sample_decision(bvid, Utc::now(), &db).await?, SampleDecision::Fetch);
        let invisible = BiliError::ErrorResponse {
            code: 62002,
            message: None,
            response: "{}".into(),
        };
        record_video_unavailable(bvid, &invisible, &db).await?;
        let mut recovered = detail(json!({"view": 42}));
        if let VideoInfo::Detail {
            is_upower_exclusive, ..
        } = &mut recovered
        {
            *is_upower_exclusive = true;
        }
        save_sample_result(bvid, Ok(recovered), &db).await?;
        assert_eq!(sample_decision(bvid, Utc::now(), &db).await?, SampleDecision::Fetch);
        assert!(!video::Entity::find_by_id(1).one(&db).await?.unwrap().valid);
        assert_eq!(video_stat::Entity::find().count(&db).await?, 1);

        // 兼容升级前已经记录 deleted_at 的视频，动态采样也直接跳过。
        video::Entity::update_many()
            .filter(video::Column::Bvid.eq(bvid))
            .col_expr(video::Column::DeletedAt, Expr::value(Utc::now().naive_utc()))
            .exec(&db)
            .await?;
        assert_eq!(sample_decision(bvid, Utc::now(), &db).await?, SampleDecision::Deleted);
        Ok(())
    }

    #[tokio::test]
    async fn availability_survives_restart_and_migration_retry_without_losing_statistics() -> Result<()> {
        let path = std::env::temp_dir().join(format!("bili-sync-availability-{}.sqlite", uuid::Uuid::new_v4()));
        let url = format!("sqlite:{}?mode=rwc", path.display());
        let db = Database::connect(&url).await?;
        Migrator::up(&db, None).await?;
        let invisible = BiliError::ErrorResponse {
            code: 62002,
            message: None,
            response: "{}".into(),
        };
        record_video_unavailable("BV1invisible", &invisible, &db).await?;
        let deleted = BiliError::ErrorResponse {
            code: 62012,
            message: None,
            response: "{}".into(),
        };
        record_video_unavailable("BV1deleted", &deleted, &db).await?;
        save_sample_result("BV1xx411c7mD", Ok(detail(json!({"view": 42}))), &db).await?;
        db.close().await?;

        let db = Database::connect(&url).await?;
        assert_eq!(
            sample_decision("BV1invisible", Utc::now(), &db).await?,
            SampleDecision::Invisible
        );
        assert_eq!(
            sample_decision("BV1deleted", Utc::now(), &db).await?,
            SampleDecision::Deleted
        );
        // 模拟迁移建表后中断，重试必须保留暂停状态和统计历史。
        db.execute_unprepared("DELETE FROM seaql_migrations WHERE version = 'm20261012_000001_video_availability'")
            .await?;
        Migrator::up(&db, None).await?;
        assert_eq!(video_availability::Entity::find().count(&db).await?, 2);
        assert_eq!(video_stat::Entity::find().count(&db).await?, 1);
        // 新迁移回滚不影响已有统计表，重新应用后仍可正常使用。
        let steps = Migrator::migrations()
            .iter()
            .rev()
            .take_while(|m| m.name() != "m20261012_000001_video_availability")
            .count() as u32
            + 1;
        Migrator::down(&db, Some(steps)).await?;
        assert_eq!(video_stat::Entity::find().count(&db).await?, 1);
        Migrator::up(&db, Some(steps)).await?;
        assert_eq!(video_availability::Entity::find().count(&db).await?, 0);
        assert_eq!(video_stat::Entity::find().count(&db).await?, 1);
        db.close().await?;
        tokio::fs::remove_file(path).await?;
        Ok(())
    }
}
