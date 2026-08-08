use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Dynamic::Table)
                    .add_column(ColumnDef::new(Dynamic::Location).string().default("").not_null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Dynamic::Table)
                    .drop_column(Dynamic::Location)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Dynamic {
    Table,
    Location,
}
