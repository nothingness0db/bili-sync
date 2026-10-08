use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex as StdMutex};
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use bili_sync_entity::{dynamic, dynamic_source, reply, upper_stat};
use futures::{StreamExt, stream};
use sea_orm::ActiveValue::Set;
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::{Condition, OnConflict};
use sea_orm::{ConnectionTrait, QueryOrder, TransactionTrait};
use serde_json::Value;
use tokio::fs;

use crate::bilibili::{
    BiliClient, BiliError, DynamicFeed, DynamicInfo, MIXIN_KEY, Reply, ReplyInfo, ReplySyncState, Submission,
    UpperInfo, VideoInfo,
};
use crate::config::Config;
use crate::downloader::Downloader;
use crate::utils::dynamic_render::{render_comments_md, render_dynamic_md};
use crate::utils::status::STATUS_COMPLETED;

/// 动态源同步的实时进度（供看板展示），任务结束后自动清空
#[derive(Clone, Default)]
pub struct SyncProgress {
    pub source_name: String,
    /// 当前阶段：账号快照 / 扫描动态 / 视频统计 / 评论同步 / 评论续抓
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub eta_seconds: Option<u64>,
}

static SYNC_PROGRESS: LazyLock<parking_lot::RwLock<HashMap<i32, SyncProgress>>> =
    LazyLock::new(|| parking_lot::RwLock::new(HashMap::new()));

/// 读取指定动态源的同步进度（看板轮询用）
pub fn read_sync_progress(source_id: i32) -> SyncProgress {
    SYNC_PROGRESS.read().get(&source_id).cloned().unwrap_or_default()
}

fn set_sync_progress(source_id: i32, progress: SyncProgress) {
    let mut progresses = SYNC_PROGRESS.write();
    if progress.source_name.is_empty() {
        progresses.remove(&source_id);
    } else {
        progresses.insert(source_id, progress);
    }
}

struct SyncProgressGuard {
    source_id: i32,
}

impl Drop for SyncProgressGuard {
    fn drop(&mut self) {
        set_sync_progress(self.source_id, SyncProgress::default());
    }
}

/// 每个动态源的同步锁，防止定时任务与手动同步并发
static SOURCE_LOCKS: LazyLock<StdMutex<HashMap<i32, std::sync::Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

pub(crate) fn get_source_lock(source_id: i32) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    SOURCE_LOCKS
        .lock()
        .unwrap()
        .entry(source_id)
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// 每条动态每轮最多发起的评论请求总数（顶级与楼中楼共用预算）。
const MAX_REPLY_REQUESTS_PER_ROUND: usize = 50;
/// 动态发布后自动同步评论的时间窗口（天）
const REPLY_SYNC_WINDOW_DAYS: i64 = 5;

/// 确保全局 wbi 签名密钥已初始化，未初始化时立即获取（依赖视频任务的全局状态不可靠）
pub async fn ensure_mixin_key(bili_client: &BiliClient, credential: &crate::bilibili::Credential) -> Result<()> {
    if MIXIN_KEY.load().is_none() {
        let mixin_key = bili_client
            .wbi_img(credential)
            .await
            .context("获取 wbi_img 失败")?
            .into_mixin_key()
            .context("解析 mixin key 失败")?;
        crate::bilibili::set_global_mixin_key(mixin_key);
    }
    Ok(())
}

/// 完整地处理某个动态源：刷新动态列表、下载图片、同步评论并导出
/// 定时任务使用：源正在被处理时本轮跳过
pub async fn process_dynamic_source(
    source: dynamic_source::Model,
    bili_client: &BiliClient,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<()> {
    // wbi 签名密钥不依赖视频任务的全局状态，动态同步独立初始化
    ensure_mixin_key(bili_client, &config.credential).await?;
    // 防止同一动态源被并发处理（定时任务 + 立即同步按钮），一轮在跑时本轮跳过
    let lock = get_source_lock(source.id);
    let _guard = match lock.try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            warn!("动态源「{}」已有同步任务在进行中，本轮跳过", source.upper_name);
            return Ok(());
        }
    };
    process_dynamic_source_inner(source, bili_client, connection, config).await
}

/// 完整地处理某个动态源，手动触发专用：等待该源正在进行的同步结束后再执行（排队语义）
pub async fn process_dynamic_source_queued(
    source_id: i32,
    bili_client: &BiliClient,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<()> {
    ensure_mixin_key(bili_client, &config.credential).await?;
    // 手动触发排队：等待该源当前任务结束，不跳过。
    // 必须在拿到锁后重新读取 source，避免路径更新与手动同步之间使用旧快照。
    let lock = get_source_lock(source_id);
    let _guard = lock.lock().await;
    let source = dynamic_source::Entity::find_by_id(source_id)
        .one(connection)
        .await?
        .with_context(|| format!("dynamic source {source_id} no longer exists"))?;
    process_dynamic_source_inner(source, bili_client, connection, config).await
}

async fn process_dynamic_source_inner(
    source: dynamic_source::Model,
    bili_client: &BiliClient,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<()> {
    let _progress_guard = SyncProgressGuard { source_id: source.id };
    fs::create_dir_all(&source.path)
        .await
        .with_context(|| format!("failed to create dynamic source directory {}", source.path))?;
    info!("开始处理动态源「{}」..", source.upper_name);
    set_sync_progress(
        source.id,
        SyncProgress {
            source_name: source.upper_name.clone(),
            phase: "扫描动态".into(),
            ..Default::default()
        },
    );
    // 先落库新 AV 动态，使这一轮的视频统计也包含仅动态发布的新视频。
    refresh_dynamic_source(&source, bili_client, connection, config).await?;
    set_sync_progress(
        source.id,
        SyncProgress {
            source_name: source.upper_name.clone(),
            phase: "账号快照".to_string(),
            ..Default::default()
        },
    );
    // 记录账号信息快照（粉丝/关注/投稿/播放/名字/签名），有变化才插入新记录
    update_upper_stat(&source, bili_client, connection, config).await?;
    set_sync_progress(
        source.id,
        SyncProgress {
            source_name: source.upper_name.clone(),
            phase: "评论同步".to_string(),
            ..Default::default()
        },
    );
    // 评论补拉：只对尚未完整扫描过的历史动态标记重扫，完整或关闭的评论区不反复补拉
    backfill_missing_replies(&source, connection).await?;
    process_unhandled_dynamics(&source, bili_client, connection, config).await?;
    info!("处理动态源「{}」完成", source.upper_name);
    set_sync_progress(source.id, SyncProgress::default());
    Ok(())
}

/// 评论补拉：对已完成的动态，若 API 评论数 > 0 但本地无评论，自动标记重扫
///
/// 解决首次全量导入时 5 天窗口外历史动态评论被永久跳过的问题。
/// 每轮限量检查，标记后由 process_unhandled_dynamics 按慢任务队列消化。
async fn backfill_missing_replies(source: &dynamic_source::Model, connection: &DatabaseConnection) -> Result<()> {
    /// 每轮最多检查的已完成动态数
    const BACKFILL_CHECK_LIMIT: u64 = 50;
    /// 每轮最多标记的待重扫动态数（与每轮处理上限对齐，避免积压无限膨胀）
    const MAX_BACKFILL_MARK_PER_ROUND: usize = 5;
    // 未开启评论同步的源不做补拉标记，避免白标记后又被直接清掉
    if !source.sync_reply {
        return Ok(());
    }
    let mut candidates = dynamic::Entity::find()
        .filter(dynamic::Column::SourceId.eq(source.id))
        .filter(dynamic::Column::Valid.eq(true))
        .filter(dynamic::Column::DownloadStatus.gte(STATUS_COMPLETED))
        .filter(dynamic::Column::RescanReply.eq(false))
        .filter(dynamic::Column::ReplySyncedAt.is_null())
        .order_by_desc(dynamic::Column::PubTs)
        .all(connection)
        .await?;
    candidates.truncate(BACKFILL_CHECK_LIMIT as usize);
    let mut marked = 0;
    for dyn_model in candidates {
        if marked >= MAX_BACKFILL_MARK_PER_ROUND {
            break;
        }
        // API 评论数（来自动态抓取时的 stat 快照）
        let api_count = dyn_model
            .stat
            .as_ref()
            .and_then(|s| s["comment"]["count"].as_i64())
            .unwrap_or(0);
        if api_count <= 0 {
            continue;
        }
        // 本地已同步的评论数
        let local_count = reply::Entity::find()
            .filter(reply::Column::DynamicId.eq(&dyn_model.id))
            .filter(reply::Column::Valid.eq(true))
            .count(connection)
            .await? as i64;
        // 本地评论少于 API 评论数（含部分同步）时补拉
        if local_count >= api_count {
            continue;
        }
        let mut model: dynamic::ActiveModel = dyn_model.into();
        model.rescan_reply = Set(true);
        model.save(connection).await?;
        marked += 1;
    }
    if marked > 0 {
        info!(
            "「{}」评论补拉：标记 {} 条历史动态待重扫评论（每轮限量消化）",
            source.upper_name, marked
        );
    }
    Ok(())
}

/// 采样全部当前投稿及 AV 动态视频，同时确认「仅动态视频」数量。
/// 复用这轮详情请求，不为计数与快照各请求一次；历史动态视频每轮也可持续积累统计。
async fn collect_dynamic_video_count(
    source: &dynamic_source::Model,
    bili_client: &BiliClient,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<i64> {
    let sampling_started_at = chrono::Utc::now();
    let mut dynamic_bvids = std::collections::HashSet::new();
    let dynamics = dynamic::Entity::find()
        .filter(dynamic::Column::SourceId.eq(source.id))
        .filter(dynamic::Column::DynType.eq("DYNAMIC_TYPE_AV"))
        .all(connection)
        .await
        .context("collect av dynamics failed")?;
    for dyn_model in dynamics {
        if let Some(raw) = &dyn_model.raw
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(raw)
            && let Some(bvid) = value
                .get("modules")
                .and_then(|m| m.get("module_dynamic"))
                .and_then(|m| m.get("major"))
                .and_then(|m| m.get("archive"))
                .and_then(|a| a.get("bvid"))
                .and_then(|b| b.as_str())
        {
            dynamic_bvids.insert(bvid.to_string());
        }
    }
    // 拉取当前投稿列表（arc/search 全量），每页保持低频避免触发风控
    let submission = Submission::new(bili_client, source.upper_id.to_string(), &config.credential);
    let mut current_bvids = std::collections::HashSet::new();
    let mut stream = Box::pin(submission.into_video_stream());
    while let Some(res) = stream.next().await {
        let VideoInfo::Submission { bvid, .. } = res? else {
            unreachable!("submission stream should only yield Submission variant")
        };
        current_bvids.insert(bvid);
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    }
    let bvids = dynamic_bvids
        .union(&current_bvids)
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    crate::video_stats::track_source_videos(source.upper_id, &bvids.iter().cloned().collect::<Vec<_>>(), connection)
        .await?;
    info!("开始采样「{}」的 {} 个视频统计..", source.upper_name, bvids.len());
    let mut count = 0i64;
    let mut incomplete_count = false;
    for (idx, bvid) in bvids.iter().enumerate() {
        set_sync_progress(
            source.id,
            SyncProgress {
                source_name: source.upper_name.clone(),
                phase: "视频统计".into(),
                current: idx + 1,
                total: bvids.len(),
                ..Default::default()
            },
        );
        let exists =
            crate::video_stats::sample_video(bvid, sampling_started_at, bili_client, &config.credential, connection)
                .await?;
        if dynamic_bvids.contains(bvid) && !current_bvids.contains(bvid) {
            match exists {
                Some(true) => count += 1,
                None => incomplete_count = true,
                Some(false) => {}
            }
        }
    }
    anyhow::ensure!(!incomplete_count, "部分动态视频状态未能确认，跳过账号计数快照");
    info!("采样「{}」视频统计完成", source.upper_name);
    Ok(count)
}

/// 手动触发扫描账号信息专用：等待该源当前任务结束后再执行（排队语义，不跳过）
pub async fn scan_profile_queued(
    source: dynamic_source::Model,
    bili_client: &BiliClient,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<()> {
    ensure_mixin_key(bili_client, &config.credential).await?;
    let lock = get_source_lock(source.id);
    let _guard = lock.lock().await;
    let _progress_guard = SyncProgressGuard { source_id: source.id };
    update_upper_stat(&source, bili_client, connection, config).await
}

/// 拉取 UP 主账号信息并写入快照表，与最近一条快照对比，有变化才插入
pub async fn update_upper_stat(
    source: &dynamic_source::Model,
    bili_client: &BiliClient,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<()> {
    let upper = UpperInfo::new(bili_client, source.upper_id.to_string(), &config.credential);
    // 独立留存每轮充电观察，不受账号数值是否变化、后续视频采样是否完成影响。
    crate::elec_stats::sample_elec(source.upper_id, &upper, connection).await?;
    let profile = match upper.get_profile().await {
        Ok(profile) => profile,
        Err(e) => {
            // 触发风控时立即向上抛出终止本轮任务，避免继续请求延长封锁
            if let Some(inner) = e.downcast_ref::<BiliError>()
                && inner.is_risk_control_related()
            {
                return Err(e);
            }
            warn!("获取「{}」账号信息失败：{:#}", source.upper_name, e);
            return Ok(());
        }
    };
    let latest = upper_stat::Entity::find()
        .filter(upper_stat::Column::UpperId.eq(source.upper_id))
        .order_by_desc(upper_stat::Column::RecordedAt)
        .one(connection)
        .await?;
    // 图文投稿数（B 站投稿 tab 中图文投稿会自动生成 DRAW 动态，投稿总数 = 视频投稿 + 图文投稿）
    let draw_count = dynamic::Entity::find()
        .filter(dynamic::Column::SourceId.eq(source.id))
        .filter(dynamic::Column::DynType.eq("DYNAMIC_TYPE_DRAW"))
        .count(connection)
        .await? as i64;
    // 动态视频数：动态里的视频（AV 动态）中不属于当前投稿列表的部分（仅动态视频）
    // 需要拉取当前投稿列表对比 + 对差集做详情接口确认（-404 为已删除投稿，不算）
    let dynamic_video_count = match collect_dynamic_video_count(source, bili_client, connection, config).await {
        Ok(count) => count,
        Err(e) => {
            if let Some(inner) = e.downcast_ref::<BiliError>()
                && inner.is_risk_control_related()
            {
                return Err(e);
            }
            warn!("统计「{}」动态视频数失败：{:#}", source.upper_name, e);
            // 统计失败时跳过本轮快照记录，避免写入伪造的 0 污染曲线
            return Ok(());
        }
    };
    let video_count = profile.video_count + draw_count;
    // 总视频数 = 视频投稿 + 仅动态视频（图文投稿不算视频）
    let total_video_count = profile.video_count + dynamic_video_count;
    let changed = match latest {
        Some(s) => {
            s.name != profile.name
                || s.sign != profile.sign
                || s.face != profile.face
                || s.fan_count != profile.fan_count
                || s.follow_count != profile.follow_count
                || s.video_count != video_count
                || s.dynamic_video_count != dynamic_video_count
                || s.total_video_count != total_video_count
                || s.view_count != profile.view_count
                || s.like_count != profile.like_count
        }
        None => true,
    };
    if changed {
        upper_stat::Entity::insert(upper_stat::ActiveModel {
            upper_id: Set(source.upper_id),
            name: Set(profile.name.clone()),
            sign: Set(profile.sign.clone()),
            face: Set(profile.face.clone()),
            fan_count: Set(profile.fan_count),
            follow_count: Set(profile.follow_count),
            video_count: Set(video_count),
            dynamic_video_count: Set(dynamic_video_count),
            total_video_count: Set(total_video_count),
            view_count: Set(profile.view_count),
            like_count: Set(profile.like_count),
            recorded_at: Set(chrono::Utc::now().naive_utc()),
            ..Default::default()
        })
        .exec(connection)
        .await?;
        info!(
            "「{}」账号信息更新：粉丝 {} 关注 {} 投稿 {}（视频 {} + 图文 {}）动态视频 {} 总视频 {} 播放 {} 获赞 {}",
            source.upper_name,
            profile.fan_count,
            profile.follow_count,
            video_count,
            profile.video_count,
            draw_count,
            dynamic_video_count,
            total_video_count,
            profile.view_count,
            profile.like_count
        );
    } else {
        info!(
            "「{}」账号信息无变化（粉丝 {} 关注 {} 投稿 {} 动态视频 {} 总视频 {} 播放 {} 获赞 {}），跳过记录",
            source.upper_name,
            profile.fan_count,
            profile.follow_count,
            video_count,
            dynamic_video_count,
            total_video_count,
            profile.view_count,
            profile.like_count
        );
    }
    // 名字变化时同步更新动态源名称
    if source.upper_name != profile.name {
        let new_name = profile.name.clone();
        let mut model: dynamic_source::ActiveModel = source.clone().into();
        model.upper_name = Set(profile.name);
        model.update(connection).await?;
        info!("「{}」改名为「{}」，已更新动态源名称", source.upper_name, new_name);
    }
    Ok(())
}

/// 请求接口，获取动态源下所有新动态，写入数据库
async fn refresh_dynamic_source(
    source: &dynamic_source::Model,
    bili_client: &BiliClient,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<()> {
    info!("开始扫描「{}」的动态..", source.upper_name);
    // 该源还没有任何动态记录时（首次全量导入），忽略记录时间，避免新源只拉到置顶的一条
    let has_dynamics = dynamic::Entity::find()
        .filter(dynamic::Column::SourceId.eq(source.id))
        .count(connection)
        .await?
        > 0;
    let latest_row_at = if has_dynamics {
        source.latest_dyn_at.and_utc()
    } else {
        chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap_or_default()
    };
    let mut max_datetime = latest_row_at;
    let mut count = 0;
    let mut error = Ok(());
    let feed = DynamicFeed::new(bili_client, source.upper_id.to_string(), &config.credential);
    let mut stream = Box::pin(feed.into_dynamic_stream()).enumerate();
    while let Some((idx, res)) = stream.next().await {
        let info = match res {
            Err(e) => {
                if let Some(inner) = e.downcast_ref::<BiliError>() {
                    error = Err(inner.clone()).context(e.to_string());
                } else {
                    error = Err(anyhow!("{:#}", e));
                }
                break;
            }
            Ok(info) => info,
        };
        if info.pub_ts > max_datetime {
            max_datetime = info.pub_ts;
        }
        // 动态按时间倒序返回，遇到比记录时间更早的动态即可停止
        // 第一条可能是置顶的旧动态，单独跳过该限制
        if idx > 0 && info.pub_ts <= latest_row_at {
            break;
        }
        create_dynamic(&info, source.id, connection).await?;
        count += 1;
    }
    error?;
    if max_datetime != latest_row_at {
        let mut model: dynamic_source::ActiveModel = source.clone().into();
        model.latest_dyn_at = Set(max_datetime.naive_utc());
        model.update(connection).await?;
    }
    info!("扫描「{}」动态完成，获取到 {} 条新动态", source.upper_name, count);
    Ok(())
}

/// 尝试创建 Dynamic Model，如果发生冲突则忽略
async fn create_dynamic(info: &DynamicInfo, source_id: i32, connection: &DatabaseConnection) -> Result<()> {
    let model = dynamic::ActiveModel {
        id: Set(info.id.clone()),
        video_bvid: Set(info.raw["modules"]["module_dynamic"]["major"]["archive"]["bvid"]
            .as_str()
            .map(str::to_owned)),
        source_id: Set(source_id),
        dyn_type: Set(info.dyn_type.clone()),
        content: Set(info.content.clone()),
        pics: Set(Some(serde_json::to_value(&info.pics)?)),
        stat: Set(Some(info.stat.clone())),
        pub_ts: Set(info.pub_ts.naive_utc()),
        comment_type: Set(info.comment_type),
        comment_oid: Set(info.comment_oid.clone()),
        location: Set(info.location.clone()),
        raw: Set(Some(info.raw.to_string())),
        download_status: Set(0),
        path: Set(String::new()),
        valid: Set(true),
        rescan_reply: Set(false),
        ..Default::default()
    };
    dynamic::Entity::insert(model)
        .on_conflict(OnConflict::new().do_nothing().to_owned())
        .do_nothing()
        .exec(connection)
        .await?;
    Ok(())
}

/// 处理动态源下所有未完成的动态
///
/// 为避免触发风控，每轮任务只处理有限数量的动态：
/// - 被标记重扫评论的动态优先处理（慢任务，每轮最多 `MAX_RESCAN_PER_ROUND` 条）
/// - 其余新动态每轮最多 `MAX_NEW_PER_ROUND` 条
async fn process_unhandled_dynamics(
    source: &dynamic_source::Model,
    bili_client: &BiliClient,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<()> {
    /// 每轮最多处理的重扫评论动态数
    const MAX_RESCAN_PER_ROUND: usize = 5;
    /// 每轮最多处理的新动态数
    const MAX_NEW_PER_ROUND: usize = 20;
    let dynamics = dynamic::Entity::find()
        .filter(dynamic::Column::SourceId.eq(source.id))
        .filter(dynamic::Column::Valid.eq(true))
        // 未完成的动态 + 被标记重扫评论的已完成动态（backfill 标记的也在此列）
        .filter(
            Condition::any()
                .add(dynamic::Column::DownloadStatus.lt(STATUS_COMPLETED))
                .add(dynamic::Column::RescanReply.eq(true)),
        )
        .order_by_desc(dynamic::Column::PubTs)
        .all(connection)
        .await
        .context("filter unhandled dynamics failed")?;
    if dynamics.is_empty() {
        return Ok(());
    }
    let (dynamics, remaining_rescan, remaining_new) =
        select_dynamic_batch(dynamics, MAX_RESCAN_PER_ROUND, MAX_NEW_PER_ROUND);
    info!(
        "开始处理「{}」的 {} 条动态（本轮含重扫 {} 条，剩余待重扫 {} 条，剩余新动态 {} 条）..",
        source.upper_name,
        dynamics.len(),
        dynamics.iter().filter(|d| d.rescan_reply).count(),
        remaining_rescan,
        remaining_new
    );
    let downloader = Downloader::new(bili_client.client.clone());
    let reply_api = Reply::new(bili_client, &config.credential);
    let total = dynamics.len();
    // 每条动态处理耗时的滑动平均（秒），用于估算剩余时间
    let mut avg_item_secs: Option<f64> = None;
    for (idx, dyn_model) in dynamics.into_iter().enumerate() {
        let item_start = Instant::now();
        let dyn_id = dyn_model.id.clone();
        let eta_seconds = avg_item_secs.map(|avg| ((total - idx - 1) as f64 * avg).ceil() as u64);
        set_sync_progress(
            source.id,
            SyncProgress {
                source_name: source.upper_name.clone(),
                phase: if dyn_model.reply_sync_state.is_some() {
                    "评论续抓"
                } else {
                    "评论同步"
                }
                .to_string(),
                current: idx + 1,
                total,
                eta_seconds,
            },
        );
        info!(
            "开始处理「{}」第 {}/{} 条动态 {}..",
            source.upper_name,
            idx + 1,
            total,
            dyn_id
        );
        dynamic::Entity::update_many()
            .filter(dynamic::Column::Id.eq(&dyn_id))
            .col_expr(
                dynamic::Column::ReplyLastAttemptAt,
                Expr::value(chrono::Utc::now().naive_utc()),
            )
            .exec(connection)
            .await?;
        if let Err(e) = process_dynamic(source, dyn_model, &downloader, &reply_api, connection, config).await {
            error!("处理动态 {dyn_id} 失败：{:#}", e);
            if let Ok(e) = e.downcast::<BiliError>()
                && e.is_risk_control_related()
            {
                bail!(e);
            }
        }
        let elapsed = item_start.elapsed().as_secs_f64();
        avg_item_secs = Some(match avg_item_secs {
            Some(prev) => prev * 0.7 + elapsed * 0.3,
            None => elapsed,
        });
    }
    Ok(())
}

/// 未尝试过的任务优先；尝试失败或达到预算的任务轮到队尾，避免最新五条长期挡住积压。
fn select_dynamic_batch(
    dynamics: Vec<dynamic::Model>,
    max_rescan: usize,
    max_new: usize,
) -> (Vec<dynamic::Model>, usize, usize) {
    let (mut rescan, new): (Vec<_>, Vec<_>) = dynamics.into_iter().partition(|d| d.rescan_reply);
    rescan.sort_by(|a, b| {
        a.reply_last_attempt_at
            .cmp(&b.reply_last_attempt_at)
            .then_with(|| b.pub_ts.cmp(&a.pub_ts))
            .then_with(|| a.id.cmp(&b.id))
    });
    let remaining_rescan = rescan.len().saturating_sub(max_rescan);
    let remaining_new = new.len().saturating_sub(max_new);
    let mut selected: Vec<_> = rescan.into_iter().take(max_rescan).collect();
    selected.extend(new.into_iter().take(max_new));
    (selected, remaining_rescan, remaining_new)
}

/// 评论和游标必须原子落库，避免进程重启后跳过未保存的页面。
async fn checkpoint_reply_page(
    dynamic_id: &str,
    replies: &[ReplyInfo],
    state: &ReplySyncState,
    connection: &DatabaseConnection,
) -> Result<()> {
    let txn = connection.begin().await?;
    save_replies(dynamic_id, replies, &txn).await?;
    let result = dynamic::Entity::update_many()
        .filter(dynamic::Column::Id.eq(dynamic_id))
        .col_expr(
            dynamic::Column::ReplySyncState,
            Expr::value(serde_json::to_value(state)?),
        )
        .col_expr(dynamic::Column::RescanReply, Expr::value(true))
        .exec(&txn)
        .await?;
    anyhow::ensure!(
        result.rows_affected == 1,
        "dynamic {dynamic_id} disappeared while saving reply checkpoint"
    );
    txn.commit().await?;
    Ok(())
}

async fn reconcile_replies(dynamic_id: &str, seen_ids: &[i64], connection: &DatabaseConnection) -> Result<()> {
    let txn = connection.begin().await?;
    reply::Entity::update_many()
        .filter(reply::Column::DynamicId.eq(dynamic_id))
        .col_expr(reply::Column::Valid, Expr::value(false))
        .exec(&txn)
        .await?;
    // 控制绑定参数数量；热门评论区的完整扫描可能超过 SQLite 的变量上限。
    for ids in seen_ids.chunks(500) {
        reply::Entity::update_many()
            .filter(reply::Column::DynamicId.eq(dynamic_id))
            .filter(reply::Column::Rpid.is_in(ids.iter().copied()))
            .col_expr(reply::Column::Valid, Expr::value(true))
            .exec(&txn)
            .await?;
    }
    txn.commit().await?;
    Ok(())
}

/// 处理单条动态：导出 JSON/Markdown，下载图片，同步评论
async fn process_dynamic(
    source: &dynamic_source::Model,
    dyn_model: dynamic::Model,
    downloader: &Downloader,
    reply_api: &Reply<'_>,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<()> {
    // 目录格式 {path}/{YYYY-MM-DD} {dyn_id}
    let dir = PathBuf::from(&source.path).join(format!("{} {}", dyn_model.pub_ts.format("%Y-%m-%d"), dyn_model.id));
    fs::create_dir_all(&dir).await?;
    // 原始 JSON
    if let Some(raw) = &dyn_model.raw {
        fs::write(dir.join("dynamic.json"), raw).await?;
    }
    // 正文 markdown
    let pics: Vec<String> = match &dyn_model.pics {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => Vec::new(),
    };
    let info = DynamicInfo {
        id: dyn_model.id.clone(),
        dyn_type: dyn_model.dyn_type.clone(),
        content: dyn_model.content.clone(),
        pics,
        stat: dyn_model.stat.clone().unwrap_or(Value::Null),
        pub_ts: dyn_model.pub_ts.and_utc(),
        comment_type: 0,
        comment_oid: String::new(),
        location: dyn_model.location.clone(),
        raw: Value::Null,
    };
    fs::write(dir.join("content.md"), render_dynamic_md(&info, &source.upper_name)).await?;
    // 下载正文图片（重扫评论时本体已存在，跳过重下）
    if !dyn_model.rescan_reply {
        let pics_dir = dir.join("pics");
        for (i, url) in info.pics.iter().enumerate() {
            downloader
                .fetch(
                    url,
                    &pics_dir.join(format!("{:0>2}.jpg", i + 1)),
                    &config.concurrent_limit.download,
                )
                .await?;
        }
    }
    // 同步评论：发布 5 天内的动态每轮自动同步，5 天外的仅在被手动标记重扫时同步
    let within_window =
        dyn_model.pub_ts.and_utc() + chrono::Duration::days(REPLY_SYNC_WINDOW_DAYS) >= chrono::Utc::now();
    let need_rescan = dyn_model.rescan_reply;
    let mut synced_replies = false;
    let mut replies_incomplete = false;
    if source.sync_reply && (within_window || need_rescan) {
        info!(
            "动态 {} 本体已处理，评论待扫描{}，开始同步评论..",
            dyn_model.id,
            if need_rescan {
                "（重扫）"
            } else {
                "（5 天窗口内）"
            }
        );
        let reply_sync_result = match sync_dynamic_replies(
            &dyn_model,
            dyn_model.comment_type,
            &dyn_model.comment_oid,
            &dir.join("comments"),
            downloader,
            reply_api,
            connection,
            config,
        )
        .await
        {
            Ok(result) => result,
            Err(e) => {
                // 动态已被删除（-404）：标记无效，停止重试，不影响本体已存档的数据
                if let Some(BiliError::ErrorResponse { code: -404, .. }) = e.downcast_ref::<BiliError>() {
                    warn!(
                        "动态 {} 已不存在（可能被 UP 删除），标记为无效，保留本地档案",
                        dyn_model.id
                    );
                    let mut model: dynamic::ActiveModel = dyn_model.clone().into();
                    model.valid = Set(false);
                    model.rescan_reply = Set(false);
                    model.reply_sync_state = Set(None);
                    model.download_status = Set(STATUS_COMPLETED);
                    model.save(connection).await?;
                    return Ok(());
                } else if let Some(BiliError::ErrorResponse { code: 12002, .. }) = e.downcast_ref::<BiliError>() {
                    // 评论功能已关闭（12002）：动态仍在但没有评论，按正常完成处理，不再重试
                    warn!("动态 {} 评论功能已关闭（12002），按无评论完成", dyn_model.id);
                    let mut model: dynamic::ActiveModel = dyn_model.clone().into();
                    model.download_status = Set(STATUS_COMPLETED);
                    model.rescan_reply = Set(false);
                    model.path = Set(dir.to_string_lossy().to_string());
                    model.reply_sync_state = Set(None);
                    model.reply_synced_at = Set(Some(chrono::Utc::now().naive_utc()));
                    model.save(connection).await?;
                    return Ok(());
                } else {
                    return Err(e);
                }
            }
        };
        synced_replies = matches!(reply_sync_result, ReplySyncResult::Complete | ReplySyncResult::Skipped);
        replies_incomplete = matches!(reply_sync_result, ReplySyncResult::Incomplete);
        match reply_sync_result {
            ReplySyncResult::Complete => info!("动态 {} 评论同步完整完成", dyn_model.id),
            ReplySyncResult::Incomplete => info!("动态 {} 评论本轮部分同步，已保存断点，下轮继续", dyn_model.id),
            ReplySyncResult::Skipped => info!("动态 {} 评论同步已跳过（缺少评论对象信息）", dyn_model.id),
        }
    }
    // 这里只标记动态本体完成；评论是否完整结束由独立状态表示。
    let dyn_id = dyn_model.id.clone();
    let mut model: dynamic::ActiveModel = dyn_model.into();
    model.download_status = Set(STATUS_COMPLETED);
    model.path = Set(dir.to_string_lossy().to_string());
    model.rescan_reply = Set(replies_incomplete);
    if synced_replies {
        model.reply_sync_state = Set(None);
        model.reply_synced_at = Set(Some(chrono::Utc::now().naive_utc()));
    }
    model.save(connection).await?;
    if replies_incomplete {
        info!("动态 {dyn_id} 本体处理完成，评论仍待续抓");
    } else {
        info!("处理动态 {dyn_id} 完成");
    }
    Ok(())
}

#[derive(Debug)]
enum ReplySyncResult {
    Skipped,
    Complete,
    Incomplete,
}

/// 拉取动态的评论：存库、导出 JSON/Markdown、下载评论图片
#[allow(clippy::too_many_arguments)]
async fn sync_dynamic_replies(
    dyn_model: &dynamic::Model,
    comment_type: i64,
    comment_oid: &str,
    comments_dir: &PathBuf,
    downloader: &Downloader,
    reply_api: &Reply<'_>,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<ReplySyncResult> {
    let dynamic_id = &dyn_model.id;
    if comment_type <= 0 || comment_oid.is_empty() {
        warn!("动态 {dynamic_id} 缺少评论信息（comment_type={comment_type}），跳过评论同步");
        return Ok(ReplySyncResult::Skipped);
    }
    let mut state: ReplySyncState = dyn_model
        .reply_sync_state
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .context("invalid saved reply pagination state")?
        .unwrap_or_default();
    if state.restart_legacy_pagination() {
        info!("动态 {dynamic_id} 的旧分页游标需要重新初始化，保留本地历史评论，开始新的扫描周期");
    }
    for _ in 0..MAX_REPLY_REQUESTS_PER_ROUND {
        if state.is_complete() {
            break;
        }
        let (replies, next_state) = reply_api
            .get_next_page(comment_type, comment_oid, &state)
            .await
            .with_context(|| format!("failed to get replies of dynamic {dynamic_id}"))?;
        checkpoint_reply_page(dynamic_id, &replies, &next_state, connection).await?;
        state = next_state;
    }
    let replies_complete = state.is_complete();
    if replies_complete {
        // 只有整个扫描周期（包括此前各轮的页面）完整结束，才能标记失效。
        reconcile_replies(dynamic_id, &state.seen_ids(), connection).await?;
    } else {
        info!(
            "动态 {dynamic_id} 评论达到本轮 {MAX_REPLY_REQUESTS_PER_ROUND} 次请求预算，{}",
            state.progress_description()
        );
    }
    // 从本地数据库导出完整历史，而不是用 B 站本轮返回结果覆盖历史评论。
    let local_replies = load_local_replies(dynamic_id, connection).await?;
    // 导出 JSON / Markdown
    fs::create_dir_all(comments_dir).await?;
    fs::write(
        comments_dir.join("comments.json"),
        serde_json::to_string_pretty(&local_replies)?,
    )
    .await?;
    fs::write(comments_dir.join("comments.md"), render_comments_md(&local_replies)).await?;
    // 从落库后的有效评论补齐缺失图片，断点推进后图片下载失败也可以继续重试。
    let image_tasks = local_replies
        .iter()
        .filter(|reply| reply.valid)
        .flat_map(|reply| {
            let mut tasks = Vec::new();
            for (i, url) in reply.images.iter().enumerate() {
                tasks.push((url.clone(), comments_dir.join(format!("{}_{}.jpg", reply.rpid, i + 1))));
            }
            for sub in reply.sub_replies.iter().filter(|sub| sub.valid) {
                for (i, url) in sub.images.iter().enumerate() {
                    tasks.push((url.clone(), comments_dir.join(format!("{}_{}.jpg", sub.rpid, i + 1))));
                }
            }
            tasks
        })
        .collect::<Vec<_>>();
    let concurrency = config.concurrent_limit.download.concurrency.max(1);
    let mut image_stream = stream::iter(image_tasks)
        .map(|(url, path)| async move {
            if fs::try_exists(&path).await? {
                return Ok(());
            }
            downloader.fetch(&url, &path, &config.concurrent_limit.download).await
        })
        .buffer_unordered(concurrency);
    while let Some(res) = image_stream.next().await {
        res?;
    }
    Ok(if replies_complete {
        ReplySyncResult::Complete
    } else {
        ReplySyncResult::Incomplete
    })
}

/// 从本地数据库读取某条动态的全部评论历史，并重建评论树。
/// `valid` 只作为展示状态，不作为过滤条件。
async fn load_local_replies(dynamic_id: &str, connection: &DatabaseConnection) -> Result<Vec<ReplyInfo>> {
    let rows = reply::Entity::find()
        .filter(reply::Column::DynamicId.eq(dynamic_id))
        .order_by_asc(reply::Column::Ctime)
        .all(connection)
        .await?;

    let mut replies = HashMap::with_capacity(rows.len());
    let mut order = Vec::with_capacity(rows.len());
    for row in rows {
        let rpid = row.rpid;
        order.push((rpid, row.parent_rpid));
        let images = row
            .images
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        let raw = row
            .raw
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok())
            .unwrap_or(Value::Null);
        replies.insert(
            rpid,
            ReplyInfo {
                rpid,
                parent_rpid: row.parent_rpid,
                uname: row.uname,
                avatar: row.avatar,
                content: row.content,
                images,
                ctime: row.ctime.and_utc(),
                valid: row.valid,
                raw,
                sub_replies: Vec::new(),
            },
        );
    }

    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut top_level = Vec::new();
    for (rpid, parent_rpid) in order {
        if let Some(parent_rpid) = parent_rpid
            && replies.contains_key(&parent_rpid)
        {
            children.entry(parent_rpid).or_default().push(rpid);
        } else {
            top_level.push(rpid);
        }
    }

    fn attach(
        rpid: i64,
        replies: &mut HashMap<i64, ReplyInfo>,
        children: &HashMap<i64, Vec<i64>>,
    ) -> Option<ReplyInfo> {
        let mut reply = replies.remove(&rpid)?;
        if let Some(child_ids) = children.get(&rpid) {
            reply.sub_replies = child_ids
                .iter()
                .filter_map(|child_id| attach(*child_id, replies, children))
                .collect();
        }
        Some(reply)
    }

    Ok(top_level
        .into_iter()
        .filter_map(|rpid| attach(rpid, &mut replies, &children))
        .collect())
}

/// 将评论（含楼中楼）写入数据库
async fn save_replies(dynamic_id: &str, replies: &[ReplyInfo], connection: &impl ConnectionTrait) -> Result<()> {
    let mut models = Vec::with_capacity(replies.len() * 2);
    for reply in replies {
        models.push(reply_to_model(dynamic_id, reply));
        for sub in &reply.sub_replies {
            models.push(reply_to_model(dynamic_id, sub));
        }
    }
    if models.is_empty() {
        return Ok(());
    }
    // 分批插入：reply 表列数较多，超过 SQLite 绑定变量上限（999）会报
    // too many SQL variables，热门动态评论（含楼中楼）可达数千条，每批 80 条留足余量
    for chunk in models.chunks(80) {
        reply::Entity::insert_many(chunk.to_vec())
            .on_conflict(
                OnConflict::column(reply::Column::Rpid)
                    .update_columns([
                        reply::Column::ParentRpid,
                        reply::Column::Uname,
                        reply::Column::Avatar,
                        reply::Column::Content,
                        reply::Column::Images,
                        reply::Column::Ctime,
                        reply::Column::Raw,
                        reply::Column::Valid,
                    ])
                    .to_owned(),
            )
            .exec(connection)
            .await?;
    }
    Ok(())
}

fn reply_to_model(dynamic_id: &str, reply: &ReplyInfo) -> reply::ActiveModel {
    reply::ActiveModel {
        rpid: Set(reply.rpid),
        dynamic_id: Set(dynamic_id.to_string()),
        parent_rpid: Set(reply.parent_rpid),
        uname: Set(reply.uname.clone()),
        avatar: Set(reply.avatar.clone()),
        content: Set(reply.content.clone()),
        images: Set(if reply.images.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&reply.images).unwrap_or(Value::Null))
        }),
        ctime: Set(reply.ctime.naive_utc()),
        raw: Set(Some(reply.raw.to_string())),
        download_status: Set(0),
        valid: Set(true),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;

    use axum::extract::RawQuery;
    use axum::routing::get;
    use axum::{Json, Router};
    use bili_sync_migration::{Migrator, MigratorTrait};
    use sea_orm::{ConnectOptions, Database};
    use serde_json::json;

    use super::*;
    use crate::bilibili::Credential;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("bili-sync-reply-test-{}", uuid::Uuid::new_v4())))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn comment(id: i64) -> ReplyInfo {
        ReplyInfo {
            rpid: id,
            parent_rpid: None,
            uname: "test".into(),
            avatar: String::new(),
            content: "test".into(),
            images: Vec::new(),
            ctime: chrono::Utc::now(),
            valid: true,
            raw: json!({}),
            sub_replies: Vec::new(),
        }
    }

    async fn test_database() -> Result<DatabaseConnection> {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let db = Database::connect(options).await?;
        Migrator::up(&db, None).await?;
        Ok(db)
    }

    async fn insert_dynamic(db: &DatabaseConnection, id: &str) -> Result<dynamic::Model> {
        let model = dynamic::Model {
            id: id.into(),
            source_id: 1,
            comment_type: 1,
            comment_oid: "123".into(),
            stat: Some(json!({"comment": {"count": 100}})),
            pub_ts: (chrono::Utc::now() - chrono::Duration::days(10)).naive_utc(),
            valid: true,
            download_status: STATUS_COMPLETED,
            rescan_reply: true,
            ..Default::default()
        };
        let active: dynamic::ActiveModel = model.into();
        Ok(active.reset_all().insert(db).await?)
    }

    type RecordedQueries = Arc<StdMutex<Vec<Vec<(String, String)>>>>;

    async fn mock_api(pages: Vec<Value>) -> Result<(String, RecordedQueries, tokio::task::JoinHandle<()>)> {
        let pages = Arc::new(StdMutex::new(VecDeque::from(pages)));
        let queries: RecordedQueries = Arc::new(StdMutex::new(Vec::new()));
        let recorded = queries.clone();
        let app = Router::new().route(
            "/",
            get(move |RawQuery(query): RawQuery| {
                let pages = pages.clone();
                let recorded = recorded.clone();
                async move {
                    recorded
                        .lock()
                        .unwrap()
                        .push(serde_urlencoded::from_str(&query.unwrap_or_default()).unwrap());
                    Json(
                        pages
                            .lock()
                            .unwrap()
                            .pop_front()
                            .unwrap_or(json!({"code": -1, "message": "unexpected request"})),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/", listener.local_addr()?);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Ok((url, queries, task))
    }

    fn main_response(id: i64, end: bool, next: &str, children: u64) -> Value {
        json!({"code": 0, "data": {
            "cursor": {"is_end": end, "pagination_reply": {"next_offset": next}},
            "replies": [{"rpid": id, "ctime": 1, "rcount": children, "content": {"message": "test"}}]
        }})
    }

    #[test]
    fn failed_rescans_rotate_and_backlog_counts_are_separate() {
        let now = chrono::Utc::now().naive_utc();
        let dynamics = (0..30)
            .map(|n| dynamic::Model {
                id: n.to_string(),
                rescan_reply: n < 8,
                reply_last_attempt_at: (n < 5).then_some(now),
                pub_ts: now - chrono::Duration::seconds(n),
                ..Default::default()
            })
            .collect();
        let (batch, rescans_left, new_left) = select_dynamic_batch(dynamics, 5, 20);
        assert_eq!(rescans_left, 3);
        assert_eq!(new_left, 2);
        assert_eq!(
            batch.iter().take(3).map(|d| d.id.as_str()).collect::<Vec<_>>(),
            vec!["5", "6", "7"]
        );
        assert_eq!(batch.len(), 25);
    }

    #[tokio::test]
    async fn scan_resumes_after_request_budget_without_losing_or_invalidating_history() -> Result<()> {
        let db = test_database().await?;
        let model = insert_dynamic(&db, "budget").await?;
        save_replies(&model.id, &[comment(999)], &db).await?;
        let pages = (1..=51)
            .map(|n| main_response(n, n == 51, &format!("page-{}", n + 1), 0))
            .collect();
        let (endpoint, queries, server) = mock_api(pages).await?;
        let client = BiliClient::new();
        let credential = Credential::default();
        let api = Reply::for_test(&client, &credential, endpoint);
        let directory = TestDirectory::new();
        let source = dynamic_source::Model {
            id: 1,
            upper_id: 1,
            upper_name: "test".into(),
            path: directory.0.to_string_lossy().into(),
            sync_reply: true,
            enabled: true,
            created_at: chrono::Utc::now().naive_utc(),
            latest_dyn_at: chrono::Utc::now().naive_utc(),
        };
        let downloader = Downloader::new(client.client.clone());
        let config = Config::default();
        process_dynamic(&source, model, &downloader, &api, &db, &config).await?;
        assert_eq!(queries.lock().unwrap().len(), MAX_REPLY_REQUESTS_PER_ROUND);
        let paused = dynamic::Entity::find_by_id("budget").one(&db).await?.unwrap();
        assert!(paused.rescan_reply);
        assert!(paused.reply_synced_at.is_none());
        assert!(paused.reply_sync_state.is_some());
        assert!(reply::Entity::find_by_id(999).one(&db).await?.unwrap().valid);
        // 用数据库重新加载的断点继续，模拟下一轮 / 程序重启后的扫描。
        process_dynamic(&source, paused, &downloader, &api, &db, &config).await?;
        let finished = dynamic::Entity::find_by_id("budget").one(&db).await?.unwrap();
        assert!(!finished.rescan_reply);
        assert!(finished.reply_sync_state.is_none());
        assert!(finished.reply_synced_at.is_some());
        assert_eq!(finished.stat.unwrap()["comment"]["count"], 100);
        assert_eq!(queries.lock().unwrap().len(), 51);
        let last_query = queries.lock().unwrap().last().unwrap().clone();
        let offsets: Vec<_> = last_query.iter().filter(|(key, _)| key == "pagination_str").collect();
        assert_eq!(offsets.len(), 1);
        assert_eq!(serde_json::from_str::<Value>(&offsets[0].1)?["offset"], "page-51");
        assert!(!reply::Entity::find_by_id(999).one(&db).await?.unwrap().valid);
        assert_eq!(
            reply::Entity::find()
                .filter(reply::Column::Valid.eq(true))
                .count(&db)
                .await?,
            51
        );
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn sub_reply_requests_share_the_same_budget_and_resume_at_saved_page() -> Result<()> {
        let db = test_database().await?;
        let model = insert_dynamic(&db, "sub-budget").await?;
        let mut pages = vec![main_response(5000, true, "unused", 1020)];
        for n in 0..51 {
            pages.push(json!({"code": 0, "data": {"page": {"size": 20, "count": 1020},
                "replies": (1+n*20..=20+n*20).map(|id| json!({"rpid": id, "ctime": 1, "content": {"message": "test"}})).collect::<Vec<_>>()}}));
        }
        let (endpoint, queries, server) = mock_api(pages).await?;
        let client = BiliClient::new();
        let credential = Credential::default();
        let api = Reply::for_test(&client, &credential, endpoint);
        let directory = TestDirectory::new();
        let downloader = Downloader::new(client.client.clone());
        let config = Config::default();
        assert!(matches!(
            sync_dynamic_replies(&model, 1, "123", &directory.0, &downloader, &api, &db, &config).await?,
            ReplySyncResult::Incomplete
        ));
        assert_eq!(queries.lock().unwrap().len(), 50);
        let resumed = dynamic::Entity::find_by_id("sub-budget").one(&db).await?.unwrap();
        assert!(matches!(
            sync_dynamic_replies(&resumed, 1, "123", &directory.0, &downloader, &api, &db, &config).await?,
            ReplySyncResult::Complete
        ));
        {
            let queries = queries.lock().unwrap();
            assert_eq!(queries.len(), 52);
            assert!(queries[50].contains(&("pn".into(), "50".into())));
        }
        assert_eq!(reply::Entity::find().count(&db).await?, 1021);
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn real_numeric_cursor_shape_requests_three_distinct_pages_without_mixing_protocols() -> Result<()> {
        let db = test_database().await?;
        let model = insert_dynamic(&db, "numeric-pages").await?;
        let pages = (1..=3)
            .map(|id| {
                let mut response = main_response(id, id == 3, "same-unused-offset", 0);
                response["data"]["cursor"]["next"] = json!(id + 1);
                response
            })
            .collect();
        let (endpoint, queries, server) = mock_api(pages).await?;
        let client = BiliClient::new();
        let credential = Credential::default();
        let api = Reply::for_test(&client, &credential, endpoint);
        let directory = TestDirectory::new();
        let downloader = Downloader::new(client.client.clone());
        assert!(matches!(
            sync_dynamic_replies(
                &model,
                1,
                "123",
                &directory.0,
                &downloader,
                &api,
                &db,
                &Config::default()
            )
            .await?,
            ReplySyncResult::Complete
        ));
        {
            let queries = queries.lock().unwrap();
            assert_eq!(queries.len(), 3);
            for (query, next) in queries.iter().zip([0, 2, 3]) {
                assert!(query.contains(&("next".into(), next.to_string())));
                assert!(!query.iter().any(|(key, _)| key == "pagination_str"));
            }
        }
        assert_eq!(reply::Entity::find().count(&db).await?, 3);
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn risk_control_stops_requests_and_preserves_the_last_committed_page() -> Result<()> {
        let db = test_database().await?;
        let model = insert_dynamic(&db, "interrupted").await?;
        save_replies(&model.id, &[comment(999)], &db).await?;
        let pages = vec![
            main_response(1, false, "page-two", 0),
            json!({"code": -352, "message": "risk control"}),
            main_response(2, true, "unused", 0),
        ];
        let (endpoint, queries, server) = mock_api(pages).await?;
        let client = BiliClient::new();
        let credential = Credential::default();
        let api = Reply::for_test(&client, &credential, endpoint);
        let directory = TestDirectory::new();
        let downloader = Downloader::new(client.client.clone());
        let config = Config::default();
        let error = sync_dynamic_replies(&model, 1, "123", &directory.0, &downloader, &api, &db, &config)
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<BiliError>().unwrap().is_risk_control_related());
        assert_eq!(queries.lock().unwrap().len(), 2);
        assert!(reply::Entity::find_by_id(1).one(&db).await?.is_some());
        assert!(reply::Entity::find_by_id(999).one(&db).await?.unwrap().valid);
        let resumed = dynamic::Entity::find_by_id("interrupted").one(&db).await?.unwrap();
        assert!(resumed.reply_sync_state.is_some());
        assert!(matches!(
            sync_dynamic_replies(&resumed, 1, "123", &directory.0, &downloader, &api, &db, &config).await?,
            ReplySyncResult::Complete
        ));
        assert_eq!(queries.lock().unwrap().len(), 3);
        assert_eq!(
            reply::Entity::find()
                .filter(reply::Column::Valid.eq(true))
                .count(&db)
                .await?,
            2
        );
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn checkpoint_rolls_back_comments_if_dynamic_disappears() -> Result<()> {
        let db = test_database().await?;
        assert!(
            checkpoint_reply_page("missing", &[comment(1)], &ReplySyncState::default(), &db)
                .await
                .is_err()
        );
        assert_eq!(reply::Entity::find().count(&db).await?, 0);
        Ok(())
    }

    #[tokio::test]
    async fn completion_and_closed_comments_are_not_automatically_requeued() -> Result<()> {
        let db = test_database().await?;
        let model = insert_dynamic(&db, "completed").await?;
        let mut active: dynamic::ActiveModel = model.into();
        active.rescan_reply = Set(false);
        active.reply_synced_at = Set(Some(chrono::Utc::now().naive_utc()));
        active.save(&db).await?;
        let source = dynamic_source::Model {
            id: 1,
            upper_id: 1,
            upper_name: "test".into(),
            path: String::new(),
            sync_reply: true,
            enabled: true,
            created_at: chrono::Utc::now().naive_utc(),
            latest_dyn_at: chrono::Utc::now().naive_utc(),
        };
        backfill_missing_replies(&source, &db).await?;
        assert!(
            !dynamic::Entity::find_by_id("completed")
                .one(&db)
                .await?
                .unwrap()
                .rescan_reply
        );
        Ok(())
    }

    #[tokio::test]
    async fn progress_migration_can_be_reverted_and_preserves_existing_rows() -> Result<()> {
        let db = test_database().await?;
        insert_dynamic(&db, "existing").await?;
        Migrator::down(&db, Some(progress_migration_steps())).await?;
        Migrator::up(&db, Some(progress_migration_steps())).await?;
        let row = dynamic::Entity::find_by_id("existing").one(&db).await?.unwrap();
        assert!(row.rescan_reply);
        assert!(row.reply_sync_state.is_none());
        assert!(row.reply_last_attempt_at.is_none());
        assert!(row.reply_synced_at.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn progress_migration_resumes_after_each_partially_applied_schema() -> Result<()> {
        let changes = [
            "ALTER TABLE dynamic ADD COLUMN reply_sync_state json NULL",
            "ALTER TABLE dynamic ADD COLUMN reply_last_attempt_at timestamp NULL",
            "ALTER TABLE dynamic ADD COLUMN reply_synced_at timestamp NULL",
        ];
        for applied_columns in 1..=changes.len() {
            let db = test_database().await?;
            insert_dynamic(&db, "existing").await?;
            Migrator::down(&db, Some(progress_migration_steps())).await?;
            // 模拟列已部分写入、进程却在迁移记录提交之前退出。
            for change in changes.iter().take(applied_columns) {
                db.execute_unprepared(change).await?;
            }
            db.execute_unprepared("UPDATE dynamic SET reply_sync_state = '{\"resume\": true}'")
                .await?;
            Migrator::up(&db, Some(progress_migration_steps())).await?;
            let row = dynamic::Entity::find_by_id("existing").one(&db).await?.unwrap();
            assert!(row.rescan_reply);
            assert_eq!(row.reply_sync_state, Some(json!({"resume": true})));
            assert!(row.reply_last_attempt_at.is_none());
            assert!(row.reply_synced_at.is_none());
        }
        Ok(())
    }

    fn progress_migration_steps() -> u32 {
        Migrator::migrations()
            .iter()
            .rev()
            .take_while(|m| m.name() != "m20261008_000001_add_reply_sync_progress")
            .count() as u32
            + 1
    }
}
