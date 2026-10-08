use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ElecStat::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ElecStat::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ElecStat::UpperId).big_integer().not_null())
                    .col(ColumnDef::new(ElecStat::Show).boolean().not_null())
                    .col(ColumnDef::new(ElecStat::State).big_integer().not_null())
                    .col(ColumnDef::new(ElecStat::Total).big_integer().null())
                    .col(ColumnDef::new(ElecStat::UpowerCountShow).boolean().null())
                    .col(ColumnDef::new(ElecStat::ListAvailable).boolean().not_null())
                    .col(ColumnDef::new(ElecStat::RecordedAt).timestamp().not_null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(ElecRank::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ElecRank::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ElecRank::SnapshotId).big_integer().not_null())
                    .col(ColumnDef::new(ElecRank::Rank).big_integer().not_null())
                    .col(ColumnDef::new(ElecRank::PayMid).big_integer().not_null())
                    .col(ColumnDef::new(ElecRank::Uname).string().not_null())
                    .col(ColumnDef::new(ElecRank::Avatar).string().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .from(ElecRank::Table, ElecRank::SnapshotId)
                            .to(ElecStat::Table, ElecStat::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        if !manager.has_index("elec_stat", "idx_elec_stat_upper_time").await? {
            manager
                .create_index(
                    Index::create()
                        .table(ElecStat::Table)
                        .name("idx_elec_stat_upper_time")
                        .col(ElecStat::UpperId)
                        .col(ElecStat::RecordedAt)
                        .col(ElecStat::Id)
                        .to_owned(),
                )
                .await?;
        }
        if !manager.has_index("elec_rank", "idx_elec_rank_snapshot_member").await? {
            manager
                .create_index(
                    Index::create()
                        .table(ElecRank::Table)
                        .name("idx_elec_rank_snapshot_member")
                        .col(ElecRank::SnapshotId)
                        .col(ElecRank::PayMid)
                        .unique()
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ElecRank::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ElecStat::Table).if_exists().to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ElecStat {
    Table,
    Id,
    UpperId,
    Show,
    State,
    Total,
    UpowerCountShow,
    ListAvailable,
    RecordedAt,
}
#[derive(DeriveIden)]
enum ElecRank {
    Table,
    Id,
    SnapshotId,
    Rank,
    PayMid,
    Uname,
    Avatar,
}
