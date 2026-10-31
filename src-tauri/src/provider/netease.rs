//! 网易云适配器（M6：登录 + 六档音质 + 双通道）。
//!
//! ## 通道选择
//!
//! `ncm-api-rs` 默认用 **eapi**（`interface.music.163.com`）。实测部分运营商网络会
//! RST 这个路径，而 **weapi 正常**。所以这里统一走 `CryptoType::Weapi`：
//! `request()` 会把 `/api/xxx` 重写成 `/weapi/xxx`，鉴权、设备指纹、cookie
//! 仍由 SDK 处理，不必自己实现加密。
//!
//! ## 音质
//!
//! SVIP 档位（Hi-Res / 超清母带 / 沉浸声）在移动端 UA 下会被降档，所以所有
//! weapi 请求统一带 PC 端 UA（[`PC_UA`]）。拿不到目标档位时逐级降级：
//! 目标档 → 极高 320K → 免签名 128K 直链。
//!
//! 搜索额外保留一条明文回退（`music.163.com/api/search/get`），只在 weapi 连接
//! 失败时启用。

use std::sync::RwLock;

use futures_util::StreamExt;
use ncm_api_rs::{create_client, ApiClient, ApiResponse, CryptoType, RequestOption};
use reqwest::header;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{ResolvedStream, SearchHit, SourceAdapter};
use crate::error::{AppError, AppResult};
use crate::model::source::SourceId;
use crate::model::task::QualityPref;
use crate::pipeline::download::BROWSER_UA;
use crate::pipeline::magic;

const REFERER: &str = "https://music.163.com/";
const PLAIN_SEARCH: &str = "https://music.163.com/api/search/get";
const SONG_DETAIL: &str = "https://music.163.com/api/song/detail";
const OUTER_URL: &str = "https://music.163.com/song/media/outer/url";

/// PC 端 UA：SVIP 高音质在移动端 UA 下会被降档，用 PC 端更稳
pub const PC_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NeteaseProfile {
    pub user_id: i64,
    pub nickname: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeteasePlaylist {
    pub id: String,
    pub name: String,
    pub cover_url: Option<String>,
    pub total: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QrState {
    /// 800 过期 / 801 待扫码 / 802 已扫码待确认 / 803 成功
    pub code: i64,
    pub message: String,
}

/// 实际拿到的档位低于请求档位时的原因，前端据此选 Toast 文案与颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityFallback {
    /// 未登录：无损及以上 → 极高
    Anonymous,
    /// 母带 / 沉浸声 / Hi-Res 拿不到 → 无损
    Lossless,
    /// 目标档不可用 → 极高 320K
    Exhigh,
    /// 只有免签名 128K 可用
    Standard,
}

/// 解析结果：直链 + 实际档位 + 降级原因
#[derive(Debug, Clone)]
pub struct ResolvedStreamInfo {
    pub stream: ResolvedStream,
    pub quality: QualityPref,
    pub fallback: Option<QualityFallback>,
}

pub struct NeteaseAdapter {
    sdk: ApiClient,
    http: reqwest::Client,
    /// 专用于探测直链：必须跟随 302（`http` 为了识别 outer/url 的跳转是禁跟随的）
    probe: reqwest::Client,
    cookie: RwLock<Option<String>>,
}

impl NeteaseAdapter {
    pub fn new(cookie: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(BROWSER_UA)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("netease http client");

        let probe = reqwest::Client::builder()
            .user_agent(PC_UA)
            .redirect(reqwest::redirect::Policy::limited(5))
            .connect_timeout(std::time::Duration::from_secs(8))
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("netease probe client");

        Self {
            sdk: create_client(None),
            http,
            probe,
            cookie: RwLock::new(cookie),
        }
    }

    pub fn cookie(&self) -> Option<String> {
        self.cookie.read().ok().and_then(|guard| guard.clone())
    }

    pub fn set_cookie(&self, value: Option<String>) {
        if let Ok(mut guard) = self.cookie.write() {
            *guard = value;
        }
    }

    pub fn is_logged_in(&self) -> bool {
        self.cookie().is_some()
    }

    /// 未登录时高音质档位拿不到，统一降到极高 320K。
    ///
    /// 返回（实际档位, 是否降级）。ipc 与 provider 共用这一条规则，
    /// 保证任务快照里记的音质与实际请求一致，前端据此弹 Toast。
    pub fn effective_quality(&self, requested: QualityPref) -> (QualityPref, bool) {
        if requested.needs_login() && !self.is_logged_in() {
            (QualityPref::Exhigh, true)
        } else {
            (requested, false)
        }
    }

    async fn weapi_raw(&self, path: &str, data: Value) -> AppResult<ApiResponse> {
        let options = RequestOption {
            crypto: CryptoType::Weapi,
            cookie: self.cookie(),
            // 关键：PC 端 UA，否则 SVIP 档位会被降档
            ua: Some(PC_UA.to_string()),
            ..Default::default()
        };

        self.sdk
            .request(path, data, options)
            .await
            .map_err(|err| AppError::Network(format!("网易云请求失败: {}", error_chain(&err))))
    }

    async fn weapi(&self, path: &str, data: Value) -> AppResult<Value> {
        Ok(self.weapi_raw(path, data).await?.body)
    }

    // ───────────────────────── 登录 ─────────────────────────

    /// 申请扫码登录的 unikey；前端据此渲染二维码内容
    pub async fn qr_key(&self) -> AppResult<String> {
        let body = self.weapi("/api/login/qrcode/unikey", json!({ "type": 1 })).await?;
        body["unikey"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| AppError::Network("未取到二维码 unikey".to_string()))
    }

    /// 轮询扫码状态；803 时把 Set-Cookie 存下来
    pub async fn qr_check(&self, key: &str) -> AppResult<QrState> {
        let response = self
            .weapi_raw("/api/login/qrcode/client/login", json!({ "key": key, "type": 1 }))
            .await?;

        let code = response.body["code"].as_i64().unwrap_or(0);
        let message = response.body["message"].as_str().unwrap_or("").to_string();

        if code == 803 {
            let cookie = cookies_to_header(&response.cookie);
            if !cookie.is_empty() {
                self.set_cookie(Some(cookie));
            }
        }

        Ok(QrState { code, message })
    }

    pub async fn profile(&self) -> AppResult<Option<NeteaseProfile>> {        if !self.is_logged_in() {
            return Ok(None);
        }

        let body = self.weapi("/api/w/nuser/account/get", json!({})).await?;
        let profile = &body["profile"];

        if profile.is_null() {
            return Ok(None);
        }

        Ok(Some(NeteaseProfile {
            user_id: profile["userId"].as_i64().unwrap_or(0),
            nickname: profile["nickname"].as_str().unwrap_or("").to_string(),
            avatar_url: profile["avatarUrl"].as_str().unwrap_or("").to_string(),
        }))
    }

    /// 当前登录用户创建/收藏的歌单；先走 /user/account 拿 UID。
    pub async fn my_playlists(&self) -> AppResult<Vec<NeteasePlaylist>> {
        if !self.is_logged_in() {
            return Err(AppError::BadInput("网易云未登录".to_string()));
        }

        let account = self.weapi("/api/w/nuser/account/get", json!({})).await?;
        let uid = account["account"]["id"]
            .as_i64()
            .or_else(|| account["profile"]["userId"].as_i64())
            .unwrap_or(0);
        if uid == 0 {
            return Err(AppError::BadInput("网易云账号信息不可用".to_string()));
        }

        let body = self
            .weapi(
                "/api/user/playlist",
                json!({ "uid": uid, "limit": 100, "offset": 0, "includeVideo": false }),
            )
            .await?;

        Ok(body["playlist"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| {
                let id = item["id"]
                    .as_i64()
                    .map(|value| value.to_string())
                    .or_else(|| item["id"].as_str().map(str::to_string))?;
                let name = item["name"].as_str().unwrap_or("").to_string();
                if name.is_empty() {
                    return None;
                }

                Some(NeteasePlaylist {
                    id,
                    name,
                    cover_url: item["coverImgUrl"]
                        .as_str()
                        .or_else(|| item["coverUrl"].as_str())
                        .map(str::to_string),
                    total: item["trackCount"].as_u64().unwrap_or(0) as u32,
                })
            })
            .collect())
    }

    /// 取 LRC 歌词。纯音乐 / 无歌词时返回空串（前端显示"暂无歌词"）。
    pub async fn lyric(&self, track_id: &str) -> AppResult<String> {
        let body = self
            .weapi(
                "/api/song/lyric",
                json!({ "id": track_id, "lv": -1, "kv": -1, "tv": -1 }),
            )
            .await?;

        Ok(body["lrc"]["lyric"].as_str().unwrap_or("").to_string())
    }

    // ───────────────────────── 明文回退 ─────────────────────────

    async fn plain_search(&self, keyword: &str, offset: u32, limit: u32) -> AppResult<Vec<SearchHit>> {
        let response = self
            .http
            .get(PLAIN_SEARCH)
            .query(&[
                ("s", keyword.to_string()),
                ("type", "1".to_string()),
                ("limit", limit.to_string()),
                ("offset", offset.to_string()),
            ])
            .header(header::REFERER, REFERER)
            .send()
            .await
            .map_err(|err| AppError::Network(format!("网易云搜索(明文)失败: {}", error_chain(&err))))?;

        if !response.status().is_success() {
            return Err(AppError::Unavailable);
        }

        let body: Value = response
            .json()
            .await
            .map_err(|err| AppError::Network(format!("网易云搜索(明文)解析失败: {err}")))?;

        let songs = body["result"]["songs"].as_array().cloned().unwrap_or_default();
        let mut hits: Vec<SearchHit> = songs.iter().map(parse_song).collect();

        // 明文接口不返回封面，批量查详情补齐
        if hits.iter().any(|hit| hit.cover_url.is_none()) {
            let ids: Vec<String> = hits
                .iter()
                .map(|hit| hit.id.clone())
                .filter(|id| !id.is_empty())
                .collect();

            if let Ok(details) = self.song_details(&ids).await {
                let mut by_id: std::collections::HashMap<String, SearchHit> =
                    details.into_iter().map(|d| (d.id.clone(), d)).collect();

                for hit in hits.iter_mut() {
                    if let Some(detail) = by_id.remove(&hit.id) {
                        if hit.cover_url.is_none() {
                            hit.cover_url = detail.cover_url;
                        }
                        if hit.album.is_empty() {
                            hit.album = detail.album;
                        }
                        if hit.duration_ms.is_none() {
                            hit.duration_ms = detail.duration_ms;
                        }
                    }
                }
            }
        }

        Ok(hits)
    }

    async fn song_details(&self, ids: &[String]) -> AppResult<Vec<SearchHit>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let param = format!("[{}]", ids.join(","));
        let response = self
            .http
            .get(SONG_DETAIL)
            .query(&[("ids", param)])
            .header(header::REFERER, REFERER)
            .send()
            .await
            .map_err(|err| AppError::Network(format!("查询歌曲详情失败: {}", error_chain(&err))))?;

        if !response.status().is_success() {
            return Err(AppError::Unavailable);
        }

        let body: Value = response
            .json()
            .await
            .map_err(|err| AppError::Network(format!("歌曲详情解析失败: {err}")))?;

        Ok(body["songs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(parse_song)
            .collect())
    }

    async fn plain_stream_url(&self, track_id: &str) -> AppResult<ResolvedStream> {
        let url = format!("{OUTER_URL}?id={track_id}.mp3");

        let response = self
            .http
            .get(&url)
            .header(header::REFERER, REFERER)
            .send()
            .await
            .map_err(|err| AppError::Network(format!("获取直链(明文)失败: {}", error_chain(&err))))?;

        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(AppError::Unavailable)?;

            // 不可用的歌会被 302 到 /404；那不是音频，必须当场拒绝
            if location.contains("music.163.com/404") || !location.starts_with("http") {
                return Err(AppError::Unavailable);
            }

            return Ok(ResolvedStream {
                url: location.to_string(),
                referer: Some(REFERER.to_string()),
            });
        }

        if response.status().is_success() {
            return Ok(ResolvedStream {
                url,
                referer: Some(REFERER.to_string()),
            });
        }

        Err(AppError::Unavailable)
    }

    /// 解析直链：目标档 → 无损 → 极高 → 免签名 128K。
    ///
    /// 每一层都先用 Range 读 16 字节做魔数校验：网易云不可用时会把
    /// outer/url 302 到 `music.163.com/404`，跟着下载只会得到 107KB 的 HTML。
    pub async fn resolve_stream(
        &self,
        track_id: &str,
        requested: QualityPref,
    ) -> AppResult<ResolvedStreamInfo> {
        let (target, login_degraded) = self.effective_quality(requested);

        let mut chain = vec![target];
        match target {
            QualityPref::Jymaster | QualityPref::Sky | QualityPref::HiRes => {
                chain.push(QualityPref::Lossless);
                chain.push(QualityPref::Exhigh);
            }
            QualityPref::Lossless => chain.push(QualityPref::Exhigh),
            _ => {}
        }

        for candidate in chain {
            let url = match self.player_url(track_id, candidate).await {
                Ok(Some(url)) => url,
                Ok(None) => {
                    eprintln!("[sonotheca] {} 无可用直链", candidate.label());
                    continue;
                }
                Err(err) => {
                    eprintln!("[sonotheca] {} 解析失败: {err}", candidate.label());
                    continue;
                }
            };

            if self.probe_audio(&url).await {
                let fallback = if login_degraded {
                    Some(QualityFallback::Anonymous)
                } else if candidate != target {
                    Some(fallback_for(candidate))
                } else {
                    None
                };

                return Ok(ResolvedStreamInfo {
                    stream: ResolvedStream {
                        url,
                        referer: Some(REFERER.to_string()),
                    },
                    quality: candidate,
                    fallback,
                });
            }

            eprintln!(
                "[sonotheca] {} 返回的内容不是音频（疑似错误页），降级重试",
                candidate.label()
            );
        }

        // 最后兜底：免签名通道（通常 128K，也可能是 404 页，同样要校验）
        if let Ok(stream) = self.plain_stream_url(track_id).await {
            if self.probe_audio(&stream.url).await {
                let fallback = if login_degraded {
                    Some(QualityFallback::Anonymous)
                } else if target != QualityPref::Standard {
                    Some(QualityFallback::Standard)
                } else {
                    None
                };

                return Ok(ResolvedStreamInfo {
                    quality: QualityPref::Standard,
                    fallback,
                    stream,
                });
            }
        }

        Err(AppError::Unavailable)
    }

    /// 单档请求：只负责拿 URL，不做校验
    async fn player_url(&self, track_id: &str, quality: QualityPref) -> AppResult<Option<String>> {
        let body = self
            .weapi(
                "/api/song/enhance/player/url/v1",
                stream_body(track_id, quality),
            )
            .await?;

        Ok(url_of(&body))
    }

    /// Range 读 16 字节做魔数嗅探；HTML / JSON 错误页一律判否
    async fn probe_audio(&self, url: &str) -> bool {
        if !url.starts_with("http") || url.contains("music.163.com/404") {
            return false;
        }

        let response = match self
            .probe
            .get(url)
            .header(header::RANGE, "bytes=0-15")
            .header(header::REFERER, REFERER)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                eprintln!("[sonotheca] 直链探测失败: {err}");
                return false;
            }
        };

        if !response.status().is_success()
            && response.status() != reqwest::StatusCode::PARTIAL_CONTENT
        {
            return false;
        }

        if let Some(content_type) = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
        {
            let content_type = content_type.to_ascii_lowercase();
            if content_type.starts_with("text/")
                || content_type.contains("json")
                || content_type.contains("xml")
            {
                return false;
            }
        }

        let mut head = Vec::with_capacity(16);
        let mut body = response.bytes_stream();
        while head.len() < 16 {
            match body.next().await {
                Some(Ok(chunk)) => {
                    let take = (16 - head.len()).min(chunk.len());
                    head.extend_from_slice(&chunk[..take]);
                }
                _ => break,
            }
        }

        magic::sniff_bytes(&head) != magic::Container::Unknown
    }
}

/// 已降级到的档位 → 给用户的解释
fn fallback_for(achieved: QualityPref) -> QualityFallback {
    match achieved {
        QualityPref::Lossless => QualityFallback::Lossless,
        QualityPref::Standard => QualityFallback::Standard,
        _ => QualityFallback::Exhigh,
    }
}

#[async_trait::async_trait]
impl SourceAdapter for NeteaseAdapter {
    fn source(&self) -> SourceId {
        SourceId::Netease
    }

    async fn search(&self, keyword: &str, offset: u32, limit: u32) -> AppResult<Vec<SearchHit>> {
        let data = json!({
            "s": keyword,
            "type": "1",
            "limit": limit,
            "offset": offset,
            "total": true,
            "csrf_token": "",
        });

        match self.weapi("/api/cloudsearch/get/web", data).await {
            Ok(body) => {
                let songs = body["result"]["songs"].as_array().cloned().unwrap_or_default();
                Ok(songs.iter().map(parse_song).collect())
            }
            Err(err) => {
                eprintln!("[sonotheca] weapi 搜索失败，回退明文: {err}");
                self.plain_search(keyword, offset, limit).await
            }
        }
    }

    async fn get_stream_url(
        &self,
        track_id: &str,
        quality: QualityPref,
    ) -> AppResult<ResolvedStream> {
        Ok(self.resolve_stream(track_id, quality).await?.stream)
    }
}

/// 音质 → 请求体。沉浸声要额外带 immerseType。
///
/// **不要加 `e_r: true`**：weapi 通道收到 e_r 会把响应体 AES 加密，
/// 而 ncm-api-rs 只在 eapi 分支解密，结果就是 body 变成乱码字符串、
/// 所有档位都取不到 url（历史 107KB「404 页面」假下载的根因）。
fn stream_body(track_id: &str, quality: QualityPref) -> Value {
    let mut body = json!({
        "ids": format!("[{track_id}]"),
        "level": quality.level(),
        "encodeType": "flac",
    });

    if let Some(immerse) = quality.immerse_type() {
        body["immerseType"] = json!(immerse);
    }

    body
}

/// 从 `player/url/v1` 响应里取第一条可用直链
fn url_of(body: &Value) -> Option<String> {
    body["data"][0]["url"]
        .as_str()
        .filter(|url| !url.is_empty())
        .map(str::to_string)
}

fn cookies_to_header(raw: &[String]) -> String {
    raw.iter()
        .filter_map(|line| {
            let pair = line.split(';').next()?.trim();
            if pair.is_empty() || !pair.contains('=') {
                None
            } else {
                Some(pair.to_string())
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// 把 `source()` 链拼出来：reqwest 的 Display 只给 "error sending request"
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        out.push_str(" ← ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    eprintln!("[sonotheca] netease error: {out}");
    out
}

pub(crate) fn parse_song(song: &Value) -> SearchHit {
    let artist = song["ar"][0]["name"]
        .as_str()
        .or_else(|| song["artists"][0]["name"].as_str())
        .unwrap_or("未知艺术家")
        .to_string();

    let album = song["al"]["name"]
        .as_str()
        .or_else(|| song["album"]["name"].as_str())
        .unwrap_or("")
        .to_string();

    // picUrl 常给 http://，而 CSP 只放行 https:；空串也要过滤
    let cover = song["al"]["picUrl"]
        .as_str()
        .or_else(|| song["album"]["picUrl"].as_str())
        .map(|url| {
            if let Some(rest) = url.strip_prefix("http://") {
                format!("https://{rest}")
            } else {
                url.to_string()
            }
        })
        .filter(|url| !url.is_empty());

    let duration = song["dt"].as_i64().or_else(|| song["duration"].as_i64());

    SearchHit {
        source: SourceId::Netease,
        id: song["id"].as_i64().map(|v| v.to_string()).unwrap_or_default(),
        title: song["name"].as_str().unwrap_or("未知曲目").to_string(),
        artist,
        album,
        cover_url: cover,
        duration_ms: duration,
    }
}

#[cfg(test)]
mod tests {
    use super::NeteaseAdapter;
    use crate::model::task::QualityPref;

    #[test]
    fn anonymous_degrades_lossless_and_above_to_exhigh() {
        let adapter = NeteaseAdapter::new(None);

        for requested in [
            QualityPref::Lossless,
            QualityPref::HiRes,
            QualityPref::Jymaster,
            QualityPref::Sky,
        ] {
            assert_eq!(
                adapter.effective_quality(requested),
                (QualityPref::Exhigh, true),
                "{requested:?} 应当在未登录时降到极高"
            );
        }

        // 极高的两个档位本来就不需要登录，不能顺手降成 128K
        for requested in [QualityPref::Standard, QualityPref::Exhigh] {
            assert_eq!(adapter.effective_quality(requested), (requested, false));
        }
    }

    #[test]
    fn logged_in_keeps_requested_tier() {
        // 非真实凭据，仅验证登录态分支
        let adapter = NeteaseAdapter::new(Some("MUSIC_U=privacy-placeholder".to_string()));
        assert_eq!(
            adapter.effective_quality(QualityPref::Jymaster),
            (QualityPref::Jymaster, false)
        );
    }
}