use std::borrow::Borrow;

use bili_sync_entity::video;
use bili_sync_migration::SimpleExpr;
use itertools::Itertools;
use sea_orm::{ColumnTrait, Condition, ConnectionTrait, DatabaseTransaction};

use crate::api::request::{StatusFilter, ValidationFilter};
use crate::api::response::{PageInfo, SimplePageInfo, SimpleVideoInfo, VideoInfo};
use crate::utils::status::VideoStatus;

impl StatusFilter {
    pub fn to_video_query(&self) -> Condition {
        let query_builder = VideoStatus::query_builder();
        match self {
            Self::Failed => query_builder.failed(),
            Self::Succeeded => query_builder.succeeded(),
            Self::Waiting => query_builder.waiting(),
        }
    }
}

impl ValidationFilter {
    pub fn to_video_query(&self) -> SimpleExpr {
        match self {
            ValidationFilter::Deleted => video::Column::DeletedAt.is_not_null(),
            ValidationFilter::Invalid => video::Column::Valid.eq(false).and(video::Column::DeletedAt.is_null()),
            ValidationFilter::Skipped => video::Column::Valid
                .eq(true)
                .and(video::Column::ShouldDownload.eq(false))
                .and(video::Column::DeletedAt.is_null()),
            ValidationFilter::Normal => video::Column::Valid
                .eq(true)
                .and(video::Column::ShouldDownload.eq(true))
                .and(video::Column::DeletedAt.is_null()),
        }
    }
}

pub trait VideoRecord {
    fn as_id_status_tuple(&self) -> (i32, u32);
}

pub trait PageRecord {
    fn as_id_status_tuple(&self) -> (i32, u32);
}

impl VideoRecord for VideoInfo {
    fn as_id_status_tuple(&self) -> (i32, u32) {
        (self.id, self.download_status)
    }
}

impl VideoRecord for SimpleVideoInfo {
    fn as_id_status_tuple(&self) -> (i32, u32) {
        (self.id, self.download_status)
    }
}

impl PageRecord for PageInfo {
    fn as_id_status_tuple(&self) -> (i32, u32) {
        (self.id, self.download_status)
    }
}

impl PageRecord for SimplePageInfo {
    fn as_id_status_tuple(&self) -> (i32, u32) {
        (self.id, self.download_status)
    }
}

pub async fn update_video_download_status<T>(
    txn: &DatabaseTransaction,
    videos: &[impl Borrow<T>],
    batch_size: Option<usize>,
) -> Result<(), sea_orm::DbErr>
where
    T: VideoRecord,
{
    if videos.is_empty() {
        return Ok(());
    }
    if let Some(size) = batch_size {
        for chunk in videos.chunks(size) {
            execute_video_update_batch(txn, chunk.iter().map(|v| v.borrow().as_id_status_tuple())).await?;
        }
    } else {
        execute_video_update_batch(txn, videos.iter().map(|v| v.borrow().as_id_status_tuple())).await?;
    }
    Ok(())
}

pub async fn update_page_download_status<T>(
    txn: &DatabaseTransaction,
    pages: &[impl Borrow<T>],
    batch_size: Option<usize>,
) -> Result<(), sea_orm::DbErr>
where
    T: PageRecord,
{
    if pages.is_empty() {
        return Ok(());
    }
    if let Some(size) = batch_size {
        for chunk in pages.chunks(size) {
            execute_page_update_batch(txn, chunk.iter().map(|v| v.borrow().as_id_status_tuple())).await?;
        }
    } else {
        execute_page_update_batch(txn, pages.iter().map(|v| v.borrow().as_id_status_tuple())).await?;
    }
    Ok(())
}

async fn execute_video_update_batch(
    txn: &DatabaseTransaction,
    videos: impl Iterator<Item = (i32, u32)>,
) -> Result<(), sea_orm::DbErr> {
    let values = videos.map(|v| format!("({}, {})", v.0, v.1)).join(", ");
    if values.is_empty() {
        return Ok(());
    }
    let sql = format!(
        "WITH tempdata(id, download_status) AS (VALUES {}) \
        UPDATE video \
        SET download_status = tempdata.download_status \
        FROM tempdata \
        WHERE video.id = tempdata.id",
        values
    );
    txn.execute_unprepared(&sql).await?;
    Ok(())
}

async fn execute_page_update_batch(
    txn: &DatabaseTransaction,
    pages: impl Iterator<Item = (i32, u32)>,
) -> Result<(), sea_orm::DbErr> {
    let values = pages.map(|p| format!("({}, {})", p.0, p.1)).join(", ");
    if values.is_empty() {
        return Ok(());
    }
    let sql = format!(
        "WITH tempdata(id, download_status) AS (VALUES {}) \
        UPDATE page \
        SET download_status = tempdata.download_status \
        FROM tempdata \
        WHERE page.id = tempdata.id",
        values
    );
    txn.execute_unprepared(&sql).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bili_sync_migration::{Migrator, MigratorTrait};
    use chrono::Utc;
    use sea_orm::{ActiveModelTrait, Database, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder};

    #[tokio::test]
    async fn validity_filters_keep_deleted_and_unavailable_records_separate() -> anyhow::Result<()> {
        let db = Database::connect("sqlite::memory:").await?;
        Migrator::up(&db, None).await?;
        for (id, valid, should_download, deleted) in [
            (1, true, true, false),
            (2, true, false, false),
            (3, false, true, false),
            (4, false, true, true),
            // 删除标记优先，旧数据的 valid 即使不一致，也不能混入有效或跳过筛选。
            (5, true, false, true),
        ] {
            video::Model {
                id,
                bvid: format!("BVfilter{id}"),
                valid,
                should_download,
                deleted_at: deleted.then(|| Utc::now().naive_utc()),
                ..Default::default()
            }
            .into_active_model()
            .insert(&db)
            .await?;
        }
        for (filter, expected) in [
            (ValidationFilter::Normal, vec![1]),
            (ValidationFilter::Skipped, vec![2]),
            (ValidationFilter::Invalid, vec![3]),
            (serde_json::from_str::<ValidationFilter>("\"deleted\"")?, vec![4, 5]),
        ] {
            let ids = video::Entity::find()
                .filter(filter.to_video_query())
                .order_by_asc(video::Column::Id)
                .all(&db)
                .await?
                .into_iter()
                .map(|row| row.id)
                .collect::<Vec<_>>();
            assert_eq!(ids, expected);
        }
        Ok(())
    }
}
