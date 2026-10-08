use super::*;
use crate::elec_stats::{ElecInfo, ElecMember, save_snapshot};
use anyhow::{Result, ensure};
use axum::{body::to_bytes, response::IntoResponse};
use bili_sync_migration::{Migrator, MigratorTrait};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectOptions, Database};
use serde_json::Value;

async fn database() -> Result<DatabaseConnection> {
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1).sqlx_logging(false);
    let db = Database::connect(options).await?;
    Migrator::up(&db, None).await?;
    dynamic_source::ActiveModel {
        id: Set(1),
        upper_id: Set(42),
        upper_name: Set("test".into()),
        path: Set(String::new()),
        created_at: Set(Utc::now().naive_utc()),
        latest_dyn_at: Set(Utc::now().naive_utc()),
        sync_reply: Set(false),
        enabled: Set(false),
    }
    .insert(&db)
    .await?;
    Ok(db)
}

fn member(rank: i64, mid: i64, name: &str) -> ElecMember {
    ElecMember {
        rank,
        pay_mid: mid,
        uname: name.into(),
        avatar: String::new(),
    }
}

async fn data(db: &DatabaseConnection, params: StatsQuery) -> Result<Value> {
    let response = match get_elec_stats(Path(1), Query(params), Extension(db.clone())).await {
        Ok(response) => response.into_response(),
        Err(error) => error.into_response(),
    };
    let status = response.status();
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await?)?;
    ensure!(status.is_success(), "API failed: {body}");
    Ok(body["data"].clone())
}

#[tokio::test]
async fn board_changes_use_uid_and_never_treat_hidden_boards_as_all_members_leaving() -> Result<()> {
    let db = database().await?;
    let now = Utc::now();
    let mut info = ElecInfo {
        show: true,
        state: 3,
        total: Some(14),
        upower_count_show: Some(true),
        list: Some(vec![member(1, 101, "a"), member(2, 102, "b")]),
    };
    for days in [20, 2] {
        save_snapshot(42, &info, now - chrono::Duration::days(days), &db).await?;
    }
    info.total = Some(15);
    info.list = Some(vec![member(1, 102, "renamed b"), member(2, 103, "c")]);
    save_snapshot(42, &info, now - chrono::Duration::days(1), &db).await?;
    let visible = info.clone();
    info.show = false;
    info.list = None;
    save_snapshot(42, &info, now - chrono::Duration::minutes(30), &db).await?;
    save_snapshot(42, &visible, now - chrono::Duration::minutes(10), &db).await?;
    info.show = true;
    info.state = 1;
    save_snapshot(42, &info, now, &db).await?;
    save_snapshot(999, &visible, now, &db).await?;
    let result = data(
        &db,
        StatsQuery {
            days: Some(7),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(result["sampleCount"], 5);
    assert_eq!(result["latest"]["total"], 15);
    assert_eq!(result["latest"]["listedCount"], 0);
    assert_eq!(result["totalGrowth"], 1);
    let history = result["history"].as_array().unwrap();
    assert_eq!(history[0]["left"].as_array().unwrap().len(), 2);
    for i in [1, 2] {
        assert_eq!(history[i]["comparisonAvailable"], false);
        assert!(history[i]["left"].as_array().unwrap().is_empty());
    }
    assert!(history[2]["snapshot"]["listedCount"].is_null());
    assert_eq!(history[3]["entered"][0]["payMid"], 103);
    assert_eq!(history[3]["left"][0]["payMid"], 101);
    assert_eq!(history[3]["rankChanges"][0]["payMid"], 102);
    assert_eq!(history[3]["rankChanges"][0]["fromRank"], 2);
    assert_eq!(history[3]["rankChanges"][0]["toRank"], 1);
    assert!(history[0]["snapshot"]["recordedAt"].as_str().unwrap().ends_with('Z'));
    // 最后一页的比较依据在范围之外，仍保留真实前驱，避免虚构首次进榜。
    let page = data(
        &db,
        StatsQuery {
            days: Some(7),
            page: Some(2),
            page_size: Some(2),
        },
    )
    .await?;
    assert_eq!(page["sampleCount"], 5);
    assert_eq!(page["history"].as_array().unwrap().len(), 1);
    assert_eq!(page["history"][0]["comparisonAvailable"], true);
    assert!(page["history"][0]["entered"].as_array().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn empty_and_single_sample_ranges_do_not_fabricate_counts_or_growth() -> Result<()> {
    let db = database().await?;
    let empty = data(&db, StatsQuery::default()).await?;
    assert!(empty["latest"].is_null());
    assert!(empty["totalGrowth"].is_null());
    let info = ElecInfo {
        show: true,
        state: 3,
        total: None,
        upower_count_show: Some(false),
        list: None,
    };
    save_snapshot(42, &info, Utc::now(), &db).await?;
    let single = data(&db, StatsQuery::default()).await?;
    assert_eq!(single["sampleCount"], 1);
    assert!(single["latest"]["total"].is_null());
    assert!(single["latest"]["listedCount"].is_null());
    assert!(single["totalGrowth"].is_null());
    assert_eq!(single["history"][0]["comparisonAvailable"], false);
    assert!(
        get_elec_stats(
            Path(1),
            Query(StatsQuery {
                days: Some(0),
                ..Default::default()
            }),
            Extension(db)
        )
        .await
        .is_err()
    );
    Ok(())
}
