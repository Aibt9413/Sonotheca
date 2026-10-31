//! Navidrome（标准 Subsonic API）。
//!
//! 认证走 `u/p/v/c/f` 查询参数；`download.view` 本身就是音频流，
//! 直接交给分块下载器即可（无加密、无签名）。

use async_trait::async_trait;
use serde_json::Value;

use crate::db::repo;
use crate::error::{AppError, AppResult};
use crate::model::source::SourceId;
use crate::model::task::QualityPref;
use crate::provider::{ResolvedStream, SearchHit, SourceAdapter};

pub const URL_KEY: &str = "navidrome_url";
pub const USER_KEY: &str = "navidrome_user";
pub const PASS_KEY: &str = "navidrome_password";

#[derive(Clone)]
pub struct NavidromeConfig {
    pub base: String,
    pub user: String,
    pub password: String,
}

impl NavidromeConfig {
    /// 从 settings 表读配置；缺地址视为未配置
    pub fn from_settings(conn: &rusqlite::Connection) -> Option<Self> {
        let base = repo::get_setting(conn, URL_KEY)
            .ok()
            .flatten()?
            .trim()
            .trim_end_matches('/')
            .to_string();

        if base.is_empty() {
            return None;
        }

        let user = repo::get_setting(conn, USER_KEY).ok().flatten().unwrap_or_default();
        let password = repo::get_setting(conn, PASS_KEY)
            .ok()
            .flatten()
            .unwrap_or_default();

        Some(Self {
            base,
            user,
            password,
        })
    }

    fn auth(&self) -> String {
        format!(
            "u={}&p={}&v=1.16.1&c=Sonotheca&f=json",
            urlencode(&self.user),
            urlencode(&self.password)
        )
    }

    pub fn download_url(&self, id: &str) -> String {
        format!(
            "{}/rest/download.view?{}&id={}",
            self.base,
            self.auth(),
            urlencode(id)
        )
    }
}

pub struct NavidromeAdapter {
    client: reqwest::Client,
    config: NavidromeConfig,
}

impl NavidromeAdapter {
    pub fn new(client: reqwest::Client, config: NavidromeConfig) -> Self {
        Self { client, config }
    }

    /// 登录校验：`ping.view` 返回 ok 才认为地址/账号/密码可用
    pub async fn ping(&self) -> AppResult<()> {
        let url = format!("{}/rest/ping.view?{}", self.config.base, self.config.auth());
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|err| AppError::Network(format!("Navidrome 连接失败: {err}")))?;
        let body: Value = response
            .json()
            .await
            .map_err(|err| AppError::Network(format!("Navidrome 响应解析失败: {err}")))?;

        let payload = &body["subsonic-response"];
        if payload["status"].as_str() == Some("ok") {
            Ok(())
        } else {
            Err(AppError::BadInput(format!(
                "登录失败：{}",
                payload["error"]["message"].as_str().unwrap_or("请检查地址、用户名与密码")
            )))
        }
    }
}

#[async_trait]
impl SourceAdapter for NavidromeAdapter {
    fn source(&self) -> SourceId {
        SourceId::Navidrome
    }

    async fn search(&self, keyword: &str, _offset: u32, limit: u32) -> AppResult<Vec<SearchHit>> {
        let url = format!(
            "{}/rest/search3.view?{}&query={}&songCount={}&artistCount=0&albumCount=0",
            self.config.base,
            self.config.auth(),
            urlencode(keyword),
            limit.clamp(1, 100)
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|err| AppError::Network(format!("Navidrome 请求失败: {err}")))?;

        let body: Value = response
            .json()
            .await
            .map_err(|err| AppError::Network(format!("Navidrome 响应解析失败: {err}")))?;

        let payload = &body["subsonic-response"];
        if payload["status"].as_str() != Some("ok") {
            let message = payload["error"]["message"].as_str().unwrap_or("未知错误");
            return Err(AppError::Network(format!("Navidrome 返回错误: {message}")));
        }

        let songs = songs_of(&payload["searchResult3"]["song"]);

        Ok(songs
            .iter()
            .filter_map(|song| {
                let id = song["id"].as_str()?.to_string();
                let title = song["title"].as_str().unwrap_or("").trim().to_string();
                if title.is_empty() {
                    return None;
                }

                Some(SearchHit {
                    source: SourceId::Navidrome,
                    id,
                    title,
                    artist: song["artist"]
                        .as_str()
                        .or_else(|| song["albumArtist"].as_str())
                        .unwrap_or("未知歌手")
                        .to_string(),
                    album: song["album"].as_str().unwrap_or("").to_string(),
                    // 封面接口同样要带密码，且多为 http，前端 CSP 不放行；先留空
                    cover_url: None,
                    duration_ms: song["duration"].as_i64().map(|sec| sec * 1000),
                })
            })
            .collect())
    }

    async fn get_stream_url(
        &self,
        track_id: &str,
        _quality: QualityPref,
    ) -> AppResult<ResolvedStream> {
        Ok(ResolvedStream {
            url: self.config.download_url(track_id),
            referer: None,
        })
    }
}

/// Subsonic 的数组字段在"只有一个元素"时可能退化成对象
fn songs_of(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(list) => list.clone(),
        Value::Object(_) => vec![value.clone()],
        _ => Vec::new(),
    }
}

fn urlencode(input: &str) -> String {
    input
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
