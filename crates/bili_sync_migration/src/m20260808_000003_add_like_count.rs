use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UpperStat::Table)
                    .add_column(ColumnDef::new(UpperStat::LikeCount).unsigned().default(0).not_null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UpperStat::Table)
                    .drop_column(UpperStat::LikeCount)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum UpperStat {
    Table,
    LikeCount,
}
