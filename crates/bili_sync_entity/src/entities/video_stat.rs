use sea_orm::entity::prelude::*;

/// 一次成功的视频详情采样；不依赖下载记录，动态视频也可独立留存。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Default)]
#[sea_orm(table_name = "video_stat")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub bvid: String,
    pub aid: Option<i64>,
    pub upper_id: i64,
    pub title: String,
    pub cover: String,
    pub pubtime: DateTime,
    pub view_count: Option<i64>,
    pub like_count: Option<i64>,
    pub coin_count: Option<i64>,
    pub favorite_count: Option<i64>,
    pub share_count: Option<i64>,
    pub reply_count: Option<i64>,
    pub danmaku_count: Option<i64>,
    pub recorded_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
