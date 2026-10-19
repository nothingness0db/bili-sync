use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{Statement, TransactionTrait};

#[derive(DeriveMigrationName)]
pub struct Migration;

// 合作视频会在多个账号下生成动态，同一个 rpid 必须能关联到每条动态。
const COLUMNS: &str =
    "rpid, dynamic_id, parent_rpid, uname, avatar, content, images, ctime, raw, download_status, valid";

async fn rebuild(manager: &SchemaManager<'_>, scoped: bool) -> Result<(), DbErr> {
    let txn = manager.get_connection().begin().await?;
    let key = if scoped { "rpid, dynamic_id" } else { "rpid" };
    txn.execute_unprepared(&format!(
        "CREATE TABLE reply_rebuild (
            rpid BIGINT NOT NULL, dynamic_id TEXT NOT NULL, parent_rpid BIGINT,
            uname TEXT NOT NULL, avatar TEXT NOT NULL, content TEXT NOT NULL,
            images JSON, ctime TIMESTAMP NOT NULL, raw TEXT,
            download_status INTEGER NOT NULL DEFAULT 0, valid BOOLEAN NOT NULL DEFAULT 1,
            PRIMARY KEY ({key}))"
    ))
    .await?;
    txn.execute_unprepared(&format!(
        "INSERT INTO reply_rebuild ({COLUMNS}) SELECT {COLUMNS} FROM reply"
    ))
    .await?;
    if scoped {
        // 补齐升级前因 rpid 冲突而丢失的关联，不发网络请求，也不删除历史。
        txn.execute_unprepared(
            "INSERT OR IGNORE INTO reply_rebuild
             SELECT r.rpid, target.id, r.parent_rpid, r.uname, r.avatar, r.content,
                    r.images, r.ctime, r.raw, r.download_status, r.valid
             FROM reply r JOIN dynamic origin ON origin.id = r.dynamic_id
             JOIN dynamic target ON target.comment_type = origin.comment_type
                AND target.comment_oid = origin.comment_oid
             WHERE origin.comment_type > 0 AND origin.comment_oid <> ''",
        )
        .await?;
    }
    txn.execute_unprepared("DROP TABLE reply").await?;
    txn.execute_unprepared("ALTER TABLE reply_rebuild RENAME TO reply")
        .await?;
    txn.execute_unprepared("CREATE INDEX idx_reply_dynamic ON reply (dynamic_id)")
        .await?;
    if scoped {
        txn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_dynamic_comment_object ON dynamic (comment_type, comment_oid, id)",
        )
        .await?;
    } else {
        txn.execute_unprepared("DROP INDEX IF EXISTS idx_dynamic_comment_object")
            .await?;
    }
    txn.commit().await
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rebuild(manager, true).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager
            .get_connection()
            .query_one(Statement::from_string(
                manager.get_database_backend(),
                "SELECT rpid FROM reply GROUP BY rpid HAVING COUNT(*) > 1 LIMIT 1".to_owned(),
            ))
            .await?
            .is_some()
        {
            return Err(DbErr::Custom(
                "shared video replies cannot be downgraded without losing dynamic associations".into(),
            ));
        }
        rebuild(manager, false).await
    }
}
