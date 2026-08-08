use anyhow::{Context, Result, anyhow};
use async_stream::try_stream;
use chrono::{DateTime, Utc};
use futures::Stream;
use reqwest::Method;
use serde_json::Value;
use tokio::time::{Duration, sleep};

use crate::bilibili::{BiliClient, Credential, ErrorForStatusExt, MIXIN_KEY, Validate, VideoInfo, WbiSign};

/// 动态列表接口请求间隔，与视频接口节奏对齐（全局限流器 250ms/4 之上再加 250ms）
const DYNAMIC_FEED_INTERVAL: Duration = Duration::from_millis(250);

pub struct Dynamic<'a> {
    client: &'a BiliClient,
    pub upper_id: String,
    credential: &'a Credential,
}

impl<'a> Dynamic<'a> {
    pub fn new(client: &'a BiliClient, upper_id: String, credential: &'a Credential) -> Self {
        Self {
            client,
            upper_id,
            credential,
        }
    }

    pub async fn get_dynamics(&self, offset: Option<String>) -> Result<Value> {
        self.client
            .request(
                Method::GET,
                "https://api.bilibili.com/x/polymer/web-dynamic/v1/feed/space",
                self.credential,
            )
            .await
            .query(&[
                ("host_mid", self.upper_id.as_str()),
                ("offset", offset.as_deref().unwrap_or("")),
                ("type", "video"),
            ])
            .wbi_sign(MIXIN_KEY.load().as_deref())?
            .send()
            .await?
            .error_for_status_ext()?
            .json::<serde_json::Value>()
            .await?
            .validate()
    }

    pub fn into_video_stream(self) -> impl Stream<Item = Result<VideoInfo>> + 'a {
        try_stream! {
            let mut offset = None;
            loop {
                let mut res = self
                    .get_dynamics(offset.take())
                    .await
                    .with_context(|| "failed to get dynamics")?;
                let items = match res["data"]["items"].as_array_mut() {
                    Some(items) if !items.is_empty() => items,
                    _ => {
                        if offset.is_none() {
                            break;
                        }
                        Err(anyhow!("no dynamics found in offset {:?}", offset))?
                    }
                };
                for item in items.iter_mut() {
                    if item["type"].as_str().is_none_or(|t| t != "DYNAMIC_TYPE_AV") {
                        continue;
                    }
                    let pub_ts = item["modules"]["module_author"]["pub_ts"].take();
                    let pub_dt = pub_ts
                        .as_i64()
                        .or_else(|| pub_ts.as_str().and_then(|s| s.parse::<i64>().ok()))
                        .and_then(DateTime::from_timestamp_secs)
                        .with_context(|| format!("invalid pub_ts: {:?}", pub_ts))?;
                    let mut video_info: VideoInfo =
                        serde_json::from_value(item["modules"]["module_dynamic"]["major"]["archive"].take())?;
                    // 这些地方不使用 let else 是因为 try_stream! 宏不支持
                    if let VideoInfo::Dynamic { ref mut pubtime, .. } = video_info {
                        *pubtime = pub_dt;
                        yield video_info;
                    } else {
                        Err(anyhow!("video info is not dynamic"))?;
                    }
                }
                if let (Some(has_more), Some(new_offset)) =
                    (res["data"]["has_more"].as_bool(), res["data"]["offset"].as_str())
                {
                    if !has_more {
                        break;
                    }
                    offset = Some(new_offset.to_string());
                } else {
                    Err(anyhow!("no has_more or offset found"))?;
                }
            }
        }
    }
}

/// 一条完整动态的信息，包含动态下的所有内容
#[derive(Debug, Clone)]
pub struct DynamicInfo {
    /// 动态 id（id_str）
    pub id: String,
    /// 动态类型，如 DYNAMIC_TYPE_AV / DYNAMIC_TYPE_DRAW / DYNAMIC_TYPE_COMMON_SQUARE / DYNAMIC_TYPE_FORWARD
    pub dyn_type: String,
    /// 渲染后的正文文本（含转发内容）
    pub content: String,
    /// 图片 URL 列表（http 已替换为 https）
    pub pics: Vec<String>,
    /// 点赞/评论/转发数（module_stat）
    pub stat: Value,
    /// 发布时间
    pub pub_ts: DateTime<Utc>,
    /// 评论类型（basic.comment_type），用于评论接口
    pub comment_type: i64,
    /// 评论对象 id（basic.rid_str），用于评论接口
    pub comment_oid: String,
    /// 发布 IP 属地（如"江苏"），可为空
    pub location: String,
    /// 原始接口 JSON
    pub raw: Value,
}

/// 用户空间动态（全类型）
pub struct DynamicFeed<'a> {
    client: &'a BiliClient,
    pub upper_id: String,
    credential: &'a Credential,
}

impl<'a> DynamicFeed<'a> {
    pub fn new(client: &'a BiliClient, upper_id: String, credential: &'a Credential) -> Self {
        Self {
            client,
            upper_id,
            credential,
        }
    }

    /// 不传 type 参数，获取全部类型的动态
    pub async fn get_dynamics(&self, offset: Option<String>) -> Result<Value> {
        self.client
            .request(
                Method::GET,
                "https://api.bilibili.com/x/polymer/web-dynamic/v1/feed/space",
                self.credential,
            )
            .await
            .query(&[
                ("host_mid", self.upper_id.as_str()),
                ("offset", offset.as_deref().unwrap_or("")),
                (
                    "features",
                    "itemOpusStyle,listOnlyfans,opusBigCover,onlyfansVote,forwardListHidden,decorationCard,commentsNewVersion,onlyfansAssetsV2,ugcDelete,onlyfansQaCard",
                ),
            ])
            .wbi_sign(MIXIN_KEY.load().as_deref())?
            .send()
            .await?
            .error_for_status_ext()?
            .json::<serde_json::Value>()
            .await?
            .validate()
    }

    pub fn into_dynamic_stream(self) -> impl Stream<Item = Result<DynamicInfo>> + 'a {
        try_stream! {
            let mut offset = None;
            loop {
                sleep(DYNAMIC_FEED_INTERVAL).await;
                let res = self
                    .get_dynamics(offset.take())
                    .await
                    .with_context(|| "failed to get dynamics")?;
                let items = match res["data"]["items"].as_array() {
                    Some(items) if !items.is_empty() => items,
                    _ => {
                        if offset.is_none() {
                            break;
                        }
                        Err(anyhow!("no dynamics found in offset {:?}", offset))?
                    }
                };
                for item in items.iter() {
                    if let Some(info) = parse_dynamic_info(item).await? {
                        yield info;
                    }
                }
                if let (Some(has_more), Some(new_offset)) =
                    (res["data"]["has_more"].as_bool(), res["data"]["offset"].as_str())
                {
                    if !has_more {
                        break;
                    }
                    offset = Some(new_offset.to_string());
                } else {
                    Err(anyhow!("no has_more or offset found"))?;
                }
            }
        }
    }
}

/// 解析一条动态为 DynamicInfo，无法解析的类型（如广告卡片）返回 None
async fn parse_dynamic_info(item: &Value) -> Result<Option<DynamicInfo>> {
    let dyn_type = match item["type"].as_str() {
        Some(t) => t.to_string(),
        None => return Ok(None),
    };
    if !dyn_type.starts_with("DYNAMIC_TYPE_") {
        return Ok(None);
    }
    let id = match item["id_str"].as_str() {
        Some(id) => id.to_string(),
        None => return Ok(None),
    };
    let pub_ts = &item["modules"]["module_author"]["pub_ts"];
    let pub_dt = pub_ts
        .as_i64()
        .or_else(|| pub_ts.as_str().and_then(|s| s.parse::<i64>().ok()))
        .and_then(DateTime::from_timestamp_secs)
        .with_context(|| format!("invalid pub_ts: {:?}", pub_ts))?;
    let modules = &item["modules"]["module_dynamic"];
    let content = render_module_dynamic(modules, &dyn_type);
    let pics = extract_pics(modules);
    let stat = item["modules"]["module_stat"].clone();
    let basic = &item["basic"];
    let comment_type = basic["comment_type"].as_i64().unwrap_or(-1);
    let comment_oid = basic["rid_str"].as_str().unwrap_or("").to_string();
    let location = item["modules"]["module_author"]["pub_location_text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    Ok(Some(DynamicInfo {
        id,
        dyn_type,
        content,
        pics,
        stat,
        pub_ts: pub_dt,
        comment_type,
        comment_oid,
        location,
        raw: item.clone(),
    }))
}

/// 渲染动态正文为纯文本，转发动态会附带转发源的内容
fn render_module_dynamic(modules: &Value, dyn_type: &str) -> String {
    let mut parts = Vec::new();
    if let Some(desc) = modules["desc"]["text"].as_str() {
        let desc = desc.trim();
        if !desc.is_empty() {
            parts.push(desc.to_string());
        }
    }
    if let Some(desc) = modules["desc"].as_str() {
        let desc = desc.trim();
        if !desc.is_empty() {
            parts.push(desc.to_string());
        }
    }
    let major = &modules["major"];
    match major["type"].as_str() {
        Some("MAJOR_TYPE_OPUS") => {
            if let Some(title) = major["opus"]["title"].as_str() {
                parts.push(title.trim().to_string());
            }
            if let Some(text) = major["opus"]["summary"]["text"].as_str() {
                parts.push(text.trim().to_string());
            }
        }
        Some("MAJOR_TYPE_DRAW") => {
            if let Some(text) = major["draw"]["text"].as_str() {
                parts.push(text.trim().to_string());
            }
        }
        Some("MAJOR_TYPE_ARCHIVE") => {
            if let Some(title) = major["archive"]["title"].as_str() {
                parts.push(title.trim().to_string());
            }
            if let Some(desc) = major["archive"]["desc"].as_str() {
                let desc = desc.trim();
                if !desc.is_empty() && desc != "-" {
                    parts.push(desc.to_string());
                }
            }
            if let Some(bvid) = major["archive"]["bvid"].as_str() {
                parts.push(format!("视频链接: https://www.bilibili.com/video/{bvid}"));
            }
        }
        Some("MAJOR_TYPE_ARTICLE") => {
            if let Some(title) = major["article"]["title"].as_str() {
                parts.push(title.trim().to_string());
            }
            if let Some(desc) = major["article"]["desc"].as_str() {
                parts.push(desc.trim().to_string());
            }
        }
        _ => {}
    }
    // 转发动态：附加转发源的内容
    if dyn_type == "DYNAMIC_TYPE_FORWARD" {
        if let Some(origin) = modules["origin"].as_object()
            && let Some(origin_type) = origin["type"].as_str()
            && origin_type.starts_with("DYNAMIC_TYPE_")
        {
            let upper_name = origin["modules"]["module_author"]["name"].as_str().unwrap_or("未知用户");
            let origin_content = render_module_dynamic(&origin["modules"]["module_dynamic"], origin_type);
            let jump = origin["id_str"].as_str().map(|id| format!("\n原动态链接: https://www.bilibili.com/opus/{id}")).unwrap_or_default();
            parts.push(format!("---- 转发自 @{upper_name} ----\n{origin_content}{jump}"));
        }
    }
    parts.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n")
}

/// 提取动态正文中的图片 URL（原图），http 统一替换为 https
fn extract_pics(modules: &Value) -> Vec<String> {
    let mut pics = Vec::new();
    let major = &modules["major"];
    match major["type"].as_str() {
        Some("MAJOR_TYPE_OPUS") => {
            if let Some(items) = major["opus"]["pics"].as_array() {
                for item in items {
                    if let Some(url) = item["url"].as_str().or_else(|| item["src"].as_str()) {
                        pics.push(normalize_url(url));
                    }
                }
            }
        }
        Some("MAJOR_TYPE_DRAW") => {
            if let Some(items) = major["draw"]["items"].as_array() {
                for item in items {
                    if let Some(url) = item["src"].as_str() {
                        pics.push(normalize_url(url));
                    }
                }
            }
        }
        _ => {}
    }
    pics
}

fn normalize_url(url: &str) -> String {
    if url.starts_with("http://") {
        format!("https://{}", &url[7..])
    } else {
        url.to_string()
    }
}
