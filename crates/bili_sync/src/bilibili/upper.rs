use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

use crate::bilibili::{BiliClient, Credential, ErrorForStatusExt, MIXIN_KEY, Validate, WbiSign};

/// UP 主账号信息快照
#[derive(Debug, Clone)]
pub struct UpperProfile {
    pub name: String,
    pub sign: String,
    pub face: String,
    pub fan_count: i64,
    pub follow_count: i64,
    pub video_count: i64,
    pub view_count: i64,
    pub like_count: i64,
}

pub struct UpperInfo<'a> {
    client: &'a BiliClient,
    pub upper_id: String,
    credential: &'a Credential,
}

impl<'a> UpperInfo<'a> {
    pub fn new(client: &'a BiliClient, upper_id: String, credential: &'a Credential) -> Self {
        Self {
            client,
            upper_id,
            credential,
        }
    }

    /// 获取 UP 主账号信息：名字、签名、头像、粉丝数、关注数、投稿数、总播放数、总获赞数
    pub async fn get_profile(&self) -> Result<UpperProfile> {
        let (card, upstat, arc_search) = tokio::try_join!(
            self.get_card(),
            self.get_upstat(),
            self.get_arc_search()
        )?;
        let card = card["data"]["card"].clone();
        Ok(UpperProfile {
            name: card["name"].as_str().unwrap_or_default().to_string(),
            sign: card["sign"].as_str().unwrap_or_default().to_string(),
            face: card["face"].as_str().unwrap_or_default().to_string(),
            fan_count: card["fans"].as_i64().unwrap_or(0),
            follow_count: card["attention"].as_i64().unwrap_or(0),
            video_count: arc_search["data"]["page"]["count"].as_i64().unwrap_or(0),
            view_count: upstat["data"]["archive"]["view"].as_i64().unwrap_or(0),
            like_count: upstat["data"]["likes"].as_i64().unwrap_or(0),
        })
    }

    /// 账号基本信息（无需 wbi 签名）
    async fn get_card(&self) -> Result<Value> {
        self.client
            .request(
                Method::GET,
                "https://api.bilibili.com/x/web-interface/card",
                self.credential,
            )
            .await
            .query(&[("mid", self.upper_id.as_str())])
            .send()
            .await?
            .error_for_status_ext()?
            .json::<serde_json::Value>()
            .await?
            .validate()
    }

    /// 总播放数（无需 wbi 签名）
    async fn get_upstat(&self) -> Result<Value> {
        self.client
            .request(
                Method::GET,
                "https://api.bilibili.com/x/space/upstat",
                self.credential,
            )
            .await
            .query(&[("mid", self.upper_id.as_str())])
            .send()
            .await?
            .error_for_status_ext()?
            .json::<serde_json::Value>()
            .await?
            .validate()
    }

    /// 投稿数（需要 wbi 签名）
    async fn get_arc_search(&self) -> Result<Value> {
        self.client
            .request(
                Method::GET,
                "https://api.bilibili.com/x/space/wbi/arc/search",
                self.credential,
            )
            .await
            .query(&[
                ("mid", self.upper_id.as_str()),
                ("ps", "1"),
                ("pn", "1"),
                ("platform", "web"),
                ("web_location", "1550101"),
            ])
            .wbi_sign(MIXIN_KEY.load().as_deref())?
            .send()
            .await?
            .error_for_status_ext()?
            .json::<serde_json::Value>()
            .await?
            .validate()
    }
}
