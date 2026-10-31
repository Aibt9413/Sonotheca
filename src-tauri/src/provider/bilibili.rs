//! 哔哩哔哩音频提取（搜索 → bvid → cid → dash 音频流）。
//!
//! B 站 CDN 强制校验 `Referer: https://www.bilibili.com/`，所以直链必须
//! 走本地代理把 Referer 补上，否则一律 403。

use async_trait::async_trait;
use reqwest::header;
use serde::Serialize;
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::model::source::SourceId;
use crate::model::task::QualityPref;
use crate::provider::{ResolvedStream, SearchHit, SourceAdapter};
use crate::pipeline::download::BROWSER_UA;

const REFERER: &str = "https://www.bilibili.com/";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BiliPage {
    pub cid: String,
    pub page: u32,
    pub title: String,
    pub duration_ms: Option<i64>,
}

pub struct BilibiliAdapter {
    client: reqwest::Client,
}

impl BilibiliAdapter {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// 多 P 支持：track_id 允许 `bvid:cid` 形式，直接指定分 P。
    fn split_id(track_id: &str) -> (&str, Option<&str>) {
        match track_id.split_once(':') {
            Some((bvid, cid)) if !cid.is_empty() => (bvid, Some(cid)),
            _ => (track_id, None),
        }
    }

    /// 视频分 P 列表；单 P 时前端不弹选集。
    pub async fn pages(&self, bvid: &str) -> AppResult<Vec<BiliPage>> {
        let (bvid, _) = Self::split_id(bvid);
        let body = self
            .get_json(&format!(
                "https://api.bilibili.com/x/player/pagelist?bvid={bvid}"
            ))
            .await?;

        Ok(body["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|page| {
                let cid = page["cid"]
                    .as_i64()
                    .map(|value| value.to_string())
                    .or_else(|| page["cid"].as_str().map(str::to_string))?;
                let title = page["part"].as_str().unwrap_or("").trim().to_string();

                Some(BiliPage {
                    cid,
                    page: page["page"].as_u64().unwrap_or(0) as u32,
                    title: if title.is_empty() {
                        format!("P{}", page["page"].as_u64().unwrap_or(0))
                    } else {
                        title
                    },
                    duration_ms: page["duration"].as_i64().map(|seconds| seconds * 1000),
                })
            })
            .collect())
    }

    async fn get_json(&self, url: &str) -> AppResult<Value> {
        let response = self
            .client
            .get(url)
            .header(header::REFERER, REFERER)
            .header(header::USER_AGENT, BROWSER_UA)
            .send()
            .await
            .map_err(|err| AppError::Network(format!("B站请求失败: {err}")))?;

        let body: Value = response
            .json()
            .await
            .map_err(|err| AppError::Network(format!("B站响应解析失败: {err}")))?;

        let code = body["code"].as_i64().unwrap_or(-1);
        if code != 0 {
            return Err(AppError::Network(format!(
                "B站接口错误 code={code} message={}",
                body["message"].as_str().unwrap_or("")
            )));
        }

        Ok(body)
    }

    async fn cid_of(&self, bvid: &str) -> AppResult<String> {
        let (bvid, explicit) = Self::split_id(bvid);
        if let Some(cid) = explicit {
            return Ok(cid.to_string());
        }

        let body = self
            .get_json(&format!(
                "https://api.bilibili.com/x/player/pagelist?bvid={bvid}"
            ))
            .await?;

        body["data"][0]["cid"]
            .as_i64()
            .map(|cid| cid.to_string())
            .ok_or_else(|| AppError::Unavailable)
    }
}

#[async_trait]
impl SourceAdapter for BilibiliAdapter {
    fn source(&self) -> SourceId {
        SourceId::Bilibili
    }

    async fn search(&self, keyword: &str, offset: u32, limit: u32) -> AppResult<Vec<SearchHit>> {
        let page = offset / limit.max(1) + 1;
        let url = format!(
            "https://api.bilibili.com/x/web-interface/search/type?search_type=video&keyword={}&page={page}&page_size={}",
            urlencode(keyword),
            limit.clamp(1, 50)
        );

        let body = self.get_json(&url).await?;
        let items = body["data"]["result"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        Ok(items
            .iter()
            .filter_map(|item| {
                let bvid = item["bvid"].as_str()?.to_string();
                let title = strip_tags(item["title"].as_str().unwrap_or("").trim());
                if title.is_empty() {
                    return None;
                }

                let pic = item["pic"].as_str().map(|url| {
                    if let Some(rest) = url.strip_prefix("//") {
                        format!("https://{rest}")
                    } else {
                        url.to_string()
                    }
                });

                Some(SearchHit {
                    source: SourceId::Bilibili,
                    id: bvid,
                    title,
                    artist: item["author"].as_str().unwrap_or("未知UP主").to_string(),
                    album: "哔哩哔哩".to_string(),
                    cover_url: pic,
                    duration_ms: parse_duration(item["duration"].as_str().unwrap_or("")),
                })
            })
            .collect())
    }

    async fn get_stream_url(
        &self,
        track_id: &str,
        _quality: QualityPref,
    ) -> AppResult<ResolvedStream> {
        let cid = self.cid_of(track_id).await?;
        let (bvid, _) = Self::split_id(track_id);

        // fnval=16 → dash（纯音频流）；拿不到再退回 durl（音视频合流）
        let dash_url = format!(
            "https://api.bilibili.com/x/player/playurl?bvid={bvid}&cid={cid}&fnval=16&fnver=0&fourk=1"
        );

        if let Ok(body) = self.get_json(&dash_url).await {
            let audios = body["data"]["dash"]["audio"].as_array();
            if let Some(audio) = audios.and_then(|list| list.first()) {
                let url = audio["baseUrl"]
                    .as_str()
                    .or_else(|| audio["base_url"].as_str());

                if let Some(url) = url {
                    return Ok(ResolvedStream {
                        url: url.to_string(),
                        referer: Some(REFERER.to_string()),
                    });
                }
            }
        }

        let fallback = format!(
            "https://api.bilibili.com/x/player/playurl?bvid={bvid}&cid={cid}&fnval=1&platform=html5"
        );
        let body = self.get_json(&fallback).await?;
        let url = body["data"]["durl"][0]["url"]
            .as_str()
            .ok_or(AppError::Unavailable)?;

        Ok(ResolvedStream {
            url: url.to_string(),
            referer: Some(REFERER.to_string()),
        })
    }
}

fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn parse_duration(raw: &str) -> Option<i64> {
    let mut total = 0i64;
    for part in raw.split(':') {
        total = total * 60 + part.parse::<i64>().ok()?;
    }
    if total > 0 {
        Some(total * 1000)
    } else {
        None
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
