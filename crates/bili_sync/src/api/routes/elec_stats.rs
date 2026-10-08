use std::collections::HashMap;

use axum::{
    Extension, Router,
    extract::{Path, Query},
    routing::get,
};
use bili_sync_entity::{dynamic_source, elec_rank, elec_stat};
use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Statement,
};
use serde::Serialize;

use super::video_stats::StatsQuery;
use crate::api::{
    error::InnerApiError,
    wrapper::{ApiError, ApiResponse},
};

pub(super) fn router() -> Router {
    Router::new().route("/dynamic-sources/{id}/elec-stats", get(get_elec_stats))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub rank: i64,
    pub pay_mid: i64,
    pub uname: String,
    pub avatar: String,
}

impl From<elec_rank::Model> for Member {
    fn from(m: elec_rank::Model) -> Self {
        Self {
            rank: m.rank,
            pay_mid: m.pay_mid,
            uname: m.uname,
            avatar: m.avatar,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    id: i64,
    recorded_at: DateTime<Utc>,
    show: bool,
    state: i64,
    total: Option<i64>,
    upower_count_show: Option<bool>,
    list_available: bool,
    listed_count: Option<i64>,
}

impl Point {
    fn new(s: &elec_stat::Model, count: i64) -> Self {
        Self {
            id: s.id,
            recorded_at: s.recorded_at.and_utc(),
            show: s.show,
            state: s.state,
            total: s.total,
            upower_count_show: s.upower_count_show,
            list_available: s.list_available,
            listed_count: s.list_available.then_some(count),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    #[serde(flatten)]
    point: Point,
    members: Vec<Member>,
}

fn snapshot(s: &elec_stat::Model, members: &HashMap<i64, Vec<Member>>) -> Snapshot {
    let members = members.get(&s.id).cloned().unwrap_or_default();
    Snapshot {
        point: Point::new(s, members.len() as i64),
        members,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankChange {
    pay_mid: i64,
    uname: String,
    from_rank: i64,
    to_rank: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    snapshot: Snapshot,
    previous_at: Option<DateTime<Utc>>,
    comparison_available: bool,
    entered: Vec<Member>,
    left: Vec<Member>,
    rank_changes: Vec<RankChange>,
}

fn history(
    current: &elec_stat::Model,
    previous: Option<&elec_stat::Model>,
    members: &HashMap<i64, Vec<Member>>,
) -> History {
    let current_snapshot = snapshot(current, members);
    let comparison_available = current.list_available && previous.is_some_and(|p| p.list_available);
    let mut entered = Vec::new();
    let mut left = Vec::new();
    let mut rank_changes = Vec::new();
    if comparison_available {
        let before = members
            .get(&previous.unwrap().id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for m in &current_snapshot.members {
            match before.iter().find(|p| p.pay_mid == m.pay_mid) {
                None => entered.push(m.clone()),
                Some(p) if p.rank != m.rank => rank_changes.push(RankChange {
                    pay_mid: m.pay_mid,
                    uname: m.uname.clone(),
                    from_rank: p.rank,
                    to_rank: m.rank,
                }),
                _ => {}
            }
        }
        left.extend(
            before
                .iter()
                .filter(|m| !current_snapshot.members.iter().any(|p| p.pay_mid == m.pay_mid))
                .cloned(),
        );
    }
    History {
        snapshot: current_snapshot,
        previous_at: previous.map(|p| p.recorded_at.and_utc()),
        comparison_available,
        entered,
        left,
        rank_changes,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElecStatsResponse {
    latest: Option<Snapshot>,
    points: Vec<Point>,
    sample_count: u64,
    baseline_at: Option<DateTime<Utc>>,
    total_growth: Option<i64>,
    history: Vec<History>,
}

pub async fn get_elec_stats(
    Path(id): Path<i32>,
    Query(params): Query<StatsQuery>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<ElecStatsResponse>, ApiError> {
    let cutoff = params.cutoff()?;
    let source = dynamic_source::Entity::find_by_id(id)
        .one(&db)
        .await?
        .ok_or(InnerApiError::NotFound(id))?;
    let all = elec_stat::Entity::find().filter(elec_stat::Column::UpperId.eq(source.upper_id));
    let latest = all
        .clone()
        .order_by_desc(elec_stat::Column::RecordedAt)
        .order_by_desc(elec_stat::Column::Id)
        .one(&db)
        .await?;
    let mut window = all.clone();
    if let Some(cutoff) = cutoff {
        window = window.filter(elec_stat::Column::RecordedAt.gte(cutoff));
    }
    let sample_count = window.clone().count(&db).await?;
    let baseline = window
        .clone()
        .order_by_asc(elec_stat::Column::RecordedAt)
        .order_by_asc(elec_stat::Column::Id)
        .one(&db)
        .await?;
    let total_growth = if sample_count > 1 {
        latest.as_ref().zip(baseline.as_ref()).and_then(|(last, first)| {
            if last.upower_count_show == Some(false) || first.upower_count_show == Some(false) {
                return None;
            }
            last.total.zip(first.total).map(|(l, f)| l - f)
        })
    } else {
        None
    };
    let size = params.page_size.unwrap_or(20).clamp(1, 100);
    let offset = params.page.unwrap_or(0).saturating_mul(size).min(i64::MAX as u64);
    let records = window
        .order_by_desc(elec_stat::Column::RecordedAt)
        .order_by_desc(elec_stat::Column::Id)
        .limit(size)
        .offset(offset)
        .all(&db)
        .await?;
    // 分页最后一条也与真正的前一次采样比较，范围外的前驱仍可作为榜单比较依据。
    let predecessor = if let Some(last) = records.last() {
        all.filter(
            Condition::any()
                .add(elec_stat::Column::RecordedAt.lt(last.recorded_at))
                .add(
                    Condition::all()
                        .add(elec_stat::Column::RecordedAt.eq(last.recorded_at))
                        .add(elec_stat::Column::Id.lt(last.id)),
                ),
        )
        .order_by_desc(elec_stat::Column::RecordedAt)
        .order_by_desc(elec_stat::Column::Id)
        .one(&db)
        .await?
    } else {
        None
    };
    let mut ids = records.iter().map(|s| s.id).collect::<Vec<_>>();
    ids.extend(latest.iter().chain(predecessor.iter()).map(|s| s.id));
    let mut members = HashMap::<i64, Vec<Member>>::new();
    if !ids.is_empty() {
        for m in elec_rank::Entity::find()
            .filter(elec_rank::Column::SnapshotId.is_in(ids))
            .order_by_asc(elec_rank::Column::Rank)
            .all(&db)
            .await?
        {
            members.entry(m.snapshot_id).or_default().push(m.into());
        }
    }
    let history = records
        .iter()
        .enumerate()
        .map(|(i, s)| history(s, records.get(i + 1).or(predecessor.as_ref()), &members))
        .collect();
    let stride = (sample_count.saturating_sub(1).saturating_add(998) / 999).max(1);
    let sampled = elec_stat::Entity::find()
        .from_raw_sql(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "WITH ordered AS (SELECT *, ROW_NUMBER() OVER (ORDER BY recorded_at, id) AS rn, COUNT(*) OVER () AS n
        FROM elec_stat WHERE upper_id = ? AND (? IS NULL OR recorded_at >= ?))
        SELECT * FROM ordered WHERE rn = 1 OR rn = n OR (rn - 1) % ? = 0 ORDER BY recorded_at, id",
            [
                source.upper_id.into(),
                cutoff.into(),
                cutoff.into(),
                (stride as i64).into(),
            ],
        ))
        .all(&db)
        .await?;
    #[derive(FromQueryResult)]
    struct Count {
        snapshot_id: i64,
        count: i64,
    }
    let counts = if sampled.is_empty() {
        HashMap::new()
    } else {
        elec_rank::Entity::find()
            .filter(elec_rank::Column::SnapshotId.is_in(sampled.iter().map(|s| s.id)))
            .select_only()
            .column(elec_rank::Column::SnapshotId)
            .column_as(elec_rank::Column::Id.count(), "count")
            .group_by(elec_rank::Column::SnapshotId)
            .into_model::<Count>()
            .all(&db)
            .await?
            .into_iter()
            .map(|c| (c.snapshot_id, c.count))
            .collect()
    };
    Ok(ApiResponse::ok(ElecStatsResponse {
        latest: latest.as_ref().map(|s| snapshot(s, &members)),
        points: sampled
            .iter()
            .map(|s| Point::new(s, counts.get(&s.id).copied().unwrap_or(0)))
            .collect(),
        sample_count,
        baseline_at: baseline.map(|s| s.recorded_at.and_utc()),
        total_growth,
        history,
    }))
}

#[cfg(test)]
mod tests;
