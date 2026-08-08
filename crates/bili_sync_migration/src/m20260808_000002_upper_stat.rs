use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UpperStat::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UpperStat::Id)
                            .unsigned()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(UpperStat::UpperId).unsigned().not_null())
                    .col(ColumnDef::new(UpperStat::Name).string().not_null())
                    .col(ColumnDef::new(UpperStat::Sign).string().not_null())
                    .col(ColumnDef::new(UpperStat::Face).string().not_null())
                    .col(ColumnDef::new(UpperStat::FanCount).unsigned().not_null())
                    .col(ColumnDef::new(UpperStat::FollowCount).unsigned().not_null())
                    .col(ColumnDef::new(UpperStat::VideoCount).unsigned().not_null())
                    .col(ColumnDef::new(UpperStat::ViewCount).unsigned().not_null())
                    .col(
                        ColumnDef::new(UpperStat::RecordedAt)
                            .timestamp()
                            .default(Expr::current_timestamp())
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .table(UpperStat::Table)
                    .name("idx_upper_stat_upper")
                    .col(UpperStat::UpperId)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Dynamic::Table)
                    .add_column(ColumnDef::new(Dynamic::RescanReply).boolean().default(false).not_null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Dynamic::Table)
                    .drop_column(Dynamic::RescanReply)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(UpperStat::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum UpperStat {
    Table,
    Id,
    UpperId,
    Name,
    Sign,
    Face,
    FanCount,
    FollowCount,
    VideoCount,
    ViewCount,
    RecordedAt,
}

#[derive(DeriveIden)]
enum Dynamic {
    Table,
    RescanReply,
}
