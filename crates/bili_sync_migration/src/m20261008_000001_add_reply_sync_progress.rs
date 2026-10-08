use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite 的一次 ALTER TABLE 只能添加一列，SeaORM 也不会将 SQLite 迁移包在事务中。
        // 启动中断可能留下部分列但没有迁移记录；重试时跳过已存在的列。
        for (column, definition) in [
            (
                Dynamic::ReplySyncState,
                ColumnDef::new(Dynamic::ReplySyncState).json().null().to_owned(),
            ),
            (
                Dynamic::ReplyLastAttemptAt,
                ColumnDef::new(Dynamic::ReplyLastAttemptAt)
                    .timestamp()
                    .null()
                    .to_owned(),
            ),
            (
                Dynamic::ReplySyncedAt,
                ColumnDef::new(Dynamic::ReplySyncedAt).timestamp().null().to_owned(),
            ),
        ] {
            if !manager
                .has_column(Dynamic::Table.to_string(), column.to_string())
                .await?
            {
                manager
                    .alter_table(Table::alter().table(Dynamic::Table).add_column(definition).to_owned())
                    .await?;
            }
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            Dynamic::ReplySyncState,
            Dynamic::ReplyLastAttemptAt,
            Dynamic::ReplySyncedAt,
        ] {
            if manager
                .has_column(Dynamic::Table.to_string(), column.to_string())
                .await?
            {
                manager
                    .alter_table(Table::alter().table(Dynamic::Table).drop_column(column).to_owned())
                    .await?;
            }
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Dynamic {
    Table,
    ReplySyncState,
    ReplyLastAttemptAt,
    ReplySyncedAt,
}
