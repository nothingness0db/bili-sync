use std::sync::LazyLock;
use std::time::Duration;

use anyhow::Result;
use bili_sync_entity::video_stat;
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait,
    QueryFilter, Statement,
};

use crate::bilibili::{BiliClient, BiliError, Credential, Video, VideoInfo};

// 所有新增的视频采样串行节流，避免多个动态源的手动同步叠加请求速度。
static SAMPLE_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

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

/// 同一轮中复用已获取的详情；返回 Some(false) 表示确实不存在，None 表示请求失败。
/// 非风控错误只跳过该视频，不写伪造的快照；风控立即向上抛出终止本轮。
pub async fn sample_video(
    bvid: &str,
    since: DateTime<Utc>,
    client: &BiliClient,
    credential: &Credential,
    db: &DatabaseConnection,
) -> Result<Option<bool>> {
    let _guard = SAMPLE_LOCK.lock().await;
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
    save_sample_result(bvid, Video::new(client, bvid, credential).get_view_info().await, db).await
}

async fn save_sample_result(bvid: &str, result: Result<VideoInfo>, db: &DatabaseConnection) -> Result<Option<bool>> {
    match result {
        Ok(info) => {
            if let Some(snapshot) = snapshot_from_info(&info, Utc::now()) {
                snapshot.insert(db).await?;
            } else {
                warn!("视频 {} 未返回可用统计，保留历史快照", bvid);
            }
            Ok(Some(true))
        }
        Err(e) if matches!(e.downcast_ref::<BiliError>(), Some(inner) if inner.is_risk_control_related()) => Err(e),
        Err(e) if matches!(e.downcast_ref::<BiliError>(), Some(inner) if inner.is_video_not_found()) => {
            info!("视频 {} 已不存在，保留历史统计", bvid);
            Ok(Some(false))
        }
        Err(e) => {
            warn!("采样视频 {} 统计失败，跳过：{:#}", bvid, e);
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bili_sync_migration::{Migrator, MigratorTrait};
    use sea_orm::{Database, PaginatorTrait};
    use serde_json::json;

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
            sample_video("BV1xx411c7mD", since, &BiliClient::new(), &Credential::default(), &db).await?,
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
}
