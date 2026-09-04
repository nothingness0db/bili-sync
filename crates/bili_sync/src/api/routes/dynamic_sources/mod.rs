use std::path::PathBuf;

use anyhow::{Context, Result};
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
use crate::workflow_dynamic::get_source_lock;

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
        let reply_count = reply::Entity::find()
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
            .filter(reply::Column::Valid.eq(true))
            .count(&db)
            .await?
            .try_into()?;
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

/// 回滚已完成的动态目录迁移；失败时尽力恢复文件系统状态。
async fn rollback_moved_dynamic_dirs(moved: &[(PathBuf, PathBuf)]) {
    for (old_dir, new_dir) in moved.iter().rev() {
        let _ = tokio::fs::rename(new_dir, old_dir).await;
    }
}

/// 更新动态源；修改保存路径时同步迁移已有动态目录并更新数据库中的路径。
pub async fn update_dynamic_source(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
    ValidatedJson(request): ValidatedJson<UpdateDynamicSourceRequest>,
) -> Result<ApiResponse<bool>, ApiError> {
    let source_lock = get_source_lock(id);
    let _source_lock_guard = source_lock.lock().await;
    let Some(model) = dynamic_source::Entity::find_by_id(id).one(&db).await? else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let old_path = PathBuf::from(&model.path);
    let new_path = PathBuf::from(&request.path);
    if old_path != new_path && new_path.starts_with(&old_path) {
        return Err(
            InnerApiError::BadRequest("new path cannot be inside the current dynamic source path".to_string()).into(),
        );
    }
    let dynamics = if old_path != new_path {
        dynamic::Entity::find()
            .filter(dynamic::Column::SourceId.eq(id))
            .all(&db)
            .await?
    } else {
        Vec::new()
    };

    let mut moved = Vec::new();
    if old_path != new_path {
        tokio::fs::create_dir_all(&new_path)
            .await
            .with_context(|| format!("failed to create dynamic source directory {}", new_path.display()))?;
        for dyn_model in &dynamics {
            if dyn_model.path.is_empty() {
                continue;
            }
            let old_dir = PathBuf::from(&dyn_model.path);
            let Some(name) = old_dir.file_name() else {
                continue;
            };
            let new_dir = new_path.join(name);
            if old_dir == new_dir {
                continue;
            }
            let target_exists = match tokio::fs::try_exists(&new_dir).await {
                Ok(exists) => exists,
                Err(error) => {
                    rollback_moved_dynamic_dirs(&moved).await;
                    return Err(anyhow::Error::from(error)
                        .context(format!("failed to inspect target directory {}", new_dir.display()))
                        .into());
                }
            };
            if target_exists {
                rollback_moved_dynamic_dirs(&moved).await;
                return Err(anyhow::anyhow!(
                    "cannot migrate dynamic {}: target directory {} already exists",
                    dyn_model.id,
                    new_dir.display()
                )
                .into());
            }
            let source_exists = match tokio::fs::try_exists(&old_dir).await {
                Ok(exists) => exists,
                Err(error) => {
                    rollback_moved_dynamic_dirs(&moved).await;
                    return Err(anyhow::Error::from(error)
                        .context(format!("failed to inspect source directory {}", old_dir.display()))
                        .into());
                }
            };
            if source_exists {
                if let Err(error) = tokio::fs::rename(&old_dir, &new_dir).await {
                    rollback_moved_dynamic_dirs(&moved).await;
                    return Err(anyhow::Error::from(error)
                        .context(format!("failed to move {} to {}", old_dir.display(), new_dir.display()))
                        .into());
                }
                moved.push((old_dir, new_dir));
            }
        }
    }

    let txn = match db.begin().await {
        Ok(txn) => txn,
        Err(error) => {
            rollback_moved_dynamic_dirs(&moved).await;
            return Err(error.into());
        }
    };
    let result: Result<(), anyhow::Error> = async {
        for dyn_model in dynamics {
            let old_dir = PathBuf::from(&dyn_model.path);
            let Some(name) = old_dir.file_name() else {
                continue;
            };
            if !old_dir.as_os_str().is_empty() && old_dir != new_path.join(name) {
                let mut active: dynamic::ActiveModel = dyn_model.into();
                active.path = Set(new_path.join(name).to_string_lossy().to_string());
                active.update(&txn).await?;
            }
        }
        let mut active_model: dynamic_source::ActiveModel = model.into();
        active_model.path = Set(request.path);
        active_model.enabled = Set(request.enabled);
        active_model.sync_reply = Set(request.sync_reply);
        active_model.update(&txn).await?;
        txn.commit().await?;
        Ok(())
    }
    .await;
    if let Err(error) = result {
        for (from, to) in moved.into_iter().rev() {
            let _ = tokio::fs::rename(&to, &from).await;
        }
        return Err(error.into());
    }
    Ok(ApiResponse::ok(true))
}

/// 删除动态源（连同动态与评论）
pub async fn remove_dynamic_source(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<bool>, ApiError> {
    let source_lock = get_source_lock(id);
    let _source_lock_guard = source_lock.lock().await;
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
