use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::body::Body;
use axum::extract::{Extension, Path, Query};
use axum::http::{StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use bili_sync_entity::*;
use sea_orm::ActiveValue::Set;
use sea_orm::entity::prelude::*;
use sea_orm::{DatabaseConnection, QueryFilter, QueryOrder, QuerySelect, QueryTrait};
use serde::Deserialize;

use crate::api::error::InnerApiError;
use crate::api::response::{
    DynamicDetailResponse, DynamicListItem, DynamicStatsResponse, ReplyItem, StatPoint, UpperVersion,
};
use crate::api::wrapper::{ApiError, ApiResponse};
use crate::bilibili::BiliClient;
use crate::config::VersionedConfig;
use crate::workflow_dynamic::{ensure_mixin_key, process_dynamic_source_queued, update_upper_stat};

pub(super) fn router() -> Router {
    Router::new()
        .route("/dynamic-sources/{id}/stats", get(get_dynamic_source_stats))
        .route("/dynamic-sources/{id}/scan-profile", post(scan_profile))
        .route("/dynamic-sources/{id}/sync-now", post(sync_now))
        .route("/dynamic-sources/{id}/dynamics", get(get_dynamic_source_dynamics))
        .route(
            "/dynamic-sources/{id}/dynamics/{dyn_id}/detail",
            get(get_dynamic_detail),
        )
        .route("/dynamic-sources/{id}/dynamics/{dyn_id}/file", get(get_dynamic_file))
        .route("/dynamic-sources/{id}/rescan-replies", post(rescan_all_replies))
        .route(
            "/dynamic-sources/{id}/dynamics/{dyn_id}/rescan-reply",
            post(rescan_single_reply),
        )
}

#[derive(Deserialize)]
pub struct DynamicFileRequest {
    /// 相对动态目录的路径，如 "pics/01.jpg" 或 "comments/309397270945_1.jpg"
    pub name: String,
}

/// 返回动态目录下的文件（仅允许 pics/ 与 comments/ 下的图片）
pub async fn get_dynamic_file(
    Path((id, dyn_id)): Path<(i32, String)>,
    Query(params): Query<DynamicFileRequest>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<Response, ApiError> {
    let Some(source) = dynamic_source::Entity::find_by_id(id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let Some(dyn_model) = dynamic::Entity::find_by_id(&dyn_id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    if dyn_model.source_id != source.id {
        return Err(InnerApiError::BadRequest("dynamic does not belong to this source".to_string()).into());
    }
    let name = params.name;
    // 只允许 pics/ 与 comments/ 前缀，禁止路径穿越
    if !(name.starts_with("pics/") || name.starts_with("comments/")) || name.contains("..") {
        return Err(InnerApiError::BadRequest("invalid file name".to_string()).into());
    }
    let file_path = PathBuf::from(&dyn_model.path).join(&name);
    match tokio::fs::read(&file_path).await {
        Ok(bytes) => {
            let mime = match file_path.extension().and_then(|e| e.to_str()) {
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("png") => "image/png",
                Some("gif") => "image/gif",
                Some("webp") => "image/webp",
                Some("json") => "application/json",
                Some("md") => "text/markdown; charset=utf-8",
                _ => "application/octet-stream",
            };
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .body(Body::from(bytes))
                .expect("failed to build response"))
        }
        Err(_) => Err(InnerApiError::NotFound(id).into()),
    }
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
    // 手动扫描路径不依赖定时任务，独立初始化 wbi 签名密钥
    ensure_mixin_key(&bili_client, &config.credential).await?;
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
            dynamic_video_count: s.dynamic_video_count,
            view_count: s.view_count,
            like_count: s.like_count,
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
    // 一次聚合查询拿到所有动态的本地评论数，避免 N+1
    #[derive(sea_orm::FromQueryResult)]
    struct ReplyCountRow {
        dynamic_id: String,
        cnt: i64,
    }
    let reply_counts: std::collections::HashMap<String, i64> = reply::Entity::find()
        .filter(
            reply::Column::DynamicId.in_subquery(
                dynamic::Entity::find()
                    .filter(dynamic::Column::SourceId.eq(source.id))
                    .select_only()
                    .column(dynamic::Column::Id)
                    .as_query()
                    .to_owned(),
            ),
        )
        .select_only()
        .column(reply::Column::DynamicId)
        .column_as(reply::Column::Rpid.count(), "cnt")
        .group_by(reply::Column::DynamicId)
        .into_model::<ReplyCountRow>()
        .all(&db)
        .await?
        .into_iter()
        .map(|row| (row.dynamic_id, row.cnt))
        .collect();
    let mut items = Vec::with_capacity(dynamics.len());
    for d in dynamics {
        items.push(DynamicListItem {
            id: d.id.clone(),
            dyn_type: d.dyn_type.clone(),
            content: d.content.chars().take(100).collect(),
            pub_ts: d.pub_ts,
            comment_count: d
                .stat
                .as_ref()
                .and_then(|s| s["comment"]["count"].as_i64())
                .unwrap_or(0),
            reply_count: reply_counts.get(&d.id).copied().unwrap_or(0),
            rescan_reply: d.rescan_reply,
            path: d.path,
            valid: d.valid,
        });
    }
    Ok(ApiResponse::ok(items))
}

/// 获取动态详情（正文全文 + 评论树）
pub async fn get_dynamic_detail(
    Path((id, dyn_id)): Path<(i32, String)>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<DynamicDetailResponse>, ApiError> {
    let Some(source) = dynamic_source::Entity::find_by_id(id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let Some(dyn_model) = dynamic::Entity::find_by_id(&dyn_id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    if dyn_model.source_id != source.id {
        return Err(InnerApiError::BadRequest("dynamic does not belong to this source".to_string()).into());
    }
    let pics: Vec<String> = dyn_model
        .pics
        .clone()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let replies = reply::Entity::find()
        .filter(reply::Column::DynamicId.eq(&dyn_id))
        .filter(reply::Column::Valid.eq(true))
        .order_by_asc(reply::Column::Ctime)
        .all(&db)
        .await?;
    // 在内存中构建评论树
    let mut reply_map = std::collections::HashMap::<i64, ReplyItem>::new();
    for r in replies.iter() {
        let images: Vec<String> = r
            .images
            .clone()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        reply_map.insert(
            r.rpid,
            ReplyItem {
                rpid: r.rpid,
                parent_rpid: r.parent_rpid,
                uname: r.uname.clone(),
                avatar: r.avatar.clone(),
                content: r.content.clone(),
                images,
                ctime: r.ctime,
                sub_replies: Vec::new(),
            },
        );
    }
    let mut top_replies = Vec::new();
    for r in replies.iter() {
        if let Some(parent_rpid) = r.parent_rpid
            && reply_map.contains_key(&parent_rpid)
        {
            let sub = reply_map.remove(&r.rpid).expect("reply should exist");
            reply_map
                .get_mut(&parent_rpid)
                .expect("parent reply should exist")
                .sub_replies
                .push(sub);
        } else {
            top_replies.push(reply_map.remove(&r.rpid).expect("reply should exist"));
        }
    }
    Ok(ApiResponse::ok(DynamicDetailResponse {
        id: dyn_model.id.clone(),
        dyn_type: dyn_model.dyn_type.clone(),
        content: dyn_model.content.clone(),
        pics,
        stat: dyn_model.stat.clone().unwrap_or(serde_json::Value::Null),
        pub_ts: dyn_model.pub_ts,
        location: dyn_model.location.clone(),
        path: dyn_model.path.clone(),
        replies: top_replies,
    }))
}

/// 立即执行一轮完整的动态同步（账号快照 + 动态列表 + 评论），不等定时任务
pub async fn sync_now(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
    Extension(bili_client): Extension<Arc<BiliClient>>,
) -> Result<ApiResponse<bool>, ApiError> {
    let Some(source) = dynamic_source::Entity::find_by_id(id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let connection = db.clone();
    tokio::spawn(async move {
        let config = VersionedConfig::get().snapshot();
        // 排队执行：等待该源正在进行的同步结束后再跑，不跳过
        match process_dynamic_source_queued(source, &bili_client, &connection, &config).await {
            Ok(()) => info!("手动触发的动态同步完成"),
            Err(e) => error!("手动触发的动态同步失败：{:#}", e),
        }
    });
    Ok(ApiResponse::ok(true))
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
