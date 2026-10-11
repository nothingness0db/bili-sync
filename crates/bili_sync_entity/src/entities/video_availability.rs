use sea_orm::entity::prelude::*;

/// 明确不可用的视频：确认删除后不再自动请求，不可见则到期复查。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "video_availability")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub bvid: String,
    pub error_code: i64,
    pub checked_at: DateTime,
    /// null 表示确认删除，仅手动检查可解除；否则为下次自动复查时间。
    pub retry_at: Option<DateTime>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
