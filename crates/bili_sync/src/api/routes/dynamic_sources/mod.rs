use anyhow::Result;
use axum::Router;
use axum::extract::{Extension, Path};
use axum::routing::{get, put};
use bili_sync_entity::*;
use sea_orm::ActiveValue::Set;
use sea_orm::entity::prelude::*;
use sea_orm::{DatabaseConnection, QuerySelect, QueryTrait, TransactionTrait};

use crate::api::error::InnerApiError;
use crate::api::request::{InsertDynamicSourceRequest, UpdateDynamicSourceRequest};
use crate::api::response::{DynamicSourceDetail, DynamicSourcesResponse};
use crate::api::wrapper::{ApiError, ApiResponse, ValidatedJson};
use crate::bilibili::{BiliClient, Submission};
use crate::config::VersionedConfig;

pub(super) fn router() -> Router {
    Router::new()
        .route("/dynamic-sources", get(get_dynamic_sources).post(insert_dynamic_source))
        .route("/dynamic-sources/details", get(get_dynamic_sources_details))
        .route(
            "/dynamic-sources/{id}",
            put(update_dynamic_source).delete(remove_dynamic_source),
        )
}

/// 列出所有动态源
pub async fn get_dynamic_sources(
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<DynamicSourcesResponse>, ApiError> {
    let sources = dynamic_source::Entity::find().all(&db).await?;
    let dynamic_sources = sources
        .into_iter()
        .map(|s| DynamicSourceDetail {
            id: s.id,
            upper_id: s.upper_id,
            upper_name: s.upper_name,
            path: s.path,
            sync_reply: s.sync_reply,
            enabled: s.enabled,
            latest_dyn_at: Some(s.latest_dyn_at),
            dynamic_count: 0,
            reply_count: 0,
        })
        .collect();
    Ok(ApiResponse::ok(DynamicSourcesResponse { dynamic_sources }))
}

/// 获取动态源详情（含动态/评论数量）
pub async fn get_dynamic_sources_details(
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<Vec<DynamicSourceDetail>>, ApiError> {
    let sources = dynamic_source::Entity::find().all(&db).await?;
    let mut details = Vec::with_capacity(sources.len());
    for source in sources {
        let dynamic_count = dynamic::Entity::find()
            .filter(dynamic::Column::SourceId.eq(source.id))
            .count(&db)
            .await?
            .try_into()?;
        let reply_count = dynamic::Entity::find()
            .filter(dynamic::Column::SourceId.eq(source.id))
            .find_with_related(reply::Entity)
            .all(&db)
            .await
            .map(|pairs| pairs.into_iter().map(|(_, r)| r.len()).sum())
            .unwrap_or(0);
        details.push(DynamicSourceDetail {
            id: source.id,
            upper_id: source.upper_id,
            upper_name: source.upper_name,
            path: source.path,
            sync_reply: source.sync_reply,
            enabled: source.enabled,
            latest_dyn_at: Some(source.latest_dyn_at),
            dynamic_count,
            reply_count,
        });
    }
    Ok(ApiResponse::ok(details))
}

/// 新增动态源
pub async fn insert_dynamic_source(
    Extension(db): Extension<DatabaseConnection>,
    Extension(bili_client): Extension<std::sync::Arc<BiliClient>>,
    ValidatedJson(request): ValidatedJson<InsertDynamicSourceRequest>,
) -> Result<ApiResponse<bool>, ApiError> {
    let credential = &VersionedConfig::get().read().credential;
    let submission = Submission::new(bili_client.as_ref(), request.upper_id.to_string(), credential);
    let upper = submission.get_info().await?;
    dynamic_source::Entity::insert(dynamic_source::ActiveModel {
        upper_id: Set(upper.mid.parse()?),
        upper_name: Set(upper.name),
        path: Set(request.path),
        sync_reply: Set(request.sync_reply),
        enabled: Set(false),
        // 新源必须从 epoch 开始，否则首次扫描只会拉到置顶的一条动态
        latest_dyn_at: Set(chrono::DateTime::from_timestamp(0, 0)
            .expect("epoch is valid")
            .naive_utc()),
        ..Default::default()
    })
    .exec(&db)
    .await?;
    Ok(ApiResponse::ok(true))
}

/// 更新动态源
pub async fn update_dynamic_source(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
    ValidatedJson(request): ValidatedJson<UpdateDynamicSourceRequest>,
) -> Result<ApiResponse<bool>, ApiError> {
    let Some(model) = dynamic_source::Entity::find_by_id(id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let mut active_model: dynamic_source::ActiveModel = model.into();
    active_model.path = Set(request.path);
    active_model.enabled = Set(request.enabled);
    active_model.sync_reply = Set(request.sync_reply);
    active_model.save(&db).await?;
    Ok(ApiResponse::ok(true))
}

/// 删除动态源（连同动态与评论）
pub async fn remove_dynamic_source(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<bool>, ApiError> {
    let Some(source) = dynamic_source::Entity::find_by_id(id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let txn = db.begin().await?;
    reply::Entity::delete_many()
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
        .exec(&txn)
        .await?;
    dynamic::Entity::delete_many()
        .filter(dynamic::Column::SourceId.eq(source.id))
        .exec(&txn)
        .await?;
    dynamic_source::Entity::delete_by_id(source.id).exec(&txn).await?;
    txn.commit().await?;
    Ok(ApiResponse::ok(true))
}
