use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use bili_sync_entity::{dynamic, dynamic_source, reply, upper_stat};
use futures::StreamExt;
use sea_orm::ActiveValue::Set;
use sea_orm::QueryOrder;
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::OnConflict;
use serde_json::Value;
use tokio::fs;

use crate::bilibili::{BiliClient, BiliError, DynamicFeed, DynamicInfo, MIXIN_KEY, Reply, ReplyInfo, UpperInfo};
use crate::config::Config;
use crate::downloader::Downloader;
use crate::utils::dynamic_render::{render_comments_md, render_dynamic_md};
use crate::utils::status::STATUS_COMPLETED;

/// 顶级评论最大翻页数（每页 20 条）
const MAX_REPLY_PAGES: usize = 50;
/// 楼中楼最大翻页数
const MAX_SUB_REPLY_PAGES: usize = 10;
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
pub async fn process_dynamic_source(
    source: dynamic_source::Model,
    bili_client: &BiliClient,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<()> {
    // wbi 签名密钥不依赖视频任务的全局状态，动态同步独立初始化
    ensure_mixin_key(bili_client, &config.credential).await?;
    fs::create_dir_all(&source.path)
        .await
        .with_context(|| format!("failed to create dynamic source directory {}", source.path))?;
    info!("开始处理动态源「{}」..", source.upper_name);
    // 记录账号信息快照（粉丝/关注/投稿/播放/名字/签名），有变化才插入新记录
    update_upper_stat(&source, bili_client, connection, config).await?;
    refresh_dynamic_source(&source, bili_client, connection, config).await?;
    // 评论补拉：5 天窗口外的历史动态，若 API 评论数 > 0 但本地无评论，自动标记重扫
    backfill_missing_replies(&source, connection).await?;
    process_unhandled_dynamics(&source, bili_client, connection, config).await?;
    info!("处理动态源「{}」完成", source.upper_name);
    Ok(())
}

/// 评论补拉：对已完成的动态，若 API 评论数 > 0 但本地无评论，自动标记重扫
///
/// 解决首次全量导入时 5 天窗口外历史动态评论被永久跳过的问题。
/// 每轮限量检查，标记后由 process_unhandled_dynamics 按慢任务队列消化。
async fn backfill_missing_replies(source: &dynamic_source::Model, connection: &DatabaseConnection) -> Result<()> {
    /// 每轮最多检查的已完成动态数
    const BACKFILL_CHECK_LIMIT: u64 = 50;
    let mut candidates = dynamic::Entity::find()
        .filter(dynamic::Column::SourceId.eq(source.id))
        .filter(dynamic::Column::Valid.eq(true))
        .filter(dynamic::Column::DownloadStatus.gte(STATUS_COMPLETED))
        .filter(dynamic::Column::RescanReply.eq(false))
        .order_by_desc(dynamic::Column::PubTs)
        .all(connection)
        .await?;
    candidates.truncate(BACKFILL_CHECK_LIMIT as usize);
    let mut marked = 0;
    for dyn_model in candidates {
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
            .count(connection)
            .await?;
        if local_count > 0 {
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

/// 拉取 UP 主账号信息并写入快照表，与最近一条快照对比，有变化才插入
pub async fn update_upper_stat(
    source: &dynamic_source::Model,
    bili_client: &BiliClient,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<()> {
    let upper = UpperInfo::new(bili_client, source.upper_id.to_string(), &config.credential);
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
    let changed = match latest {
        Some(s) => {
            s.name != profile.name
                || s.sign != profile.sign
                || s.face != profile.face
                || s.fan_count != profile.fan_count
                || s.follow_count != profile.follow_count
                || s.video_count != profile.video_count
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
            video_count: Set(profile.video_count),
            view_count: Set(profile.view_count),
            like_count: Set(profile.like_count),
            recorded_at: Set(chrono::Utc::now().naive_utc()),
            ..Default::default()
        })
        .exec(connection)
        .await?;
        info!(
            "「{}」账号信息更新：粉丝 {} 关注 {} 投稿 {} 播放 {} 获赞 {}",
            source.upper_name,
            profile.fan_count,
            profile.follow_count,
            profile.video_count,
            profile.view_count,
            profile.like_count
        );
    } else {
        info!(
            "「{}」账号信息无变化（粉丝 {} 关注 {} 投稿 {} 播放 {} 获赞 {}），跳过记录",
            source.upper_name,
            profile.fan_count,
            profile.follow_count,
            profile.video_count,
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
    const MAX_RESCAN_PER_ROUND: usize = 10;
    /// 每轮最多处理的新动态数
    const MAX_NEW_PER_ROUND: usize = 20;
    let dynamics = dynamic::Entity::find()
        .filter(dynamic::Column::SourceId.eq(source.id))
        .filter(dynamic::Column::Valid.eq(true))
        .filter(dynamic::Column::DownloadStatus.lt(STATUS_COMPLETED))
        .order_by_desc(dynamic::Column::PubTs)
        .all(connection)
        .await
        .context("filter unhandled dynamics failed")?;
    if dynamics.is_empty() {
        return Ok(());
    }
    // 重扫标记的动态优先，且限制每轮处理数量
    let (rescan, new): (Vec<_>, Vec<_>) = dynamics.into_iter().partition(|d| d.rescan_reply);
    let mut dynamics: Vec<_> = rescan.into_iter().take(MAX_RESCAN_PER_ROUND).collect();
    let new_count = new.len();
    dynamics.extend(new.into_iter().take(MAX_NEW_PER_ROUND));
    info!(
        "开始处理「{}」的 {} 条未完成动态（本轮，含重扫 {} 条，剩余待重扫 {} 条）..",
        source.upper_name,
        dynamics.len(),
        dynamics.iter().filter(|d| d.rescan_reply).count(),
        new_count.saturating_sub(MAX_NEW_PER_ROUND)
    );
    let downloader = Downloader::new(bili_client.client.clone());
    let reply_api = Reply::new(bili_client, &config.credential);
    for dyn_model in dynamics {
        let dyn_id = dyn_model.id.clone();
        if let Err(e) = process_dynamic(source, dyn_model, &downloader, &reply_api, connection, config).await {
            error!("处理动态 {dyn_id} 失败：{:#}", e);
            if let Ok(e) = e.downcast::<BiliError>()
                && e.is_risk_control_related()
            {
                bail!(e);
            }
        }
    }
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
    // 下载正文图片
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
    // 同步评论：发布 5 天内的动态每轮自动同步，5 天外的仅在被手动标记重扫时同步
    let within_window =
        dyn_model.pub_ts.and_utc() + chrono::Duration::days(REPLY_SYNC_WINDOW_DAYS) >= chrono::Utc::now();
    let need_rescan = dyn_model.rescan_reply;
    if source.sync_reply && (within_window || need_rescan) {
        sync_dynamic_replies(
            &dyn_model.id,
            dyn_model.comment_type,
            &dyn_model.comment_oid,
            &dir.join("comments"),
            downloader,
            reply_api,
            connection,
            config,
        )
        .await?;
        info!(
            "动态 {} 评论同步完成{}",
            dyn_model.id,
            if need_rescan { "（手动重扫）" } else { "" }
        );
    }
    // 标记完成
    let dyn_id = dyn_model.id.clone();
    let mut model: dynamic::ActiveModel = dyn_model.into();
    model.download_status = Set(STATUS_COMPLETED);
    model.path = Set(dir.to_string_lossy().to_string());
    model.rescan_reply = Set(false);
    model.save(connection).await?;
    info!("处理动态 {dyn_id} 完成");
    Ok(())
}

/// 拉取动态的评论：存库、导出 JSON/Markdown、下载评论图片
#[allow(clippy::too_many_arguments)]
async fn sync_dynamic_replies(
    dynamic_id: &str,
    comment_type: i64,
    comment_oid: &str,
    comments_dir: &PathBuf,
    downloader: &Downloader,
    reply_api: &Reply<'_>,
    connection: &DatabaseConnection,
    config: &Config,
) -> Result<()> {
    if comment_type <= 0 || comment_oid.is_empty() {
        warn!("动态 {dynamic_id} 缺少评论信息（comment_type={comment_type}），跳过评论同步");
        return Ok(());
    }
    let replies = reply_api
        .get_replies(comment_type, comment_oid, MAX_REPLY_PAGES, MAX_SUB_REPLY_PAGES)
        .await
        .with_context(|| format!("failed to get replies of dynamic {dynamic_id}"))?;
    // 写入数据库
    save_replies(dynamic_id, &replies, connection).await?;
    // 导出 JSON / Markdown
    fs::create_dir_all(comments_dir).await?;
    fs::write(
        comments_dir.join("comments.json"),
        serde_json::to_string_pretty(&replies)?,
    )
    .await?;
    fs::write(comments_dir.join("comments.md"), render_comments_md(&replies)).await?;
    // 下载评论图片
    for reply in &replies {
        for (i, url) in reply.images.iter().enumerate() {
            downloader
                .fetch(
                    url,
                    &comments_dir.join(format!("{}_{}.jpg", reply.rpid, i + 1)),
                    &config.concurrent_limit.download,
                )
                .await?;
        }
        for sub in &reply.sub_replies {
            for (i, url) in sub.images.iter().enumerate() {
                downloader
                    .fetch(
                        url,
                        &comments_dir.join(format!("{}_{}.jpg", sub.rpid, i + 1)),
                        &config.concurrent_limit.download,
                    )
                    .await?;
            }
        }
    }
    Ok(())
}

/// 将评论（含楼中楼）写入数据库
async fn save_replies(dynamic_id: &str, replies: &[ReplyInfo], connection: &DatabaseConnection) -> Result<()> {
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
    reply::Entity::insert_many(models)
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
                ])
                .to_owned(),
        )
        .exec(connection)
        .await?;
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
