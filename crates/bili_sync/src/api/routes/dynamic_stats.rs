use anyhow::Result;
use axum::extract::{Extension, Path};
use axum::routing::{get, post};
use axum::Router;
use bili_sync_entity::*;
use sea_orm::ActiveValue::Set;
use sea_orm::entity::prelude::*;
use sea_orm::QueryOrder;
use sea_orm::{DatabaseConnection, QueryFilter};
use std::sync::Arc;

use crate::api::error::InnerApiError;
use crate::api::response::{DynamicListItem, DynamicStatsResponse, StatPoint, UpperVersion};
use crate::api::wrapper::{ApiError, ApiResponse};
use crate::bilibili::BiliClient;
use crate::config::VersionedConfig;
use crate::workflow_dynamic::update_upper_stat;

pub(super) fn router() -> Router {
    Router::new()
        .route("/dynamic-sources/{id}/stats", get(get_dynamic_source_stats))
        .route("/dynamic-sources/{id}/scan-profile", post(scan_profile))
        .route("/dynamic-sources/{id}/dynamics", get(get_dynamic_source_dynamics))
        .route("/dynamic-sources/{id}/rescan-replies", post(rescan_all_replies))
        .route(
            "/dynamic-sources/{id}/dynamics/{dyn_id}/rescan-reply",
            post(rescan_single_reply),
        )
}

/// 立即扫描一次该动态源的账号信息（名字/签名/头像/粉丝/关注/投稿/播放）
pub async fn scan_profile(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
    Extension(bili_client): Extension<Arc<BiliClient>>,
) -> Result<ApiResponse<bool>, ApiError> {
    let Some(source) = dynamic_source::Entity::find_by_id(id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let config = VersionedConfig::get().snapshot();
    update_upper_stat(&source, &bili_client, &db, &config).await?;
    Ok(ApiResponse::ok(true))
}

/// 获取动态源的账号数据：数值折线图数据 + 名字/签名版本历史
pub async fn get_dynamic_source_stats(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<DynamicStatsResponse>, ApiError> {
    let Some(source) = dynamic_source::Entity::find_by_id(id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let stats = upper_stat::Entity::find()
        .filter(upper_stat::Column::UpperId.eq(source.upper_id))
        .order_by_asc(upper_stat::Column::RecordedAt)
        .all(&db)
        .await?;
    let points = stats
        .iter()
        .map(|s| StatPoint {
            recorded_at: s.recorded_at,
            fan_count: s.fan_count,
            follow_count: s.follow_count,
            video_count: s.video_count,
            view_count: s.view_count,
        })
        .collect::<Vec<_>>();
    // 名字/签名版本历史：值发生变化时视为新版本
    let mut versions = Vec::<UpperVersion>::new();
    for s in stats.iter() {
        match versions.last_mut() {
            Some(v) if v.name == s.name && v.sign == s.sign && v.face == s.face => {
                v.end_at = Some(s.recorded_at);
            }
            _ => versions.push(UpperVersion {
                name: s.name.clone(),
                sign: s.sign.clone(),
                face: s.face.clone(),
                start_at: s.recorded_at,
                end_at: None,
            }),
        }
    }
    Ok(ApiResponse::ok(DynamicStatsResponse {
        upper_name: source.upper_name,
        upper_id: source.upper_id,
        stats: points,
        versions,
    }))
}

/// 获取动态源下的动态列表（用于手动重扫单条评论）
pub async fn get_dynamic_source_dynamics(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<Vec<DynamicListItem>>, ApiError> {
    let Some(source) = dynamic_source::Entity::find_by_id(id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let dynamics = dynamic::Entity::find()
        .filter(dynamic::Column::SourceId.eq(source.id))
        .order_by_desc(dynamic::Column::PubTs)
        .all(&db)
        .await?;
    let items = dynamics
        .into_iter()
        .map(|d| DynamicListItem {
            id: d.id.clone(),
            dyn_type: d.dyn_type.clone(),
            content: d.content.chars().take(100).collect(),
            pub_ts: d.pub_ts,
            comment_count: d.stat.as_ref().and_then(|s| s["comment"]["count"].as_i64()).unwrap_or(0),
            rescan_reply: d.rescan_reply,
            path: d.path,
        })
        .collect();
    Ok(ApiResponse::ok(items))
}

/// 手动标记该动态源下所有动态重新同步评论
pub async fn rescan_all_replies(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<usize>, ApiError> {
    let Some(source) = dynamic_source::Entity::find_by_id(id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let dynamics = dynamic::Entity::find()
        .filter(dynamic::Column::SourceId.eq(source.id))
        .all(&db)
        .await?;
    let count = dynamics.len();
    for dyn_model in dynamics {
        let mut model: dynamic::ActiveModel = dyn_model.into();
        model.rescan_reply = Set(true);
        model.download_status = Set(0);
        model.save(&db).await?;
    }
    Ok(ApiResponse::ok(count))
}

/// 手动标记单条动态重新同步评论
pub async fn rescan_single_reply(
    Path((id, dyn_id)): Path<(i32, String)>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<bool>, ApiError> {
    let Some(source) = dynamic_source::Entity::find_by_id(id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let Some(dyn_model) = dynamic::Entity::find_by_id(&dyn_id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    if dyn_model.source_id != source.id {
        return Err(InnerApiError::BadRequest("dynamic does not belong to this source".to_string()).into());
    }
    let mut model: dynamic::ActiveModel = dyn_model.into();
    model.rescan_reply = Set(true);
    model.download_status = Set(0);
    model.save(&db).await?;
    Ok(ApiResponse::ok(true))
}
