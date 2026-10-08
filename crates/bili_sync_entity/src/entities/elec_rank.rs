use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Default)]
#[sea_orm(table_name = "elec_rank")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub snapshot_id: i64,
    pub rank: i64,
    pub pay_mid: i64,
    pub uname: String,
    pub avatar: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
impl ActiveModelBehavior for ActiveModel {}
