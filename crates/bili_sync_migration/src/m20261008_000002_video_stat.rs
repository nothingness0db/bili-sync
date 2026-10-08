use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 同一个 BV 可以出现在多个账号投稿中，采样归属独立于详情中的 owner。
        manager
            .create_table(
                Table::create()
                    .table(VideoStatSource::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(VideoStatSource::UpperId).big_integer().not_null())
                    .col(ColumnDef::new(VideoStatSource::Bvid).string().not_null())
                    .primary_key(Index::create().col(VideoStatSource::UpperId).col(VideoStatSource::Bvid))
                    .to_owned(),
            )
            .await?;
        let mut table = Table::create();
        table
            .table(VideoStat::Table)
            .if_not_exists()
            .col(
                ColumnDef::new(VideoStat::Id)
                    .big_integer()
                    .not_null()
                    .auto_increment()
                    .primary_key(),
            )
            .col(ColumnDef::new(VideoStat::Bvid).string().not_null())
            .col(ColumnDef::new(VideoStat::Aid).big_integer().null())
            .col(ColumnDef::new(VideoStat::UpperId).big_integer().not_null())
            .col(ColumnDef::new(VideoStat::Title).string().not_null())
            .col(ColumnDef::new(VideoStat::Cover).string().not_null())
            .col(ColumnDef::new(VideoStat::Pubtime).timestamp().not_null());
        for column in [
            VideoStat::ViewCount,
            VideoStat::LikeCount,
            VideoStat::CoinCount,
            VideoStat::FavoriteCount,
            VideoStat::ShareCount,
            VideoStat::ReplyCount,
            VideoStat::DanmakuCount,
        ] {
            table.col(ColumnDef::new(column).big_integer().null());
        }
        table.col(ColumnDef::new(VideoStat::RecordedAt).timestamp().not_null());
        manager.create_table(table.to_owned()).await?;
        for (name, columns) in [
            (
                "idx_video_stat_bvid_time",
                vec![VideoStat::Bvid, VideoStat::RecordedAt, VideoStat::Id],
            ),
            ("idx_video_stat_upper_bvid", vec![VideoStat::UpperId, VideoStat::Bvid]),
        ] {
            if !manager.has_index("video_stat", name).await? {
                let mut index = Index::create();
                index.table(VideoStat::Table).name(name);
                for column in columns {
                    index.col(column);
                }
                manager.create_index(index.to_owned()).await?;
            }
        }
        if !manager.has_column("dynamic", "video_bvid").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Dynamic::Table)
                        .add_column(ColumnDef::new(Dynamic::VideoBvid).string().null())
                        .to_owned(),
                )
                .await?;
        }
        // 只建立已有 JSON 的视频关联，不把历史值伪装成新采样。
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE dynamic SET video_bvid = json_extract(raw, '$.modules.module_dynamic.major.archive.bvid') \
             WHERE dyn_type = 'DYNAMIC_TYPE_AV' AND video_bvid IS NULL AND json_valid(raw)",
            )
            .await?;
        if !manager.has_index("dynamic", "idx_dynamic_video_bvid").await? {
            manager
                .create_index(
                    Index::create()
                        .table(Dynamic::Table)
                        .name("idx_dynamic_video_bvid")
                        .col(Dynamic::VideoBvid)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_index("dynamic", "idx_dynamic_video_bvid").await? {
            manager
                .drop_index(
                    Index::drop()
                        .table(Dynamic::Table)
                        .name("idx_dynamic_video_bvid")
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("dynamic", "video_bvid").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Dynamic::Table)
                        .drop_column(Dynamic::VideoBvid)
                        .to_owned(),
                )
                .await?;
        }
        manager
            .drop_table(Table::drop().table(VideoStatSource::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(VideoStat::Table).if_exists().to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum VideoStat {
    Table,
    Id,
    Bvid,
    Aid,
    UpperId,
    Title,
    Cover,
    Pubtime,
    ViewCount,
    LikeCount,
    CoinCount,
    FavoriteCount,
    ShareCount,
    ReplyCount,
    DanmakuCount,
    RecordedAt,
}

#[derive(DeriveIden)]
enum Dynamic {
    Table,
    VideoBvid,
}

#[derive(DeriveIden)]
enum VideoStatSource {
    Table,
    UpperId,
    Bvid,
}
