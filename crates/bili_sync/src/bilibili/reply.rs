use std::collections::{HashSet, VecDeque};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::{Duration, sleep};

use crate::bilibili::{BiliClient, Credential, ErrorForStatusExt, MIXIN_KEY, Validate, WbiSign};

/// 评论接口请求间隔，风控敏感接口，保守节流
const REPLY_REQUEST_INTERVAL: Duration = Duration::from_millis(400);
const REPLY_PAGINATION_VERSION: u8 = 2;

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
    /// 是否仍被 B 站标记为有效；本地历史记录即使失效也保留
    pub valid: bool,
    /// 原始 JSON
    pub raw: Value,
    /// 楼中楼回复
    pub sub_replies: Vec<ReplyInfo>,
}

pub struct Reply<'a> {
    client: &'a BiliClient,
    credential: &'a Credential,
    #[cfg(test)]
    test_endpoint: Option<String>,
}

impl<'a> Reply<'a> {
    pub fn new(client: &'a BiliClient, credential: &'a Credential) -> Self {
        Self {
            client,
            credential,
            #[cfg(test)]
            test_endpoint: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(client: &'a BiliClient, credential: &'a Credential, endpoint: String) -> Self {
        Self {
            client,
            credential,
            test_endpoint: Some(endpoint),
        }
    }

    /// 每次只请求一页；调用方在评论和断点同时落库后再请求下一页。
    /// 所有请求沿用上游 BiliClient 的凭据、全局限流、WBI 签名及错误检查。
    pub async fn get_next_page(
        &self,
        comment_type: i64,
        oid: &str,
        state: &ReplySyncState,
    ) -> Result<(Vec<ReplyInfo>, ReplySyncState)> {
        ensure!(!state.is_complete(), "reply scan already complete");
        #[cfg(not(test))]
        sleep(REPLY_REQUEST_INTERVAL).await;
        #[cfg(test)]
        if self.test_endpoint.is_none() {
            sleep(REPLY_REQUEST_INTERVAL).await;
        }
        let (url, query) = if let Some(sub) = state.pending_sub.front() {
            (
                "https://api.bilibili.com/x/v2/reply/reply",
                vec![
                    ("type", comment_type.to_string()),
                    ("oid", oid.to_string()),
                    ("root", sub.root.to_string()),
                    ("ps", "20".to_string()),
                    ("pn", sub.next_page.to_string()),
                ],
            )
        } else {
            (
                "https://api.bilibili.com/x/v2/reply/wbi/main",
                main_query(comment_type, oid, state.next_offset.as_ref(), state.next_cursor),
            )
        };
        #[cfg(test)]
        let url = self.test_endpoint.as_deref().unwrap_or(url);
        let mut res = self
            .client
            .request(Method::GET, url, self.credential)
            .await
            .query(&query)
            .wbi_sign(MIXIN_KEY.load().as_deref())?
            .send()
            .await?
            .error_for_status_ext()?
            .json::<Value>()
            .await?
            .validate()?;
        let data = res["data"].take();
        let mut next_state = state.clone();
        let replies = if state.pending_sub.is_empty() {
            next_state.apply_main_page(&data)?
        } else {
            next_state.apply_sub_page(&data)?
        };
        Ok((replies, next_state))
    }

    /// 解析单条评论
    fn parse_reply(reply: &Value) -> Result<ReplyInfo> {
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
            valid: true,
            raw: reply.clone(),
            sub_replies: Vec::new(),
        })
    }
}

fn normalize_url(url: &str) -> String {
    url.strip_prefix("http://")
        .map(|rest| format!("https://{rest}"))
        .unwrap_or_else(|| url.to_string())
}

fn main_query(
    comment_type: i64,
    oid: &str,
    offset: Option<&Value>,
    next_cursor: Option<u64>,
) -> Vec<(&'static str, String)> {
    let mut query = vec![
        ("type", comment_type.to_string()),
        ("oid", oid.to_string()),
        ("mode", "3".to_string()),
        ("ps", "20".to_string()),
    ];
    // 不混用两种分页协议：空 pagination_str 会让 next=0 进入不推进的 session 分支。
    // 首页只传 next=0，后续优先使用服务端的 next；没有数值游标的旧响应使用 offset。
    match (next_cursor, offset) {
        (Some(next), _) => query.push(("next", next.to_string())),
        (None, Some(offset)) => query.push(("pagination_str", json!({ "offset": offset }).to_string())),
        (None, None) => query.push(("next", "0".into())),
    }
    query
}

/// 一个完整评论扫描周期的断点；跨轮次保留 seen_ids，只有完整结束才清理失效评论。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplySyncState {
    #[serde(default)]
    pagination_version: u8,
    next_offset: Option<Value>,
    #[serde(default)]
    next_cursor: Option<u64>,
    main_finished: bool,
    pending_sub: VecDeque<SubReplyCursor>,
    seen_roots: HashSet<i64>,
    seen_ids: HashSet<i64>,
    used_offsets: HashSet<String>,
}

impl Default for ReplySyncState {
    fn default() -> Self {
        Self {
            pagination_version: REPLY_PAGINATION_VERSION,
            next_offset: None,
            next_cursor: None,
            main_finished: false,
            pending_sub: VecDeque::new(),
            seen_roots: HashSet::new(),
            seen_ids: HashSet::new(),
            used_offsets: HashSet::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SubReplyCursor {
    root: i64,
    next_page: u64,
}

impl ReplySyncState {
    /// 旧版混用分页参数，已保存的主评论游标可能无法推进。
    /// 只重新开始尚未结束的主评论扫描；已完成的主评论及楼中楼断点继续保留。
    pub fn restart_legacy_pagination(&mut self) -> bool {
        if self.pagination_version < REPLY_PAGINATION_VERSION && !self.main_finished && self.next_offset.is_some() {
            *self = Self::default();
            return true;
        }
        self.pagination_version = REPLY_PAGINATION_VERSION;
        false
    }

    pub fn is_complete(&self) -> bool {
        self.main_finished && self.pending_sub.is_empty()
    }

    pub fn seen_ids(&self) -> Vec<i64> {
        self.seen_ids.iter().copied().collect()
    }

    pub fn progress_description(&self) -> String {
        format!(
            "已发现 {} 条评论，待处理楼中楼 {} 个",
            self.seen_ids.len(),
            self.pending_sub.len()
        )
    }

    fn apply_main_page(&mut self, data: &Value) -> Result<Vec<ReplyInfo>> {
        let cursor = data["cursor"].as_object().context("missing reply cursor")?;
        ensure!(
            cursor.contains_key("is_end") || cursor.contains_key("pagination_reply") || cursor.contains_key("next"),
            "invalid reply cursor"
        );
        let is_end = cursor
            .get("is_end")
            .map(|value| value.as_bool().context("invalid reply end flag"))
            .transpose()?;
        let offset = &data["cursor"]["pagination_reply"]["next_offset"];
        let has_offset = !offset.is_null() && offset.as_str() != Some("");
        let next_cursor = cursor
            .get("next")
            .map(|value| value.as_u64().context("invalid numeric reply cursor"))
            .transpose()?;
        // 新接口以 is_end 为准：末页可能仍带 next_offset；兼容旧接口没有 is_end 的响应。
        let reached_end = is_end == Some(true) || (is_end.is_none() && !has_offset && next_cursor.is_none());
        ensure!(
            reached_end || has_offset || next_cursor.is_some(),
            "reply cursor has not ended but next_offset is missing"
        );
        let mut replies = Vec::new();
        if let Some(items) = data["replies"].as_array() {
            for item in items {
                let info = Reply::parse_reply(item)?;
                if self.seen_roots.insert(info.rpid) {
                    // rcount 明确为 0 时无需再为这一条评论额外发起楼中楼请求。
                    if item["rcount"].as_u64() != Some(0) {
                        self.pending_sub.push_back(SubReplyCursor {
                            root: info.rpid,
                            next_page: 1,
                        });
                    }
                    self.seen_ids.insert(info.rpid);
                    replies.push(info);
                }
            }
        } else {
            ensure!(data["replies"].is_null(), "invalid reply list");
        }
        ensure!(
            reached_end || !replies.is_empty(),
            "reply pagination stalled: page contains no new comments"
        );
        let current_cursor = match self.next_cursor {
            Some(next) => format!("next:{next}"),
            None => self
                .next_offset
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_else(|| "next:0".into()),
        };
        self.used_offsets.insert(current_cursor);
        if !reached_end {
            let next_key = next_cursor
                .map(|next| format!("next:{next}"))
                .unwrap_or_else(|| offset.to_string());
            ensure!(
                !self.used_offsets.contains(&next_key),
                "reply pagination stalled: repeated cursor"
            );
            self.next_offset = has_offset.then(|| offset.clone());
            self.next_cursor = next_cursor;
        }
        self.main_finished = reached_end;
        Ok(replies)
    }

    fn apply_sub_page(&mut self, data: &Value) -> Result<Vec<ReplyInfo>> {
        let sub = self.pending_sub.front().context("missing sub reply cursor")?;
        let root = sub.root;
        let page = sub.next_page;
        let pagination = data["page"].as_object().context("missing sub reply pagination")?;
        if let Some(number) = pagination.get("num").and_then(Value::as_u64) {
            ensure!(
                number == page,
                "sub reply pagination returned page {number} instead of {page}"
            );
        }
        let items = match &data["replies"] {
            Value::Array(items) => items.as_slice(),
            Value::Null => &[],
            _ => anyhow::bail!("invalid sub reply list"),
        };
        let size = data["page"]["size"].as_u64().filter(|size| *size > 0).unwrap_or(20);
        let reached_end = match data["page"]["count"].as_u64() {
            Some(count) => page.saturating_mul(size) >= count,
            None => items.len() < size as usize,
        };
        let mut replies = Vec::new();
        for item in items {
            let mut info = Reply::parse_reply(item)?;
            info.parent_rpid = info.parent_rpid.or(Some(root));
            if self.seen_ids.insert(info.rpid) {
                replies.push(info);
            }
        }
        ensure!(
            items.is_empty() || !replies.is_empty(),
            "sub reply pagination repeated a page for root {root}"
        );
        ensure!(
            reached_end || !replies.is_empty(),
            "sub reply pagination stalled for root {root}"
        );
        if reached_end {
            self.pending_sub.pop_front();
        } else {
            self.pending_sub.front_mut().unwrap().next_page += 1;
        }
        Ok(replies)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn reply_value(rpid: i64, children: u64) -> Value {
        json!({"rpid": rpid, "ctime": 1, "rcount": children, "content": {"message": "test"}})
    }

    pub(crate) fn main_page(ids: &[i64], is_end: bool, offset: &str) -> Value {
        json!({"cursor": {"is_end": is_end, "pagination_reply": {"next_offset": offset}},
            "replies": ids.iter().map(|id| reply_value(*id, 0)).collect::<Vec<_>>()})
    }

    #[test]
    fn pagination_query_uses_exactly_one_cursor_protocol() -> Result<()> {
        for (offset, next_cursor) in [
            (None, None),
            (Some(json!("next-page")), None),
            (Some(json!({"offset": 123})), None),
            (Some(json!("ignored-offset")), Some(2)),
        ] {
            let req = crate::bilibili::Client::new()
                .request(Method::GET, "https://api.bilibili.com/x/v2/reply/wbi/main", None)
                .query(&main_query(1, "123", offset.as_ref(), next_cursor))
                .build()?;
            let params: Vec<_> = req
                .url()
                .query_pairs()
                .filter(|(key, _)| key == "pagination_str" || key == "next")
                .collect();
            assert_eq!(params.len(), 1);
            let (key, value) = match (next_cursor, offset) {
                (Some(next), _) => ("next", next.to_string()),
                (None, Some(offset)) => ("pagination_str", json!({"offset": offset}).to_string()),
                (None, None) => ("next", "0".into()),
            };
            assert_eq!(params[0].0, key);
            assert_eq!(params[0].1, value);
        }
        Ok(())
    }

    #[test]
    fn explicit_end_wins_over_nonempty_next_offset() -> Result<()> {
        let mut state = ReplySyncState::default();
        assert_eq!(
            state.apply_main_page(&main_page(&[1, 2], true, "still-present"))?.len(),
            2
        );
        assert!(state.is_complete());
        assert!(state.pending_sub.is_empty());
        Ok(())
    }

    #[test]
    fn checkpoint_resumes_main_page_and_retains_seen_comments() -> Result<()> {
        let mut state = ReplySyncState::default();
        state.apply_main_page(&main_page(&[1], false, "page-two"))?;
        assert!(!state.is_complete());
        let mut resumed: ReplySyncState = serde_json::from_value(serde_json::to_value(state)?)?;
        assert_eq!(resumed.next_offset, Some(json!("page-two")));
        resumed.apply_main_page(&main_page(&[2], true, "unused"))?;
        assert!(resumed.is_complete());
        assert_eq!(resumed.seen_ids, HashSet::from([1, 2]));
        Ok(())
    }

    #[test]
    fn repeated_cursor_and_repeated_page_stop_without_claiming_completion() -> Result<()> {
        let mut state = ReplySyncState::default();
        state.apply_main_page(&main_page(&[1], false, "same"))?;
        let mut next = state.clone();
        assert!(next.apply_main_page(&main_page(&[2], false, "same")).is_err());
        let mut next = state.clone();
        assert!(next.apply_main_page(&main_page(&[1], false, "different")).is_err());
        assert!(!state.is_complete());
        assert_eq!(state.seen_ids, HashSet::from([1]));
        Ok(())
    }

    #[test]
    fn legacy_session_cursor_restarts_but_new_and_sub_reply_checkpoints_are_preserved() -> Result<()> {
        let mut current = ReplySyncState::default();
        current.apply_main_page(&main_page(&[1], false, "session-offset"))?;
        assert!(!current.restart_legacy_pagination());
        assert_eq!(current.next_offset, Some(json!("session-offset")));
        let mut old_json = serde_json::to_value(&current)?;
        old_json.as_object_mut().unwrap().remove("pagination_version");
        let mut legacy: ReplySyncState = serde_json::from_value(old_json)?;
        assert!(legacy.restart_legacy_pagination());
        assert!(legacy.next_offset.is_none());
        assert!(legacy.seen_ids.is_empty());
        let mut sub = ReplySyncState::default();
        sub.apply_main_page(&json!({"cursor": {"is_end": true}, "replies": [reply_value(1, 100)]}))?;
        let mut old_json = serde_json::to_value(&sub)?;
        old_json.as_object_mut().unwrap().remove("pagination_version");
        let mut legacy: ReplySyncState = serde_json::from_value(old_json)?;
        assert!(!legacy.restart_legacy_pagination());
        assert!(legacy.main_finished);
        assert_eq!(legacy.pending_sub.front().unwrap().root, 1);
        assert_eq!(legacy.seen_ids, HashSet::from([1]));
        let mut old_json = serde_json::to_value(&current)?;
        old_json["pagination_version"] = json!(1);
        let mut legacy: ReplySyncState = serde_json::from_value(old_json)?;
        assert!(legacy.restart_legacy_pagination());
        Ok(())
    }

    #[test]
    fn numeric_cursor_survives_restart_and_is_checked_instead_of_unused_offset() -> Result<()> {
        let mut state = ReplySyncState::default();
        let mut first = main_page(&[1], false, "same-unused-offset");
        first["cursor"]["next"] = json!(2);
        state.apply_main_page(&first)?;
        let mut resumed: ReplySyncState = serde_json::from_value(serde_json::to_value(state)?)?;
        assert_eq!(resumed.next_cursor, Some(2));
        let query = main_query(1, "123", resumed.next_offset.as_ref(), resumed.next_cursor);
        assert!(query.contains(&("next", "2".into())));
        assert!(!query.iter().any(|(key, _)| *key == "pagination_str"));
        let mut second = main_page(&[2], false, "same-unused-offset");
        second["cursor"]["next"] = json!(3);
        resumed.apply_main_page(&second)?;
        assert_eq!(resumed.next_cursor, Some(3));
        let mut repeated = main_page(&[3], false, "different-unused-offset");
        repeated["cursor"]["next"] = json!(3);
        assert!(resumed.clone().apply_main_page(&repeated).is_err());
        Ok(())
    }

    #[test]
    fn explicit_not_finished_requires_a_cursor_and_a_nonempty_page() {
        let mut state = ReplySyncState::default();
        assert!(state.apply_main_page(&main_page(&[1], false, "")).is_err());
        let mut state = ReplySyncState::default();
        assert!(state.apply_main_page(&main_page(&[], false, "page-two")).is_err());
        assert!(!state.is_complete());
    }

    #[test]
    fn old_cursor_without_end_flag_is_supported() -> Result<()> {
        let mut state = ReplySyncState::default();
        let mut data = main_page(&[1], true, "");
        data["cursor"].as_object_mut().unwrap().remove("is_end");
        state.apply_main_page(&data)?;
        assert!(state.is_complete());
        Ok(())
    }

    #[test]
    fn malformed_responses_cannot_mark_historical_comments_missing() -> Result<()> {
        let mut state = ReplySyncState::default();
        assert!(state.apply_main_page(&json!({"cursor": {}, "replies": null})).is_err());
        let mut data = main_page(&[], true, "");
        data["replies"] = json!([reply_value(1, 1)]);
        state.apply_main_page(&data)?;
        assert!(state.apply_sub_page(&json!({"replies": null})).is_err());
        assert!(!state.is_complete());
        Ok(())
    }

    #[test]
    fn complete_main_scan_waits_for_all_sub_pages_across_restart() -> Result<()> {
        let mut state = ReplySyncState::default();
        let mut data = main_page(&[], true, "unused");
        data["replies"] = json!([reply_value(1, 40)]);
        state.apply_main_page(&data)?;
        assert!(!state.is_complete());
        let replies: Vec<_> = (2..22).map(|id| reply_value(id, 0)).collect();
        state.apply_sub_page(&json!({"page": {"size": 20, "count": 40}, "replies": replies}))?;
        let mut resumed: ReplySyncState = serde_json::from_value(serde_json::to_value(state)?)?;
        assert_eq!(resumed.pending_sub.front().unwrap().next_page, 2);
        let replies: Vec<_> = (22..42).map(|id| reply_value(id, 0)).collect();
        let sub = resumed.apply_sub_page(&json!({"page": {"size": 20, "count": 40}, "replies": replies}))?;
        assert!(sub.iter().all(|reply| reply.parent_rpid == Some(1)));
        assert!(resumed.is_complete());
        assert_eq!(resumed.seen_ids.len(), 41);
        Ok(())
    }

    #[test]
    fn repeated_sub_page_does_not_loop() -> Result<()> {
        let mut state = ReplySyncState::default();
        let mut data = main_page(&[], true, "");
        data["replies"] = json!([reply_value(1, 40)]);
        state.apply_main_page(&data)?;
        let replies: Vec<_> = (2..22).map(|id| reply_value(id, 0)).collect();
        let page = json!({"page": {"size": 20, "count": 40}, "replies": replies});
        state.apply_sub_page(&page)?;
        // 即使错误返回的最后一页声称结束，也不能把同一页当作完整的楼中楼。
        assert!(state.clone().apply_sub_page(&page).is_err());
        assert!(!state.is_complete());
        Ok(())
    }
}
