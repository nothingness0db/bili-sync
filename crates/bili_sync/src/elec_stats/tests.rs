use bili_sync_migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, PaginatorTrait};
use serde_json::json;

use super::*;

#[tokio::test]
async fn snapshots_keep_unchanged_boards_and_distinguish_empty_from_unobservable() -> Result<()> {
    let db = Database::connect("sqlite::memory:").await?;
    Migrator::up(&db, None).await?;
    let info = ElecInfo::parse(&json!({"data":{"elec":{"show_info":{
        "show":true,"state":3,"total":14,"list":[{"rank":1,"pay_mid":123,"uname":"test"}]
    }}}}))
    .unwrap();
    for _ in 0..2 {
        save_snapshot(42, &info, Utc::now(), &db).await?;
    }
    assert_eq!(elec_stat::Entity::find().count(&db).await?, 2);
    assert_eq!(elec_rank::Entity::find().count(&db).await?, 2);
    for (state, show, known) in [(1, true, true), (3, true, false), (-1, false, false)] {
        let parsed = ElecInfo::parse(&json!({"data":{"elec":{"show_info":{"show":show,"state":state}}}})).unwrap();
        assert_eq!(parsed.list_available(), known);
        assert_eq!(parsed.total, None);
    }
    for bad in [
        json!({}),
        json!({"show":true,"state":3,"list":[{"rank":1,"pay_mid":0,"uname":"bad"}]}),
        json!({"show":true,"state":3,"list":[{"rank":1,"pay_mid":123,"uname":"a"},{"rank":2,"pay_mid":123,"uname":"b"}]}),
        json!({"show":true,"state":3,"list":"unavailable"}),
    ] {
        assert!(ElecInfo::parse(&json!({"data":{"elec":{"show_info":bad}}})).is_none());
    }
    assert_eq!(elec_stat::Entity::find().count(&db).await?, 2);
    // 即使写入时失败，成员与快照也必须一起回滚。
    let mut duplicate = info.clone();
    duplicate
        .list
        .as_mut()
        .unwrap()
        .push(info.list.as_ref().unwrap()[0].clone());
    assert!(save_snapshot(42, &duplicate, Utc::now(), &db).await.is_err());
    assert_eq!(elec_stat::Entity::find().count(&db).await?, 2);
    Ok(())
}

#[tokio::test]
async fn interrupted_migration_retries_without_losing_boards() -> Result<()> {
    let db = Database::connect("sqlite::memory:").await?;
    Migrator::up(&db, None).await?;
    let info = ElecInfo {
        show: true,
        state: 3,
        total: Some(14),
        upower_count_show: Some(true),
        list: Some(vec![ElecMember {
            rank: 1,
            pay_mid: 123,
            uname: "test".into(),
            avatar: String::new(),
        }]),
    };
    save_snapshot(42, &info, Utc::now(), &db).await?;
    db.execute_unprepared("DROP INDEX idx_elec_rank_snapshot_member")
        .await?;
    db.execute_unprepared("DELETE FROM seaql_migrations WHERE version = 'm20261008_000003_elec_stat'")
        .await?;
    Migrator::up(&db, None).await?;
    assert_eq!(elec_stat::Entity::find().count(&db).await?, 1);
    assert_eq!(elec_rank::Entity::find().count(&db).await?, 1);
    Migrator::down(&db, Some(1)).await?;
    Migrator::up(&db, Some(1)).await?;
    assert_eq!(elec_stat::Entity::find().count(&db).await?, 0);
    Ok(())
}
