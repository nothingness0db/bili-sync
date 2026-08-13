use axum::routing::get;
use axum::{Extension, Router};
use bili_sync_entity::*;
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::Condition;
use sea_orm::{ColumnTrait, FromQueryResult, QueryFilter, Statement};

use crate::api::response::{
    DashBoardResponse, DayCountPair, TaskBoardDynamicSource, TaskBoardResponse, TaskBoardScanTask, TaskBoardVideoTask,
};
use crate::api::routes::videos::read_scan_task_progress;
use crate::api::wrapper::{ApiError, ApiResponse};
use crate::task::read_video_task_progress;
use crate::utils::status::STATUS_COMPLETED;
use crate::workflow_dynamic::read_sync_progress;

pub(super) fn router() -> Router {
    Router::new()
        .route("/dashboard", get(get_dashboard))
        .route("/task-board", get(get_task_board))
}

/// 任务看板：视频任务 / 各动态源 / 删除检测的实时进度与估算
async fn get_task_board(
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<TaskBoardResponse>, ApiError> {
    let video_progress = read_video_task_progress();
    let sync_progress = read_sync_progress();
    let scan = read_scan_task_progress();
    let sources = dynamic_source::Entity::find()
        .filter(dynamic_source::Column::Enabled.eq(true))
        .all(&db)
        .await?;
    let mut dynamic_sources = Vec::with_capacity(sources.len());
    for source in sources {
        // 该源尚未消化完的动态（新动态 + 待重扫评论）
        let pending = dynamic::Entity::find()
            .filter(dynamic::Column::SourceId.eq(source.id))
            .filter(dynamic::Column::Valid.eq(true))
            .filter(
                Condition::any()
                    .add(dynamic::Column::DownloadStatus.lt(STATUS_COMPLETED))
                    .add(dynamic::Column::RescanReply.eq(true)),
            )
            .count(&db)
            .await? as usize;
        let active = sync_progress.source_name == source.upper_name;
        dynamic_sources.push(TaskBoardDynamicSource {
            id: source.id,
            name: source.upper_name,
            active,
            phase: if active {
                sync_progress.phase.clone()
            } else {
                String::new()
            },
            current: if active { sync_progress.current } else { 0 },
            total: if active { sync_progress.total } else { 0 },
            eta_seconds: if active { sync_progress.eta_seconds } else { None },
            pending,
        });
    }
    Ok(ApiResponse::ok(TaskBoardResponse {
        video_task: TaskBoardVideoTask {
            running: !video_progress.phase.is_empty(),
            phase: video_progress.phase,
            current_target: video_progress.current_target,
            current_source_index: video_progress.current_source_index,
            total_sources: video_progress.total_sources,
        },
        dynamic_sources,
        scan_task: TaskBoardScanTask {
            state: scan.state,
            current: scan.current,
            total: scan.total,
        },
    }))
}

async fn get_dashboard(
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<DashBoardResponse>, ApiError> {
    let (enabled_favorites, enabled_collections, enabled_submissions, enabled_watch_later, videos_by_day) = tokio::try_join!(
        favorite::Entity::find()
            .filter(favorite::Column::Enabled.eq(true))
            .count(&db),
        collection::Entity::find()
            .filter(collection::Column::Enabled.eq(true))
            .count(&db),
        submission::Entity::find()
            .filter(submission::Column::Enabled.eq(true))
            .count(&db),
        watch_later::Entity::find()
            .filter(watch_later::Column::Enabled.eq(true))
            .count(&db),
        DayCountPair::find_by_statement(Statement::from_string(
            db.get_database_backend(),
            // 用 SeaORM 太复杂了，直接写个裸 SQL
            "
SELECT
    dates.day AS day,
    COUNT(video.id) AS cnt
FROM
    (
        SELECT
            STRFTIME('%Y-%m-%d', DATE('now', '-' || n || ' days', 'localtime')) AS day,
            DATETIME(DATE('now', '-' || n || ' days', 'localtime'), 'utc') AS start_utc_datetime,
            DATETIME(DATE('now', '-' || n || ' days', '+1 day', 'localtime'), 'utc') AS end_utc_datetime
        FROM
            (
                SELECT 0 AS n UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6
            )
    ) AS dates
LEFT JOIN
    video ON video.created_at >= dates.start_utc_datetime AND video.created_at < dates.end_utc_datetime
GROUP BY
    dates.day
ORDER BY
    dates.day;
    "
        ))
        .all(&db),
    )?;
    Ok(ApiResponse::ok(DashBoardResponse {
        enabled_favorites,
        enabled_collections,
        enabled_submissions,
        enable_watch_later: enabled_watch_later > 0,
        videos_by_day,
    }))
}
