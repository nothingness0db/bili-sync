use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Method;
use serde_json::{Value, json};
use tokio::time::{Duration, sleep};

use crate::bilibili::{BiliClient, Credential, ErrorForStatusExt, MIXIN_KEY, Validate, WbiSign};

/// 评论接口请求间隔，风控敏感接口，保守节流
const REPLY_REQUEST_INTERVAL: Duration = Duration::from_millis(400);

/// 一条评论（含楼中楼回复）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReplyInfo {
    pub rpid: i64,
    /// 楼中楼回复的父评论 rpid，顶级评论为 None
    pub parent_rpid: Option<i64>,
    pub uname: String,
    pub avatar: String,
    pub content: String,
    /// 评论中的图片 URL
    pub images: Vec<String>,
    pub ctime: DateTime<Utc>,
    /// 原始 JSON
    pub raw: Value,
    /// 楼中楼回复
    pub sub_replies: Vec<ReplyInfo>,
}

pub struct Reply<'a> {
    client: &'a BiliClient,
    credential: &'a Credential,
}

impl<'a> Reply<'a> {
    pub fn new(client: &'a BiliClient, credential: &'a Credential) -> Self {
        Self { client, credential }
    }

    /// 获取某个评论区（type + oid）下的所有顶级评论及其楼中楼回复
    ///
    /// `max_pages` 限制顶级评论的最大翻页数，`max_sub_pages` 限制每条顶级评论楼中楼的最大翻页数
    pub async fn get_replies(
        &self,
        comment_type: i64,
        oid: &str,
        max_pages: usize,
        max_sub_pages: usize,
    ) -> Result<Vec<ReplyInfo>> {
        let mut all = Vec::new();
        let mut next_offset: Option<Value> = None;
        for _ in 0..max_pages {
            sleep(REPLY_REQUEST_INTERVAL).await;
            let mut req = self
                .client
                .request(
                    Method::GET,
                    "https://api.bilibili.com/x/v2/reply/wbi/main",
                    self.credential,
                )
                .await
                .query(&[
                    ("type", comment_type.to_string()),
                    ("oid", oid.to_string()),
                    ("mode", "3".to_string()),
                    ("pagination_str", "{\"offset\":\"\"}".to_string()),
                ]);
            if let Some(offset) = next_offset.take() {
                req = req.query(&[("pagination_str", json!({ "offset": offset }).to_string())]);
            }
            let mut res = req
                .wbi_sign(MIXIN_KEY.load().as_deref())?
                .send()
                .await?
                .error_for_status_ext()?
                .json::<serde_json::Value>()
                .await?
                .validate()?;
            let data = res["data"].take();
            let replies = match data["replies"].as_array() {
                Some(replies) if !replies.is_empty() => replies.clone(),
                _ => break,
            };
            for reply in replies.iter() {
                let mut info = self.parse_reply(reply)?;
                // 拉取楼中楼回复
                info.sub_replies = self
                    .get_sub_replies(comment_type, oid, info.rpid, max_sub_pages)
                    .await
                    .with_context(|| format!("failed to get sub replies of rpid {}", info.rpid))?;
                all.push(info);
            }
            // 下一页游标
            match data["cursor"]["pagination_reply"]["next_offset"] {
                Value::Null => break,
                ref offset if offset.is_null() || offset.is_string() && offset.as_str().unwrap_or("").is_empty() => {
                    break;
                }
                ref offset => next_offset = Some(offset.clone()),
            }
        }
        Ok(all)
    }

    /// 解析单条评论
    fn parse_reply(&self, reply: &Value) -> Result<ReplyInfo> {
        let rpid = reply["rpid"].as_i64().context("invalid rpid")?;
        let parent_rpid = reply["parent"]
            .as_i64()
            .filter(|r| *r != 0)
            .or_else(|| reply["replied_comment"]["rpid"].as_i64().filter(|r| *r != 0));
        let member = &reply["member"];
        let uname = member["uname"].as_str().unwrap_or("未知用户").to_string();
        let avatar = member["avatar"].as_str().unwrap_or_default().to_string();
        let content = reply["content"]["message"].as_str().unwrap_or_default().to_string();
        let images = reply["content"]["pictures"]
            .as_array()
            .map(|pics| {
                pics.iter()
                    .filter_map(|p| p["img_src"].as_str().or_else(|| p["img_url"].as_str()))
                    .map(normalize_url)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let ctime = reply["ctime"]
            .as_i64()
            .and_then(DateTime::from_timestamp_secs)
            .with_context(|| format!("invalid ctime: {:?}", reply["ctime"]))?;
        Ok(ReplyInfo {
            rpid,
            parent_rpid,
            uname,
            avatar,
            content,
            images,
            ctime,
            raw: reply.clone(),
            sub_replies: Vec::new(),
        })
    }

    /// 获取某条评论的楼中楼回复
    async fn get_sub_replies(
        &self,
        comment_type: i64,
        oid: &str,
        root: i64,
        max_pages: usize,
    ) -> Result<Vec<ReplyInfo>> {
        let mut all = Vec::new();
        for page in 1..=max_pages {
            sleep(REPLY_REQUEST_INTERVAL).await;
            let mut res = self
                .client
                .request(
                    Method::GET,
                    "https://api.bilibili.com/x/v2/reply/reply",
                    self.credential,
                )
                .await
                .query(&[
                    ("type", comment_type.to_string()),
                    ("oid", oid.to_string()),
                    ("root", root.to_string()),
                    ("ps", "20".to_string()),
                    ("pn", page.to_string()),
                ])
                .wbi_sign(MIXIN_KEY.load().as_deref())?
                .send()
                .await?
                .error_for_status_ext()?
                .json::<serde_json::Value>()
                .await?
                .validate()?;
            let data = res["data"].take();
            let replies = match data["replies"].as_array() {
                Some(replies) if !replies.is_empty() => replies.clone(),
                _ => break,
            };
            for reply in replies.iter() {
                let info = self.parse_reply(reply)?;
                all.push(info);
            }
        }
        Ok(all)
    }
}

fn normalize_url(url: &str) -> String {
    if url.starts_with("http://") {
        format!("https://{}", &url[7..])
    } else {
        url.to_string()
    }
}
