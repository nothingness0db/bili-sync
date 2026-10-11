use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(VideoAvailability::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(VideoAvailability::Bvid)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(VideoAvailability::ErrorCode).big_integer().not_null())
                    .col(ColumnDef::new(VideoAvailability::CheckedAt).timestamp().not_null())
                    .col(ColumnDef::new(VideoAvailability::RetryAt).timestamp().null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .table(Video::Table)
                    .name("idx_video_bvid")
                    .if_not_exists()
                    .col(Video::Bvid)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().table(Video::Table).name("idx_video_bvid").to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(VideoAvailability::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum VideoAvailability {
    Table,
    Bvid,
    ErrorCode,
    CheckedAt,
    RetryAt,
}

#[derive(DeriveIden)]
enum Video {
    Table,
    Bvid,
}
