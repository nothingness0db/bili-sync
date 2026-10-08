use std::collections::HashMap;

use axum::extract::{Path, Query};
use axum::routing::get;
use axum::{Extension, Router};
use bili_sync_entity::{dynamic_source, video, video_stat};
use chrono::{DateTime, NaiveDateTime, Utc};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, QueryFilter, QueryOrder,
    Statement,
};
use serde::{Deserialize, Serialize};

use crate::api::error::InnerApiError;
use crate::api::wrapper::{ApiError, ApiResponse};

pub(super) fn router() -> Router {
    Router::new()
        .route("/video-stats/{bvid}", get(get_video_stats))
        .route("/dynamic-sources/{id}/videos", get(get_source_videos))
}

#[derive(Default, Deserialize)]
pub struct StatsQuery {
    pub days: Option<u32>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

impl StatsQuery {
    pub(super) fn cutoff(&self) -> Result<Option<NaiveDateTime>, ApiError> {
        match self.days {
            None => Ok(None),
            Some(days @ 1..=3660) => Ok(Some((Utc::now() - chrono::Duration::days(i64::from(days))).naive_utc())),
            Some(_) => Err(InnerApiError::BadRequest("days 必须在 1 到 3660 之间".into()).into()),
        }
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMetrics {
    pub view_count: Option<i64>,
    pub like_count: Option<i64>,
    pub coin_count: Option<i64>,
    pub favorite_count: Option<i64>,
    pub share_count: Option<i64>,
    pub reply_count: Option<i64>,
    pub danmaku_count: Option<i64>,
}

impl From<&video_stat::Model> for VideoMetrics {
    fn from(s: &video_stat::Model) -> Self {
        Self {
            view_count: s.view_count,
            like_count: s.like_count,
            coin_count: s.coin_count,
            favorite_count: s.favorite_count,
            share_count: s.share_count,
            reply_count: s.reply_count,
            danmaku_count: s.danmaku_count,
        }
    }
}

impl VideoMetrics {
    fn delta(&self, first: &Self, samples: i64) -> Self {
        if samples < 2 {
            return Self::default();
        }
        let difference = |last: Option<i64>, first: Option<i64>| last.zip(first).map(|(l, f)| l - f);
        Self {
            view_count: difference(self.view_count, first.view_count),
            like_count: difference(self.like_count, first.like_count),
            coin_count: difference(self.coin_count, first.coin_count),
            favorite_count: difference(self.favorite_count, first.favorite_count),
            share_count: difference(self.share_count, first.share_count),
            reply_count: difference(self.reply_count, first.reply_count),
            danmaku_count: difference(self.danmaku_count, first.danmaku_count),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoStatPoint {
    pub recorded_at: DateTime<Utc>,
    #[serde(flatten)]
    pub metrics: VideoMetrics,
}

impl From<&video_stat::Model> for VideoStatPoint {
    fn from(s: &video_stat::Model) -> Self {
        Self {
            recorded_at: s.recorded_at.and_utc(),
            metrics: s.into(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoStatListItem {
    pub bvid: String,
    pub title: String,
    pub cover: String,
    pub pubtime: DateTime<Utc>,
    pub latest: VideoStatPoint,
    pub growth: VideoMetrics,
    pub baseline_at: Option<DateTime<Utc>>,
    pub sample_count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoStatListResponse {
    pub videos: Vec<VideoStatListItem>,
    pub total_count: u64,
}

#[derive(FromQueryResult, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedDynamic {
    pub id: String,
    pub source_id: i32,
    pub upper_name: String,
    pub reply_count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoStatsResponse {
    pub bvid: String,
    pub title: String,
    pub cover: String,
    pub latest: Option<VideoStatPoint>,
    pub stats: Vec<VideoStatPoint>,
    pub growth: VideoMetrics,
    pub baseline_at: Option<DateTime<Utc>>,
    pub sample_count: i64,
    pub related_dynamics: Vec<RelatedDynamic>,
}

#[derive(FromQueryResult)]
struct Baseline {
    bvid: String,
    recorded_at: NaiveDateTime,
    view_count: Option<i64>,
    like_count: Option<i64>,
    coin_count: Option<i64>,
    favorite_count: Option<i64>,
    share_count: Option<i64>,
    reply_count: Option<i64>,
    danmaku_count: Option<i64>,
    sample_count: i64,
}

impl Baseline {
    fn metrics(&self) -> VideoMetrics {
        VideoMetrics {
            view_count: self.view_count,
            like_count: self.like_count,
            coin_count: self.coin_count,
            favorite_count: self.favorite_count,
            share_count: self.share_count,
            reply_count: self.reply_count,
            danmaku_count: self.danmaku_count,
        }
    }
}

async fn baselines(
    db: &DatabaseConnection,
    bvids: &[String],
    cutoff: Option<NaiveDateTime>,
) -> Result<HashMap<String, Baseline>, ApiError> {
    if bvids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; bvids.len()].join(",");
    let sql = format!(
        "WITH ranked AS (
        SELECT *, ROW_NUMBER() OVER (PARTITION BY bvid ORDER BY recorded_at, id) AS rn,
            COUNT(*) OVER (PARTITION BY bvid) AS sample_count FROM video_stat
        WHERE bvid IN ({placeholders}) AND (? IS NULL OR recorded_at >= ?))
        SELECT * FROM ranked WHERE rn = 1"
    );
    let mut values = bvids.iter().cloned().map(Into::into).collect::<Vec<sea_orm::Value>>();
    values.extend([cutoff.into(), cutoff.into()]);
    Ok(
        Baseline::find_by_statement(Statement::from_sql_and_values(DbBackend::Sqlite, sql, values))
            .all(db)
            .await?
            .into_iter()
            .map(|s| (s.bvid.clone(), s))
            .collect(),
    )
}

pub async fn get_source_videos(
    Path(id): Path<i32>,
    Query(params): Query<StatsQuery>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<VideoStatListResponse>, ApiError> {
    let cutoff = params.cutoff()?;
    let source = dynamic_source::Entity::find_by_id(id)
        .one(&db)
        .await?
        .ok_or(InnerApiError::NotFound(id))?;
    let page_size = params.page_size.unwrap_or(20).clamp(1, 100);
    let offset = params.page.unwrap_or(0).saturating_mul(page_size).min(i64::MAX as u64);
    let total_count: i64 = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT COUNT(DISTINCT bvid) AS count FROM video_stat WHERE upper_id = ?
            OR bvid IN (SELECT bvid FROM video_stat_source WHERE upper_id = ?)",
            [source.upper_id.into(), source.upper_id.into()],
        ))
        .await?
        .expect("COUNT always returns a row")
        .try_get("", "count")?;
    let latest = video_stat::Entity::find()
        .from_raw_sql(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "WITH ranked AS (SELECT *, ROW_NUMBER() OVER (PARTITION BY bvid ORDER BY recorded_at DESC, id DESC) AS rn
            FROM video_stat WHERE upper_id = ? OR bvid IN (SELECT bvid FROM video_stat_source WHERE upper_id = ?))
         SELECT * FROM ranked WHERE rn = 1 ORDER BY pubtime DESC, bvid LIMIT ? OFFSET ?",
            [
                source.upper_id.into(),
                source.upper_id.into(),
                (page_size as i64).into(),
                (offset as i64).into(),
            ],
        ))
        .all(&db)
        .await?;
    let mut first = baselines(&db, &latest.iter().map(|v| v.bvid.clone()).collect::<Vec<_>>(), cutoff).await?;
    let videos = latest
        .into_iter()
        .map(|v| {
            let baseline = first.remove(&v.bvid);
            VideoStatListItem {
                latest: (&v).into(),
                growth: baseline
                    .as_ref()
                    .map(|b| VideoMetrics::from(&v).delta(&b.metrics(), b.sample_count))
                    .unwrap_or_default(),
                baseline_at: baseline.as_ref().map(|b| b.recorded_at.and_utc()),
                sample_count: baseline.map(|b| b.sample_count).unwrap_or(0),
                bvid: v.bvid,
                title: v.title,
                cover: v.cover,
                pubtime: v.pubtime.and_utc(),
            }
        })
        .collect();
    Ok(ApiResponse::ok(VideoStatListResponse {
        videos,
        total_count: total_count as u64,
    }))
}

pub async fn get_video_stats(
    Path(bvid): Path<String>,
    Query(params): Query<StatsQuery>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<VideoStatsResponse>, ApiError> {
    if bvid.len() != 12 || !bvid.starts_with("BV") || !bvid.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return Err(InnerApiError::BadRequest("无效的 BV 号".into()).into());
    }
    let cutoff = params.cutoff()?;
    let latest = video_stat::Entity::find()
        .filter(video_stat::Column::Bvid.eq(&bvid))
        .order_by_desc(video_stat::Column::RecordedAt)
        .order_by_desc(video_stat::Column::Id)
        .one(&db)
        .await?;
    let first = baselines(&db, std::slice::from_ref(&bvid), cutoff).await?.remove(&bvid);
    let sample_count = first.as_ref().map(|b| b.sample_count).unwrap_or(0);
    // 图表等间距抽样至约 1000 点，保留首末点；数据库中的原始快照全部保留。
    let stride = ((sample_count - 1).max(0) + 998) / 999;
    let stats = video_stat::Entity::find()
        .from_raw_sql(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "WITH ordered AS (SELECT *, ROW_NUMBER() OVER (ORDER BY recorded_at, id) AS rn,
            COUNT(*) OVER () AS n FROM video_stat WHERE bvid = ? AND (? IS NULL OR recorded_at >= ?))
         SELECT * FROM ordered WHERE rn = 1 OR rn = n OR (rn - 1) % ? = 0 ORDER BY recorded_at, id",
            [bvid.clone().into(), cutoff.into(), cutoff.into(), stride.max(1).into()],
        ))
        .all(&db)
        .await?;
    let related_dynamics = RelatedDynamic::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT d.id, d.source_id, s.upper_name, COUNT(r.rpid) AS reply_count FROM dynamic d
         JOIN dynamic_source s ON s.id = d.source_id LEFT JOIN reply r ON r.dynamic_id = d.id AND r.valid = 1
         WHERE d.video_bvid = ? GROUP BY d.id ORDER BY d.pub_ts DESC",
        [bvid.clone().into()],
    ))
    .all(&db)
    .await?;
    let local = if latest.is_none() {
        video::Entity::find()
            .filter(video::Column::Bvid.eq(&bvid))
            .one(&db)
            .await?
    } else {
        None
    };
    Ok(ApiResponse::ok(VideoStatsResponse {
        title: latest
            .as_ref()
            .map(|v| v.title.clone())
            .or_else(|| local.as_ref().map(|v| v.name.clone()))
            .unwrap_or_else(|| bvid.clone()),
        cover: latest
            .as_ref()
            .map(|v| v.cover.clone())
            .or_else(|| local.map(|v| v.cover))
            .unwrap_or_default(),
        growth: latest
            .as_ref()
            .zip(first.as_ref())
            .map(|(l, b)| VideoMetrics::from(l).delta(&b.metrics(), b.sample_count))
            .unwrap_or_default(),
        baseline_at: first.map(|b| b.recorded_at.and_utc()),
        latest: latest.as_ref().map(Into::into),
        stats: stats.iter().map(Into::into).collect(),
        bvid,
        sample_count,
        related_dynamics,
    }))
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, ensure};
    use axum::body::to_bytes;
    use axum::response::IntoResponse;
    use bili_sync_entity::{dynamic, reply};
    use bili_sync_migration::{Migrator, MigratorTrait};
    use sea_orm::ActiveValue::Set;
    use sea_orm::{ActiveModelTrait, ConnectOptions, Database, IntoActiveModel, PaginatorTrait};
    use serde_json::{Value, json};

    use super::*;

    const BVID: &str = "BV1xx411c7mD";

    async fn database() -> Result<DatabaseConnection> {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let db = Database::connect(options).await?;
        Migrator::up(&db, None).await?;
        dynamic_source::ActiveModel {
            id: Set(1),
            upper_id: Set(42),
            upper_name: Set("test".into()),
            path: Set("".into()),
            created_at: Set(Utc::now().naive_utc()),
            latest_dyn_at: Set(Utc::now().naive_utc()),
            sync_reply: Set(true),
            enabled: Set(false),
        }
        .insert(&db)
        .await?;
        Ok(db)
    }

    fn snapshot(bvid: &str, recorded_at: DateTime<Utc>, views: i64, likes: Option<i64>) -> video_stat::ActiveModel {
        video_stat::ActiveModel {
            bvid: Set(bvid.into()),
            aid: Set(Some(123)),
            upper_id: Set(42),
            title: Set("sample".into()),
            cover: Set("".into()),
            pubtime: Set((Utc::now() - chrono::Duration::days(60)).naive_utc()),
            view_count: Set(Some(views)),
            like_count: Set(likes),
            recorded_at: Set(recorded_at.naive_utc()),
            ..Default::default()
        }
    }

    async fn data<T: Serialize>(result: Result<ApiResponse<T>, ApiError>) -> Result<Value> {
        let response = match result {
            Ok(r) => r.into_response(),
            Err(e) => e.into_response(),
        };
        let status = response.status();
        let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await?)?;
        ensure!(status.is_success(), "API failed: {body}");
        Ok(body["data"].clone())
    }

    #[tokio::test]
    async fn source_list_deduplicates_videos_and_growth_uses_actual_window_with_signed_deltas() -> Result<()> {
        let db = database().await?;
        let now = Utc::now();
        snapshot(BVID, now - chrono::Duration::days(30), 1, Some(1))
            .insert(&db)
            .await?;
        snapshot(BVID, now - chrono::Duration::days(2), 100, Some(10))
            .insert(&db)
            .await?;
        snapshot(BVID, now, 130, Some(8)).insert(&db).await?;
        // 较早的采样晚落库，列表仍应选时间最新的记录，而不是最大的 id。
        snapshot(BVID, now - chrono::Duration::days(1), 120, None)
            .insert(&db)
            .await?;
        snapshot("BV1yy411c7mD", now, 10, Some(0)).insert(&db).await?;
        let result = data(
            get_source_videos(
                Path(1),
                Query(StatsQuery {
                    days: Some(7),
                    ..Default::default()
                }),
                Extension(db.clone()),
            )
            .await,
        )
        .await?;
        assert_eq!(result["totalCount"], 2);
        let video = result["videos"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["bvid"] == BVID)
            .unwrap();
        assert_eq!(video["latest"]["viewCount"], 130);
        assert_eq!(video["growth"]["viewCount"], 30);
        assert_eq!(video["growth"]["likeCount"], -2);
        assert_eq!(video["sampleCount"], 3);
        assert!(video["latest"]["recordedAt"].as_str().unwrap().ends_with('Z'));
        let single = result["videos"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["bvid"] != BVID)
            .unwrap();
        assert!(single["growth"]["viewCount"].is_null());
        let empty_page = data(
            get_source_videos(
                Path(1),
                Query(StatsQuery {
                    page: Some(100),
                    ..Default::default()
                }),
                Extension(db),
            )
            .await,
        )
        .await?;
        assert_eq!(empty_page["totalCount"], 2);
        assert_eq!(empty_page["videos"].as_array().unwrap().len(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn coauthored_videos_share_snapshots_without_disappearing_from_source_lists() -> Result<()> {
        let db = database().await?;
        let mut row = snapshot(BVID, Utc::now(), 123, Some(4));
        row.upper_id = Set(99);
        row.insert(&db).await?;
        let bvids = vec![BVID.to_owned(), BVID.to_owned()];
        for upper_id in [42, 43, 42] {
            crate::video_stats::track_source_videos(upper_id, &bvids, &db).await?;
        }
        let membership_count: i64 = db
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM video_stat_source",
            ))
            .await?
            .unwrap()
            .try_get("", "count")?;
        assert_eq!(membership_count, 2);
        let result =
            data(get_source_videos(Path(1), Query(StatsQuery::default()), Extension(db.clone())).await).await?;
        assert_eq!(result["totalCount"], 1);
        assert_eq!(result["videos"][0]["bvid"], BVID);
        assert_eq!(result["videos"][0]["latest"]["viewCount"], 123);
        assert_eq!(result["videos"][0]["sampleCount"], 1);
        assert!(result["videos"][0]["growth"]["viewCount"].is_null());
        assert_eq!(video_stat::Entity::find().count(&db).await?, 1);
        Ok(())
    }

    #[tokio::test]
    async fn missing_baselines_do_not_fabricate_growth_and_comments_join_by_bvid() -> Result<()> {
        let db = database().await?;
        let now = Utc::now();
        snapshot(BVID, now - chrono::Duration::days(2), 100, None)
            .insert(&db)
            .await?;
        snapshot(BVID, now, 150, Some(5)).insert(&db).await?;
        dynamic::Model {
            id: "dyn".into(),
            source_id: 1,
            video_bvid: Some(BVID.into()),
            valid: true,
            ..Default::default()
        }
        .into_active_model()
        .reset_all()
        .insert(&db)
        .await?;
        for (rpid, valid) in [(1, true), (2, false)] {
            reply::Model {
                rpid,
                dynamic_id: "dyn".into(),
                valid,
                ..Default::default()
            }
            .into_active_model()
            .reset_all()
            .insert(&db)
            .await?;
        }
        let result =
            data(get_video_stats(Path(BVID.into()), Query(StatsQuery::default()), Extension(db.clone())).await).await?;
        assert_eq!(result["growth"]["viewCount"], 50);
        assert!(result["growth"]["likeCount"].is_null());
        assert_eq!(result["relatedDynamics"][0]["id"], "dyn");
        assert_eq!(result["relatedDynamics"][0]["replyCount"], 1);
        let empty =
            data(get_video_stats(Path("BV1zz411c7mD".into()), Query(StatsQuery::default()), Extension(db)).await)
                .await?;
        assert!(empty["latest"].is_null());
        assert!(empty["growth"]["viewCount"].is_null());
        Ok(())
    }

    #[tokio::test]
    async fn chart_sampling_preserves_first_last_and_full_history_growth() -> Result<()> {
        let db = database().await?;
        let now = Utc::now();
        for start in (0..2401).step_by(200) {
            video_stat::Entity::insert_many(
                (start..(start + 200).min(2401))
                    .map(|i| snapshot(BVID, now - chrono::Duration::minutes(2401 - i), i, None)),
            )
            .exec(&db)
            .await?;
        }
        let result =
            data(get_video_stats(Path(BVID.into()), Query(StatsQuery::default()), Extension(db.clone())).await).await?;
        let points = result["stats"].as_array().unwrap();
        assert!(points.len() <= 1001);
        assert_eq!(points.first().unwrap()["viewCount"], 0);
        assert_eq!(points.last().unwrap()["viewCount"], 2400);
        assert_eq!(result["growth"]["viewCount"], 2400);
        assert_eq!(result["sampleCount"], 2401);
        assert_eq!(video_stat::Entity::find().count(&db).await?, 2401);
        Ok(())
    }

    #[tokio::test]
    async fn migration_retry_backfills_video_links_without_deleting_comments_or_statistics() -> Result<()> {
        let db = database().await?;
        let raw = json!({"modules":{"module_dynamic":{"major":{"archive":{"bvid":BVID}}}}});
        dynamic::Model {
            id: "dyn".into(),
            source_id: 1,
            dyn_type: "DYNAMIC_TYPE_AV".into(),
            raw: Some(raw.to_string()),
            ..Default::default()
        }
        .into_active_model()
        .reset_all()
        .insert(&db)
        .await?;
        dynamic::Model {
            id: "bad-json".into(),
            source_id: 1,
            dyn_type: "DYNAMIC_TYPE_AV".into(),
            raw: Some("malformed".into()),
            ..Default::default()
        }
        .into_active_model()
        .reset_all()
        .insert(&db)
        .await?;
        reply::Model {
            rpid: 1,
            dynamic_id: "dyn".into(),
            valid: true,
            ..Default::default()
        }
        .into_active_model()
        .reset_all()
        .insert(&db)
        .await?;
        snapshot(BVID, Utc::now(), 10, Some(1)).insert(&db).await?;
        db.execute_unprepared("DROP INDEX idx_dynamic_video_bvid").await?;
        db.execute_unprepared("ALTER TABLE dynamic DROP COLUMN video_bvid")
            .await?;
        db.execute_unprepared("DELETE FROM seaql_migrations WHERE version = 'm20261008_000002_video_stat'")
            .await?;
        Migrator::up(&db, None).await?;
        assert_eq!(
            dynamic::Entity::find_by_id("dyn")
                .one(&db)
                .await?
                .unwrap()
                .video_bvid
                .as_deref(),
            Some(BVID)
        );
        assert!(
            dynamic::Entity::find_by_id("bad-json")
                .one(&db)
                .await?
                .unwrap()
                .video_bvid
                .is_none()
        );
        assert_eq!(video_stat::Entity::find().count(&db).await?, 1);
        assert_eq!(reply::Entity::find().count(&db).await?, 1);
        let steps = Migrator::migrations()
            .iter()
            .rev()
            .take_while(|m| m.name() != "m20261008_000002_video_stat")
            .count() as u32
            + 1;
        Migrator::down(&db, Some(steps)).await?;
        Migrator::up(&db, Some(steps)).await?;
        assert_eq!(reply::Entity::find().count(&db).await?, 1);
        Ok(())
    }
}
