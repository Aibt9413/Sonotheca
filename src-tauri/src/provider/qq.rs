//! QQ 音乐适配器（M7 抢救版）。
//!
//! ## 为什么必须换接口
//!
//! 2026-10 实测：legacy 搜索 `c.y.qq.com/soso/fcgi-bin/client_search_cp`
//! 无论 GET / POST 都返回 **HTTP 500 + `Content-Length: 0`**。旧代码直接
//! `response.text()` → 空串 → `serde_json::from_str` 报
//! `EOF while parsing a value at line 1 column 0`，这就是历史「QQ EOF / 空包」。
//!
//! ## 现在走的三层
//!
//! 1. **搜索**：`u.y.qq.com/cgi-bin/musicu.fcg` 的
//!    `music.search.SearchCgiService / DoSearchForQQMusicMobile`（GET + `data`）。
//!    `comm` 必须是最小集合 `{ct:19, cv:1859, uin:"0"}`；带上
//!    `platform / format / needNewCode` 那一整套反而返回 `sum=0`。
//! 2. **兜底**：手机端搜索对部分关键词返回 `req_1.code=2001`（无结果），
//!    此时回退到搜索建议 `smartbox_new.fcg`，再用
//!    `music.pf_song_detail_svr / get_song_detail` 补齐专辑、封面、时长。
//!    实测「七里香」「稻香」就是这样救回来的。
//! 3. **直链**：`vkey.GetVkeyServer / CgiGetVkey`。匿名态只给免费曲目的
//!    `C400` m4a（128K）；VIP / 版权曲返回空 `purl` + `result=104003`，
//!    必须明确判失败 —— 空 URL 一旦进入下载流水线，就是又一次「EOF 假成功」。
//!
//! ## 超时
//!
//! `AppState.client` 的总超时是 1 小时（为下载保留）。QQ 所有请求都显式
//! 压到 10–12 秒：一个卡死的音源不能拖垮多源聚合搜索。

use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::future::join_all;
use futures_util::StreamExt;
use reqwest::header;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::model::source::SourceId;
use crate::model::task::QualityPref;
use crate::pipeline::download::BROWSER_UA;
use crate::pipeline::magic;
use crate::provider::{ResolvedStream, SearchHit, SourceAdapter};

const MUSICU: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
const SMARTBOX: &str = "https://c.y.qq.com/splcloud/fcgi-bin/smartbox_new.fcg";
const REFERER: &str = "https://y.qq.com/";
const FALLBACK_SIP: &str = "https://dl.stream.qqmusic.qq.com/";

/// 网页登录兜底写入的 Cookie；所有 QqAdapter 实例共享
static LOGIN_COOKIE: OnceLock<RwLock<Option<String>>> = OnceLock::new();

fn login_cookie_slot() -> &'static RwLock<Option<String>> {
    LOGIN_COOKIE.get_or_init(|| RwLock::new(None))
}

pub fn set_login_cookie(value: Option<String>) {
    if let Ok(mut guard) = login_cookie_slot().write() {
        *guard = value.filter(|cookie| !cookie.trim().is_empty());
    }
}

fn request_cookie() -> String {
    // 匿名指纹每次启动动态生成，不硬编码任何真实/固定 Cookie
    let stamp = crate::util::now_ms();
    let anonymous = format!("pgv_pvid={stamp}; ts_uid={stamp}");
    let login = login_cookie_slot()
        .read()
        .ok()
        .and_then(|guard| guard.clone());
    match login {
        Some(cookie) => format!("{cookie}; {anonymous}"),
        None => anonymous,
    }
}

const SEARCH_TIMEOUT: Duration = Duration::from_secs(12);
const DETAIL_TIMEOUT: Duration = Duration::from_secs(10);
const STREAM_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(12);

/// 搜索建议接口返回的是曲目 id 而不是完整曲目，补详情时并发抓取
const SMARTBOX_ENRICH_LIMIT: usize = 8;

pub struct QqAdapter {
    client: reqwest::Client,
    /// 直链探测专用：短超时 + 跟随 302，避免用 1 小时总超时的下载客户端
    probe: reqwest::Client,
}

impl QqAdapter {
    pub fn new(client: reqwest::Client) -> Self {
        let probe = reqwest::Client::builder()
            .user_agent(BROWSER_UA)
            .redirect(reqwest::redirect::Policy::limited(5))
            .connect_timeout(Duration::from_secs(8))
            .timeout(PROBE_TIMEOUT)
            .build()
            .unwrap_or_else(|_| client.clone());

        Self { client, probe }
    }

    /// musicu.fcg 统一入口：GET + `format=json&data=<json>`（POST 会被 500001 拒绝）。
    async fn call(&self, payload: &Value, label: &str, timeout: Duration) -> AppResult<Value> {
        let data = payload.to_string();

        let response = self
            .client
            .get(MUSICU)
            .query(&[("format", "json"), ("data", data.as_str())])
            .header(header::REFERER, REFERER)
            .header(header::USER_AGENT, BROWSER_UA)
            .header(header::ACCEPT, "application/json, text/plain, */*")
            .header(header::ORIGIN, "https://y.qq.com")
            .header(header::COOKIE, request_cookie())
            .timeout(timeout)
            .send()
            .await
            .map_err(|err| AppError::Network(format!("{label}请求失败: {}", error_chain(&err))))?;

        read_json(response, label).await
    }

    /// 手机端搜索：返回 `item_song` 数组，字段与歌曲详情基本一致
    async fn mobile_search(
        &self,
        keyword: &str,
        page: u32,
        limit: u32,
    ) -> AppResult<Vec<SearchHit>> {
        let payload = json!({
            "comm": { "ct": 19, "cv": 1859, "uin": "0" },
            "req_1": {
                "module": "music.search.SearchCgiService",
                "method": "DoSearchForQQMusicMobile",
                "param": {
                    "search_type": 0,
                    "query": keyword,
                    "page_num": page,
                    "num_per_page": limit,
                    "highlight": 0,
                    "nqc_flag": 0,
                    "multi_zhida": 0,
                    "cat": 2,
                    "grp": 1
                }
            }
        });

        let body = self
            .call(&payload, "QQ 音乐搜索", SEARCH_TIMEOUT)
            .await?;

        let top = body["code"].as_i64().unwrap_or(-1);
        if top != 0 {
            return Err(AppError::Network(format!(
                "QQ 音乐搜索返回 code={top}（可能触发风控，请稍后重试）"
            )));
        }

        let req_code = body["req_1"]["code"].as_i64().unwrap_or(-1);
        // 2001 = 该关键词无结果，不是接口故障，交给 smartbox 兜底
        if req_code != 0 && req_code != 2001 {
            return Err(AppError::Network(format!(
                "QQ 音乐搜索子请求 code={req_code}"
            )));
        }

        let songs = body["req_1"]["data"]["body"]["item_song"]
            .as_array()
            .or_else(|| body["req_1"]["data"]["body"]["song"]["list"].as_array());

        Ok(songs
            .map(|list| list.iter().filter_map(parse_song).collect())
            .unwrap_or_default())
    }

    /// 搜索建议兜底：只有 4–8 条，且需要逐条补详情
    async fn smartbox_search(&self, keyword: &str) -> AppResult<Vec<SearchHit>> {
        let response = self
            .client
            .get(SMARTBOX)
            .query(&[
                ("key", keyword),
                ("format", "json"),
                ("inCharset", "utf8"),
                ("outCharset", "utf-8"),
                ("platform", "yqq.json"),
                ("needNewCode", "0"),
            ])
            .header(header::REFERER, REFERER)
            .header(header::USER_AGENT, BROWSER_UA)
            .header(header::COOKIE, request_cookie())
            .timeout(SEARCH_TIMEOUT)
            .send()
            .await
            .map_err(|err| {
                AppError::Network(format!("QQ 音乐搜索建议请求失败: {}", error_chain(&err)))
            })?;

        let body = read_json(response, "QQ 音乐搜索建议").await?;
        let Some(items) = body["data"]["song"]["itemlist"].as_array() else {
            return Ok(Vec::new());
        };

        let mut pending = Vec::new();
        for item in items.iter().take(SMARTBOX_ENRICH_LIMIT) {
            let Some(mid) = item["mid"].as_str().map(str::trim).filter(|m| !m.is_empty()) else {
                continue;
            };

            let fallback = parse_smartbox_song(item);
            if fallback.title.is_empty() {
                continue;
            }

            let mid = mid.to_string();
            pending.push(async move {
                // 详情失败不丢结果：用建议接口给的标题/歌手先顶上
                self.song_detail(&mid).await.unwrap_or(fallback)
            });
        }

        Ok(join_all(pending)
            .await
            .into_iter()
            .filter(|hit| !hit.id.is_empty())
            .collect())
    }

    /// 单曲详情：补齐专辑、封面、时长
    async fn song_detail(&self, mid: &str) -> AppResult<SearchHit> {
        let payload = json!({
            "comm": { "ct": 24, "cv": 0, "uin": "0" },
            "req_1": {
                "module": "music.pf_song_detail_svr",
                "method": "get_song_detail",
                "param": { "song_mid": mid }
            }
        });

        let body = self
            .call(&payload, "QQ 音乐歌曲详情", DETAIL_TIMEOUT)
            .await?;

        if body["code"].as_i64().unwrap_or(-1) != 0 {
            return Err(AppError::Network(
                "QQ 音乐歌曲详情返回异常".to_string(),
            ));
        }

        parse_song(&body["req_1"]["data"]["track_info"]).ok_or(AppError::Unavailable)
    }

    /// Range 读 64 字节做魔数嗅探：HTML / JSON 错误页一律判否
    async fn probe_audio(&self, url: &str) -> bool {
        if !url.starts_with("http") {
            return false;
        }

        let response = match self
            .probe
            .get(url)
            .header(header::RANGE, "bytes=0-63")
            .header(header::REFERER, REFERER)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                eprintln!("[sonotheca] QQ 直链探测失败: {err}");
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

        let mut head = Vec::with_capacity(64);
        let mut body = response.bytes_stream();
        while head.len() < 64 {
            match body.next().await {
                Some(Ok(chunk)) => {
                    let take = (64 - head.len()).min(chunk.len());
                    head.extend_from_slice(&chunk[..take]);
                }
                _ => break,
            }
        }

        magic::sniff_bytes(&head) != magic::Container::Unknown
    }
}

#[async_trait]
impl SourceAdapter for QqAdapter {
    fn source(&self) -> SourceId {
        SourceId::Qq
    }

    async fn search(&self, keyword: &str, offset: u32, limit: u32) -> AppResult<Vec<SearchHit>> {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            return Ok(Vec::new());
        }

        let limit = limit.clamp(1, 50);
        let page = offset / limit + 1;

        let mut mobile_failure = None;
        match self.mobile_search(keyword, page, limit).await {
            Ok(hits) if !hits.is_empty() => return Ok(hits),
            Ok(_) => {}
            Err(err) => mobile_failure = Some(err),
        }

        // 搜索建议只有第一页，翻页后不重复请求
        if page == 1 {
            if let Ok(hits) = self.smartbox_search(keyword).await {
                if !hits.is_empty() {
                    return Ok(hits);
                }
            }
        }

        // 手机端搜索本身成功、只是没结果时，空数组是合法结果；
        // 只有它真的失败了才把错误抛给聚合层（由前端按源展示）。
        match mobile_failure {
            Some(err) => Err(err),
            None => Ok(Vec::new()),
        }
    }

    async fn get_stream_url(
        &self,
        track_id: &str,
        _quality: QualityPref,
    ) -> AppResult<ResolvedStream> {
        let track_id = track_id.trim();
        if track_id.is_empty() {
            return Err(AppError::BadInput("缺少 QQ 音乐 songmid".to_string()));
        }

        // 匿名态只请求默认档：带 filename 的高码率请求需要 media_mid，
        // 而 media_mid 与 songmid 并不相同，拼错只会拿到空 purl。
        let payload = json!({
            "comm": { "ct": 24, "cv": 0, "uin": "0", "format": "json" },
            "req_0": {
                "module": "vkey.GetVkeyServer",
                "method": "CgiGetVkey",
                "param": {
                    "guid": "10000",
                    "songmid": [track_id],
                    "songtype": [0],
                    "uin": "0",
                    "loginflag": 1,
                    "platform": "20"
                }
            }
        });

        let body = self
            .call(&payload, "QQ 音乐直链", STREAM_TIMEOUT)
            .await?;

        if body["code"].as_i64().unwrap_or(-1) != 0 {
            return Err(AppError::Network(
                "QQ 音乐直链接口返回异常".to_string(),
            ));
        }

        let url = stream_url_of(&body).map_err(|result| {
            AppError::AuthRequired(format!(
                "QQ 音乐需登录：匿名通道不提供该曲目（result={result}，VIP/版权限制）"
            ))
        })?;

        if !self.probe_audio(&url).await {
            return Err(AppError::Network(
                "QQ 音乐直链无效：CDN 返回的不是音频".to_string(),
            ));
        }

        Ok(ResolvedStream {
            url,
            referer: Some(REFERER.to_string()),
        })
    }
}

/// 把 `vkey` 响应变成可下载 URL。
///
/// `Err(result)` 表示 purl 为空：`104003` = 需要登录 / VIP，其余为版权或下架。
/// CDN 同时支持 http/https，这里统一升到 https，避免 Android release 的明文限制。
fn stream_url_of(body: &Value) -> Result<String, i64> {
    let data = &body["req_0"]["data"];
    let info = &data["midurlinfo"][0];
    let purl = info["purl"].as_str().unwrap_or("").trim();

    if purl.is_empty() {
        return Err(info["result"].as_i64().unwrap_or(0));
    }

    if purl.starts_with("http") {
        return Ok(purl.to_string());
    }

    let sip = data["sip"]
        .as_array()
        .and_then(|list| list.first())
        .and_then(Value::as_str)
        .unwrap_or(FALLBACK_SIP);

    let base = sip.trim_end_matches('/');
    let base = match base.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => base.to_string(),
    };

    Ok(format!("{base}/{}", purl.trim_start_matches('/')))
}

/// 手机端搜索结果与歌曲详情共用同一套字段（`item_song` / `track_info`）
fn parse_song(song: &Value) -> Option<SearchHit> {
    let id = song["mid"]
        .as_str()
        .or_else(|| song["songmid"].as_str())?
        .trim()
        .to_string();
    if id.is_empty() {
        return None;
    }

    let title = song["name"]
        .as_str()
        .or_else(|| song["title"].as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }

    let album = &song["album"];
    let album_name = album["name"]
        .as_str()
        .or_else(|| song["albumname"].as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    // 封面走 y.gtimg.cn 的 T002 规格（https，不受 CSP 限制）
    let album_mid = album["mid"].as_str().unwrap_or("").trim();
    let cover_url = (!album_mid.is_empty()).then(|| {
        format!("https://y.gtimg.cn/music/photo_new/T002R300x300M000{album_mid}.jpg")
    });

    // interval 是秒；老接口的 duration 是毫秒
    let duration_ms = song["interval"]
        .as_i64()
        .filter(|seconds| *seconds > 0)
        .map(|seconds| seconds * 1000)
        .or_else(|| song["duration"].as_i64().filter(|ms| *ms > 0));

    Some(SearchHit {
        source: SourceId::Qq,
        id,
        title,
        artist: join_singers(&song["singer"]),
        album: album_name,
        cover_url,
        duration_ms,
    })
}

fn parse_smartbox_song(item: &Value) -> SearchHit {
    SearchHit {
        source: SourceId::Qq,
        id: item["mid"].as_str().unwrap_or("").trim().to_string(),
        title: item["name"].as_str().unwrap_or("").trim().to_string(),
        artist: item["singer"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or("未知歌手")
            .to_string(),
        album: String::new(),
        cover_url: None,
        duration_ms: None,
    }
}

fn join_singers(value: &Value) -> String {
    let names: Vec<&str> = value
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|singer| singer["name"].as_str().or_else(|| singer.as_str()))
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .take(3)
                .collect()
        })
        .unwrap_or_default();

    if names.is_empty() {
        "未知歌手".to_string()
    } else {
        names.join("、")
    }
}

/// QQ 接口可能返回 JSONP / HTML 拦截页：先查状态码，再宽松剥壳解析。
///
/// 历史 bug 就是没查状态：HTTP 500 + 空 body 被当成空 JSON，报 "EOF"。
async fn read_json(response: reqwest::Response, label: &str) -> AppResult<Value> {
    let status = response.status();
    let text = response.text().await.map_err(|err| {
        AppError::Network(format!("{label}读取响应失败（HTTP {status}）: {err}"))
    })?;

    if !status.is_success() {
        return Err(AppError::Network(format!(
            "{label} HTTP {status}，响应片段: {}",
            snippet(&text)
        )));
    }

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(AppError::Network(format!(
            "{label} 返回空响应（HTTP {status}），接口可能已下线或触发风控"
        )));
    }

    let start = trimmed.find('{');
    let end = trimmed.rfind('}');
    let candidate = match (start, end) {
        (Some(start), Some(end)) if end > start => &trimmed[start..=end],
        _ => {
            return Err(AppError::Network(format!(
                "{label} 返回非 JSON，片段: {}",
                snippet(trimmed)
            )))
        }
    };

    serde_json::from_str(candidate).map_err(|err| {
        AppError::Network(format!(
            "{label} JSON 解析失败（{err}），片段: {}",
            snippet(trimmed)
        ))
    })
}

fn snippet(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
        .take(240)
        .collect();

    if text.chars().count() > 240 {
        out.push('…');
    }

    out
}

/// reqwest 的 Display 只给 "error sending request"，把 source 链拼出来方便定位
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        out.push_str(" ← ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOBILE_SEARCH: &str = r#"{
        "code": 0,
        "req_1": {
            "code": 0,
            "data": {
                "body": {
                    "item_song": [
                        {
                            "mid": "0039MnYb0qxYhV",
                            "name": "晴天",
                            "interval": 269,
                            "singer": [{ "name": "周杰伦" }],
                            "album": { "mid": "000MkMni19ClKG", "name": "叶惠美" }
                        }
                    ],
                    "song": { "list": [] }
                }
            }
        }
    }"#;

    const SONG_DETAIL: &str = r#"{
        "code": 0,
        "req_1": {
            "code": 0,
            "data": {
                "track_info": {
                    "mid": "004Z8Ihr0JIu5s",
                    "name": "七里香",
                    "interval": 299,
                    "singer": [{ "name": "周杰伦" }, { "name": "合唱" }],
                    "album": { "mid": "003DFRzD192KKD", "name": "七里香" }
                }
            }
        }
    }"#;

    const VKEY_OK: &str = r#"{
        "code": 0,
        "req_0": {
            "code": 0,
            "data": {
                "sip": ["http://aqqmusic.tc.qq.com/"],
                "midurlinfo": [{
                    "songmid": "001LcPEv1TEenh",
                    "purl": "C400001LcPEv1TEenh.m4a?guid=10000&vkey=ABC123&uin=&fromtag=120032",
                    "result": 0
                }]
            }
        }
    }"#;

    const VKEY_EMPTY: &str = r#"{
        "code": 0,
        "req_0": {
            "code": 0,
            "data": {
                "sip": ["http://aqqmusic.tc.qq.com/"],
                "midurlinfo": [{
                    "songmid": "0039MnYb0qxYhV",
                    "purl": "",
                    "result": 104003
                }]
            }
        }
    }"#;

    #[test]
    fn parses_mobile_search_song() {
        let body: Value = serde_json::from_str(MOBILE_SEARCH).expect("fixture");
        let songs = body["req_1"]["data"]["body"]["item_song"]
            .as_array()
            .expect("item_song");

        let hit = parse_song(&songs[0]).expect("hit");
        assert_eq!(hit.source, SourceId::Qq);
        assert_eq!(hit.id, "0039MnYb0qxYhV");
        assert_eq!(hit.title, "晴天");
        assert_eq!(hit.artist, "周杰伦");
        assert_eq!(hit.album, "叶惠美");
        assert_eq!(hit.duration_ms, Some(269_000));
        assert_eq!(
            hit.cover_url.as_deref(),
            Some("https://y.gtimg.cn/music/photo_new/T002R300x300M000000MkMni19ClKG.jpg")
        );
    }

    #[test]
    fn parses_detail_and_joins_multiple_singers() {
        let body: Value = serde_json::from_str(SONG_DETAIL).expect("fixture");
        let hit = parse_song(&body["req_1"]["data"]["track_info"]).expect("hit");
        assert_eq!(hit.id, "004Z8Ihr0JIu5s");
        assert_eq!(hit.title, "七里香");
        assert_eq!(hit.artist, "周杰伦、合唱");
        assert_eq!(hit.duration_ms, Some(299_000));
    }

    #[test]
    fn builds_https_stream_url_from_relative_purl() {
        let body: Value = serde_json::from_str(VKEY_OK).expect("fixture");
        let url = stream_url_of(&body).expect("url");
        assert!(
            url.starts_with("https://aqqmusic.tc.qq.com/C400001LcPEv1TEenh.m4a?"),
            "url = {url}"
        );
        assert!(url.contains("vkey=ABC123"));
    }

    #[test]
    fn empty_purl_reports_login_result_code() {
        let body: Value = serde_json::from_str(VKEY_EMPTY).expect("fixture");
        assert_eq!(stream_url_of(&body), Err(104003));
    }

    #[test]
    fn song_without_mid_or_title_is_skipped() {
        let missing_mid = json!({ "name": "晴天" });
        assert!(parse_song(&missing_mid).is_none());

        let missing_title = json!({ "mid": "0039MnYb0qxYhV", "name": "  " });
        assert!(parse_song(&missing_title).is_none());
    }

    #[test]
    fn smartbox_item_is_a_usable_fallback() {
        let item = json!({
            "mid": "004Z8Ihr0JIu5s",
            "name": "七里香",
            "singer": "周杰伦"
        });

        let hit = parse_smartbox_song(&item);
        assert_eq!(hit.id, "004Z8Ihr0JIu5s");
        assert_eq!(hit.title, "七里香");
        assert_eq!(hit.artist, "周杰伦");
        assert!(hit.album.is_empty());
    }

    /// 需要外网，手动执行：
    /// `cargo test --lib provider::qq -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "需要外网：手动验证 QQ 搜索与匿名直链"]
    async fn live_search_and_stream() {
        let client = reqwest::Client::builder()
            .user_agent(BROWSER_UA)
            .build()
            .expect("client");
        let adapter = QqAdapter::new(client);

        let hits = adapter.search("周杰伦", 0, 10).await.expect("search");
        assert!(!hits.is_empty(), "search returned no hits");

        // 搜索建议兜底覆盖手机端返回 2001 的关键词
        let fallback = adapter.search("七里香", 0, 10).await.expect("smartbox");
        assert!(
            fallback.iter().any(|hit| hit.title == "七里香"),
            "smartbox fallback missed 七里香"
        );

        // 实测匿名可用的免费曲目
        let stream = adapter
            .get_stream_url("001LcPEv1TEenh", QualityPref::Standard)
            .await
            .expect("stream");
        assert!(stream.url.starts_with("https://"));
    }
}
