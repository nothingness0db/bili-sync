use std::path::{Path as FsPath, PathBuf};

use anyhow::{Context, Result, bail};
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

/// 复制动态目录，避免在数据库事务提交前破坏旧目录。
async fn copy_dynamic_dir(source: &FsPath, target: &FsPath) -> Result<()> {
    tokio::fs::create_dir_all(target).await?;
    let mut entries = tokio::fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry.file_type().await?;
        if file_type.is_dir() {
            Box::pin(copy_dynamic_dir(&source_path, &target_path)).await?;
        } else if file_type.is_file() {
            tokio::fs::copy(&source_path, &target_path).await?;
        } else {
            bail!("unsupported file type in dynamic directory {}", source_path.display());
        }
    }
    Ok(())
}

/// 删除事务失败时创建的新目录；旧目录仍保留，数据库路径因此仍然有效。
async fn cleanup_copied_dynamic_dirs(copied: &[(PathBuf, PathBuf)]) {
    for (_, new_dir) in copied.iter().rev() {
        if let Err(error) = tokio::fs::remove_dir_all(new_dir).await {
            tracing::warn!(
                path = %new_dir.display(),
                %error,
                "清理动态目录副本失败，保留副本以便后续人工处理"
            );
        }
    }
}

/// 更新动态源；修改保存路径时先复制已有动态目录，再提交数据库路径变更。
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

    let mut copied = Vec::new();
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
                    cleanup_copied_dynamic_dirs(&copied).await;
                    return Err(anyhow::Error::from(error)
                        .context(format!("failed to inspect target directory {}", new_dir.display()))
                        .into());
                }
            };
            if target_exists {
                cleanup_copied_dynamic_dirs(&copied).await;
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
                    cleanup_copied_dynamic_dirs(&copied).await;
                    return Err(anyhow::Error::from(error)
                        .context(format!("failed to inspect source directory {}", old_dir.display()))
                        .into());
                }
            };
            if source_exists {
                if let Err(error) = copy_dynamic_dir(&old_dir, &new_dir).await {
                    cleanup_copied_dynamic_dirs(&copied).await;
                    if let Err(cleanup_error) = tokio::fs::remove_dir_all(&new_dir).await {
                        tracing::warn!(
                            path = %new_dir.display(),
                            %cleanup_error,
                            "清理复制失败的动态目录副本失败"
                        );
                    }
                    return Err(error
                        .context(format!("failed to copy {} to {}", old_dir.display(), new_dir.display()))
                        .into());
                }
                copied.push((old_dir, new_dir));
            }
        }
    }

    let txn = match db.begin().await {
        Ok(txn) => txn,
        Err(error) => {
            cleanup_copied_dynamic_dirs(&copied).await;
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
        cleanup_copied_dynamic_dirs(&copied).await;
        return Err(error.into());
    }

    // 新目录已完整复制且数据库已提交；旧目录现在只是冗余副本。
    for (old_dir, _) in copied {
        if let Err(error) = tokio::fs::remove_dir_all(&old_dir).await {
            tracing::warn!(
                path = %old_dir.display(),
                %error,
                "删除旧动态目录失败，数据库已切换到新目录"
            );
        }
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
