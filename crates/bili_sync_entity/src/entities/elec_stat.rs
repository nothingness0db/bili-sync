use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Default)]
#[sea_orm(table_name = "elec_stat")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub upper_id: i64,
    pub show: bool,
    pub state: i64,
    /// 接口返回的累计充电人数，不是当前活跃人数。
    pub total: Option<i64>,
    pub upower_count_show: Option<bool>,
    pub list_available: bool,
    pub recorded_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
impl ActiveModelBehavior for ActiveModel {}
