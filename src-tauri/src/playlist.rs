//! 通用歌单解析与分页（M8）。
//!
//! 硬纪律：**绝不一次性拉全量**。
//! - 网易云：`playlist/detail?n=0` 只拿元数据与 trackIds，每页再按 50 个 id
//!   调 `api/song/detail` 补歌曲信息；
//! - B 站：收藏夹接口每页 20 条，按 offset/limit 计算需要抓几页，合并后切片。
//!
//! 前端只认 [`PlaylistPreview`] / [`PlaylistPage`] / [`PlaylistTrack`]。

use std::collections::HashMap;
use std::time::Duration;

use reqwest::header;
use serde::Serialize;
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::model::source::SourceId;
use crate::pipeline::download::BROWSER_UA;
use crate::provider::netease::parse_song;

const NETEASE_DETAIL: &str = "https://music.163.com/api/v6/playlist/detail";
const NETEASE_SONG_DETAIL: &str = "https://music.163.com/api/song/detail";
const BILI_FAV: &str = "https://api.bilibili.com/x/v3/fav/resource/list";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const BILI_PAGE_SIZE: u32 = 20;

#[derive(Debug, Clone)]
pub struct PlaylistRef {
    pub source: SourceId,
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistPreview {
    pub source: SourceId,
    pub id: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub total: u32,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentPlaylist {
    pub source: SourceId,
    pub id: String,
    pub name: String,
    pub cover_url: Option<String>,
    pub total: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistTrack {
    pub source: SourceId,
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_url: Option<String>,
    pub duration_ms: Option<i64>,
    /// 1-based 在歌单中的序号
    pub index: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistPage {
    pub offset: u32,
    pub limit: u32,
    pub total: u32,
    pub tracks: Vec<PlaylistTrack>,
    pub has_more: bool,
}

/// 同步解析直链；短链（163cn.tv / b23.tv）交给 [`resolve_link`] 跟跳转。
pub fn parse_link(input: &str) -> Option<PlaylistRef> {
    let url = reqwest::Url::parse(input.trim()).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let query = query_map(&url);

    if matches!(
        host.as_str(),
        "music.163.com" | "y.music.163.com" | "m.music.163.com"
    ) {
        // 必须真的是歌单/榜单路由，不能把 /song?id=123 误判成歌单
        let path = url.path().to_ascii_lowercase();
        let fragment = url.fragment().unwrap_or("").to_ascii_lowercase();
        let is_playlist = path.contains("playlist")
            || path.contains("toplist")
            || fragment.contains("playlist")
            || fragment.contains("toplist");
        if !is_playlist {
            return None;
        }

        let id = query.get("id").cloned().or_else(|| {
            url.path_segments()
                .and_then(|mut segments| {
                    segments
                        .find(|part| part.chars().all(|c| c.is_ascii_digit()) && !part.is_empty())
                        .map(str::to_string)
                })
        })?;

        if id.chars().all(|c| c.is_ascii_digit()) {
            return Some(PlaylistRef {
                source: SourceId::Netease,
                id,
            });
        }
    }

    if host == "space.bilibili.com" || host.ends_with(".bilibili.com") {
        let fid = query.get("fid").cloned()?;

        if fid.chars().all(|c| c.is_ascii_digit()) {
            return Some(PlaylistRef {
                source: SourceId::Bilibili,
                id: fid,
            });
        }
    }

    None
}

/// 解析歌单链接，必要时跟随短链跳转
pub async fn resolve_link(client: &reqwest::Client, input: &str) -> AppResult<PlaylistRef> {
    if let Some(parsed) = parse_link(input) {
        return Ok(parsed);
    }

    let url = reqwest::Url::parse(input.trim())
        .map_err(|err| AppError::BadInput(format!("不是合法的歌单链接: {err}")))?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();

    if matches!(host.as_str(), "163cn.tv" | "b23.tv") {
        let response = client
            .get(url)
            .header(header::USER_AGENT, BROWSER_UA)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|err| AppError::Network(format!("短链解析失败: {err}")))?;

        if let Some(parsed) = parse_link(response.url().as_str()) {
            return Ok(parsed);
        }
    }

    Err(AppError::BadInput(
        "暂不支持该链接，请粘贴网易云歌单或 B 站收藏夹地址".to_string(),
    ))
}

pub async fn preview(
    client: &reqwest::Client,
    bili_cookie: Option<&str>,
    parsed: &PlaylistRef,
) -> AppResult<PlaylistPreview> {
    match parsed.source {
        SourceId::Netease => netease_preview(client, &parsed.id).await,
        SourceId::Bilibili => bilibili_preview(client, bili_cookie, &parsed.id).await,
        _ => Err(AppError::BadInput("该音源暂不支持歌单".to_string())),
    }
}

pub async fn page(
    client: &reqwest::Client,
    bili_cookie: Option<&str>,
    source: SourceId,
    id: &str,
    offset: u32,
    limit: u32,
) -> AppResult<PlaylistPage> {
    let limit = if limit == 0 {
        10_000
    } else {
        limit.clamp(1, 1000)
    };

    match source {
        SourceId::Netease => netease_page(client, id, offset, limit).await,
        SourceId::Bilibili => bilibili_page(client, bili_cookie, id, offset, limit).await,
        _ => Err(AppError::BadInput("该音源暂不支持歌单".to_string())),
    }
}

// ───────────────────────────── 网易云 ─────────────────────────────

async fn netease_preview(client: &reqwest::Client, id: &str) -> AppResult<PlaylistPreview> {
    let playlist = netease_playlist(client, id).await?;
    let track_ids = playlist["trackIds"].as_array().cloned().unwrap_or_default();
    let total = playlist["trackCount"]
        .as_u64()
        .map(|value| value as u32)
        .unwrap_or(track_ids.len() as u32);

    Ok(PlaylistPreview {
        source: SourceId::Netease,
        id: id.to_string(),
        title: playlist["name"]
            .as_str()
            .unwrap_or("网易云歌单")
            .to_string(),
        cover_url: playlist["coverImgUrl"].as_str().map(upgrade_https),
        total,
        description: playlist["description"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    })
}

async fn netease_playlist(client: &reqwest::Client, id: &str) -> AppResult<Value> {
    let body = fetch_json(
        client
            .get(NETEASE_DETAIL)
            .query(&[("id", id), ("n", "0"), ("s", "0")]),
        "网易云歌单",
    )
    .await?;

    if body["code"].as_i64().unwrap_or(-1) != 200 {
        return Err(AppError::BadInput(
            "网易云歌单不存在或不可见".to_string(),
        ));
    }

    let playlist = &body["playlist"];
    if playlist.is_null() {
        return Err(AppError::BadInput(
            "网易云歌单不存在或不可见".to_string(),
        ));
    }

    Ok(playlist.clone())
}

async fn netease_page(
    client: &reqwest::Client,
    id: &str,
    offset: u32,
    limit: u32,
) -> AppResult<PlaylistPage> {
    let playlist = netease_playlist(client, id).await?;
    let track_ids = playlist["trackIds"].as_array().cloned().unwrap_or_default();
    let total = playlist["trackCount"]
        .as_u64()
        .map(|value| value as u32)
        .unwrap_or(track_ids.len() as u32);

    let start = offset as usize;
    let end = ((offset + limit) as usize).min(track_ids.len());
    if start >= track_ids.len() {
        return Ok(PlaylistPage {
            offset,
            limit,
            total,
            tracks: Vec::new(),
            has_more: false,
        });
    }

    let ids: Vec<String> = track_ids[start..end]
        .iter()
        .filter_map(|item| {
            item["id"]
                .as_i64()
                .map(|value| value.to_string())
                .or_else(|| item["id"].as_str().map(str::to_string))
        })
        .collect();

    if ids.is_empty() {
        return Ok(PlaylistPage {
            offset,
            limit,
            total,
            tracks: Vec::new(),
            has_more: false,
        });
    }

    // 网易云 song/detail 单次最多稳妥拿 50 首；100/页时拆成两批
    let mut by_id: HashMap<String, Value> = HashMap::new();
    for chunk in ids.chunks(50) {
        let param = format!("[{}]", chunk.join(","));
        let body = fetch_json(
            client.get(NETEASE_SONG_DETAIL).query(&[("ids", param)]),
            "网易云歌曲详情",
        )
        .await?;

        if body["code"].as_i64().unwrap_or(-1) != 200 {
            return Err(AppError::Network(
                "网易云歌曲详情返回异常".to_string(),
            ));
        }

        for song in body["songs"].as_array().cloned().unwrap_or_default() {
            if let Some(song_id) = song["id"]
                .as_i64()
                .map(|value| value.to_string())
                .or_else(|| song["id"].as_str().map(str::to_string))
            {
                by_id.insert(song_id, song);
            }
        }
    }

    let tracks = ids
        .iter()
        .enumerate()
        .filter_map(|(index, song_id)| {
            let song = by_id.get(song_id)?;
            let hit = parse_song(song);
            if hit.title.is_empty() {
                return None;
            }

            Some(PlaylistTrack {
                source: SourceId::Netease,
                id: hit.id,
                title: hit.title,
                artist: hit.artist,
                album: hit.album,
                cover_url: hit.cover_url,
                duration_ms: hit.duration_ms,
                index: offset + index as u32 + 1,
            })
        })
        .collect::<Vec<_>>();

    Ok(PlaylistPage {
        offset,
        limit,
        total,
        has_more: offset + (tracks.len() as u32) < total,
        tracks,
    })
}

// ───────────────────────────── B 站 ─────────────────────────────

async fn bilibili_preview(
    client: &reqwest::Client,
    cookie: Option<&str>,
    media_id: &str,
) -> AppResult<PlaylistPreview> {
    let page = bilibili_fav_page(client, cookie, media_id, 1).await?;
    let info = &page.info;

    Ok(PlaylistPreview {
        source: SourceId::Bilibili,
        id: media_id.to_string(),
        title: info["title"]
            .as_str()
            .unwrap_or("B站收藏夹")
            .to_string(),
        cover_url: info["cover"].as_str().map(upgrade_https),
        total: page.total,
        description: info["intro"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    })
}

struct BiliFavPage {
    info: Value,
    medias: Vec<Value>,
    has_more: bool,
    total: u32,
}

async fn bilibili_fav_page(
    client: &reqwest::Client,
    cookie: Option<&str>,
    media_id: &str,
    page: u32,
) -> AppResult<BiliFavPage> {
    let mut request = client
        .get(BILI_FAV)
        .query(&[
            ("media_id", media_id.to_string()),
            ("pn", page.to_string()),
            ("ps", BILI_PAGE_SIZE.to_string()),
            ("platform", "web".to_string()),
        ])
        .header(header::REFERER, "https://www.bilibili.com/");

    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }

    let body = fetch_json(request, "B站收藏夹").await?;

    if body["code"].as_i64().unwrap_or(-1) != 0 {
        return Err(AppError::BadInput(format!(
            "B站收藏夹不可用: {}",
            body["message"].as_str().unwrap_or("可能未公开或需要登录")
        )));
    }

    let data = &body["data"];
    if data.is_null() {
        return Err(AppError::BadInput(
            "B站收藏夹不存在或不可见（私密收藏夹请先登录）".to_string(),
        ));
    }

    let info = data["info"].clone();
    let medias = data["medias"].as_array().cloned().unwrap_or_default();
    let total = info["media_count"]
        .as_u64()
        .map(|value| value as u32)
        .unwrap_or(medias.len() as u32);

    Ok(BiliFavPage {
        info,
        medias,
        has_more: data["has_more"].as_bool().unwrap_or(false),
        total,
    })
}

async fn bilibili_page(
    client: &reqwest::Client,
    cookie: Option<&str>,
    media_id: &str,
    offset: u32,
    limit: u32,
) -> AppResult<PlaylistPage> {
    let start_page = offset / BILI_PAGE_SIZE + 1;
    let end_page = (offset + limit - 1) / BILI_PAGE_SIZE + 1;

    let mut collected: Vec<(u32, Value)> = Vec::new();
    let mut total = 0u32;
    let mut last_has_more = false;

    for pn in start_page..=end_page {
        let page = bilibili_fav_page(client, cookie, media_id, pn).await?;
        total = total.max(page.total);
        last_has_more = page.has_more;

        for (index, media) in page.medias.into_iter().enumerate() {
            let global_index = (pn - 1) * BILI_PAGE_SIZE + index as u32 + 1;
            collected.push((global_index, media));
        }

        if !page.has_more {
            break;
        }
    }

    let tracks = collected
        .into_iter()
        .filter(|(index, _)| *index > offset && *index <= offset + limit)
        .filter_map(|(index, media)| {
            if media["type"].as_i64().unwrap_or(2) != 2 {
                return None;
            }

            let bvid = media["bvid"]
                .as_str()
                .or_else(|| media["bv_id"].as_str())?
                .to_string();
            let title = media["title"].as_str().unwrap_or("").trim().to_string();
            if title.is_empty() {
                return None;
            }

            Some(PlaylistTrack {
                source: SourceId::Bilibili,
                id: bvid,
                title,
                artist: media["upper"]["name"]
                    .as_str()
                    .unwrap_or("B站")
                    .to_string(),
                album: "B站收藏夹".to_string(),
                cover_url: media["cover"].as_str().map(upgrade_https),
                duration_ms: media["duration"].as_i64().map(|seconds| seconds * 1000),
                index,
            })
        })
        .collect::<Vec<_>>();

    Ok(PlaylistPage {
        offset,
        limit,
        total,
        has_more: last_has_more || offset + (tracks.len() as u32) < total,
        tracks,
    })
}

// ───────────────────────────── 公共 ─────────────────────────────

async fn fetch_json(request: reqwest::RequestBuilder, label: &str) -> AppResult<Value> {
    let response = request
        .header(header::USER_AGENT, BROWSER_UA)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|err| AppError::Network(format!("{label}请求失败: {err}")))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| AppError::Network(format!("{label}读取响应失败: {err}")))?;

    if !status.is_success() {
        return Err(AppError::Network(format!(
            "{label} HTTP {status}: {}",
            snippet(&text)
        )));
    }

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(AppError::Network(format!("{label}返回空响应")));
    }

    serde_json::from_str(trimmed).map_err(|err| {
        AppError::Network(format!(
            "{label} JSON 解析失败（{err}）: {}",
            snippet(trimmed)
        ))
    })
}

fn query_map(url: &reqwest::Url) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = url.query_pairs().into_owned().collect();

    // 网易云常见 hash 路由：/#/playlist?id=123
    if let Some(fragment) = url.fragment() {
        if let Some((_, query)) = fragment.split_once('?') {
            if let Ok(parsed) = reqwest::Url::parse(&format!("https://placeholder/?{query}")) {
                for (key, value) in parsed.query_pairs() {
                    out.entry(key.into_owned()).or_insert(value.into_owned());
                }
            }
        }
    }

    out
}

fn upgrade_https(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("//") {
        return format!("https://{rest}");
    }
    match url.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    }
}

fn snippet(text: &str) -> String {
    text.chars()
        .take(200)
        .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_netease_links() {
        for raw in [
            "https://music.163.com/playlist?id=3778678&userid=1",
            "https://music.163.com/#/playlist?id=3778678",
            "https://y.music.163.com/m/playlist?id=3778678",
        ] {
            let parsed = parse_link(raw).unwrap_or_else(|| panic!("failed: {raw}"));
            assert_eq!(parsed.source, SourceId::Netease);
            assert_eq!(parsed.id, "3778678");
        }
    }

    #[test]
    fn parses_bilibili_fav_link() {
        let parsed =
            parse_link("https://space.bilibili.com/123456/favlist?fid=789&ftype=create").unwrap();
        assert_eq!(parsed.source, SourceId::Bilibili);
        assert_eq!(parsed.id, "789");
    }

    #[test]
    fn rejects_unrelated_links() {
        assert!(parse_link("https://music.163.com/song?id=123").is_none());
        assert!(parse_link("https://example.com/playlist?id=1").is_none());
    }

    /// 需要外网，手动执行：
    /// `cargo test --lib playlist -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "需要外网：验证网易云歌单分页"]
    async fn live_netease_pagination() {
        let client = reqwest::Client::builder()
            .user_agent(BROWSER_UA)
            .build()
            .expect("client");
        let parsed = PlaylistRef {
            source: SourceId::Netease,
            id: "3778678".to_string(),
        };

        let preview = preview(&client, None, &parsed).await.expect("preview");
        assert!(preview.total > 50, "total = {}", preview.total);
        assert!(!preview.title.is_empty());

        let first = page(&client, None, SourceId::Netease, &parsed.id, 0, 50)
            .await
            .expect("first page");
        assert_eq!(first.tracks.len(), 50);
        assert_eq!(first.tracks[0].index, 1);
        assert!(first.has_more);

        let second = page(&client, None, SourceId::Netease, &parsed.id, 50, 50)
            .await
            .expect("second page");
        assert!(!second.tracks.is_empty());
        assert_eq!(second.tracks[0].index, 51);
    }
}
