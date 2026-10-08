use std::{collections::HashSet, sync::LazyLock, time::Duration};

use anyhow::Result;
use bili_sync_entity::{elec_rank, elec_stat};
use chrono::{DateTime, Utc};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait, TransactionTrait};
use serde::Deserialize;
use serde_json::Value;

use crate::bilibili::{BiliError, UpperInfo};

static SAMPLE_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

#[derive(Clone, Debug, Deserialize)]
pub struct ElecMember {
    pub rank: i64,
    pub pay_mid: i64,
    pub uname: String,
    #[serde(default)]
    pub avatar: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ElecInfo {
    pub show: bool,
    pub state: i64,
    #[serde(default)]
    pub total: Option<i64>,
    #[serde(default)]
    pub upower_count_show: Option<bool>,
    #[serde(default)]
    pub list: Option<Vec<ElecMember>>,
}

impl ElecInfo {
    pub fn parse(response: &Value) -> Option<Self> {
        let mut info: Self =
            serde_json::from_value(response.get("data")?.get("elec")?.get("show_info")?.clone()).ok()?;
        info.total = info.total.filter(|v| *v >= 0);
        let mut seen = HashSet::new();
        let mut ranks = HashSet::new();
        if let Some(list) = &mut info.list {
            if list
                .iter()
                .any(|m| m.pay_mid <= 0 || m.rank <= 0 || !seen.insert(m.pay_mid) || !ranks.insert(m.rank))
            {
                return None;
            }
            list.sort_by_key(|m| m.rank);
        }
        Some(info)
    }

    pub fn list_available(&self) -> bool {
        // 关闭/隐藏榜单无法观察成员变化；开放但明确无榜时可确认空榜。
        self.show && self.state != -1 && (self.list.is_some() || self.state == 1)
    }
}

pub async fn sample_elec(upper_id: i64, upper: &UpperInfo<'_>, db: &DatabaseConnection) -> Result<()> {
    let _guard = SAMPLE_LOCK.lock().await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    match upper.get_elec_info().await {
        Ok(response) => {
            if let Some(info) = ElecInfo::parse(&response) {
                save_snapshot(upper_id, &info, Utc::now(), db).await?;
            } else {
                warn!("账号 {} 未返回有效充电展示信息，保留历史记录", upper_id);
            }
        }
        Err(e) if matches!(e.downcast_ref::<BiliError>(), Some(inner) if inner.is_risk_control_related()) => {
            return Err(e);
        }
        Err(e) => warn!("采样账号 {} 充电榜失败，保留历史记录：{:#}", upper_id, e),
    }
    Ok(())
}

pub async fn save_snapshot(upper_id: i64, info: &ElecInfo, at: DateTime<Utc>, db: &DatabaseConnection) -> Result<()> {
    let txn = db.begin().await?;
    let snapshot = elec_stat::ActiveModel {
        upper_id: Set(upper_id),
        show: Set(info.show),
        state: Set(info.state),
        total: Set(info.total),
        upower_count_show: Set(info.upower_count_show),
        list_available: Set(info.list_available()),
        recorded_at: Set(at.naive_utc()),
        ..Default::default()
    }
    .insert(&txn)
    .await?;
    if info.list_available()
        && let Some(list) = &info.list
        && !list.is_empty()
    {
        elec_rank::Entity::insert_many(list.iter().map(|m| elec_rank::ActiveModel {
            snapshot_id: Set(snapshot.id),
            rank: Set(m.rank),
            pay_mid: Set(m.pay_mid),
            uname: Set(m.uname.clone()),
            avatar: Set(m.avatar.clone()),
            ..Default::default()
        }))
        .exec(&txn)
        .await?;
    }
    txn.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests;
