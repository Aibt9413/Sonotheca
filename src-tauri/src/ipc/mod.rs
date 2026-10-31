//! Tauri 命令层。命令名 snake_case，参数由 Tauri 自动映射为 camelCase。

use base64::Engine as _;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use uuid::Uuid;

use crate::db::repo;
use crate::error::{AppError, AppResult};
use crate::model::source::SourceId;
use crate::model::task::{CollisionPolicy, DownloadTask, QualityPref, TaskSnapshot, TaskState};
use crate::model::track::{PathStatus, Track};
use crate::paths;
use crate::pipeline::download::BROWSER_UA;
use crate::pipeline::path::{render_relative_dir, PathCtx};
use crate::pipeline::runner::{self, RunConfig};
use crate::playlist::{self, PlaylistPage, PlaylistPreview, RecentPlaylist};
use crate::provider::auth::{
    AuthProvider, BilibiliStatus, QrLoginStart, QrLoginState,
};
use crate::provider::bilibili::{BiliPage, BilibiliAdapter};
use crate::provider::navidrome::{self, NavidromeAdapter, NavidromeConfig};
use crate::provider::netease::{NeteaseProfile, QrState, QualityFallback};
use crate::provider::qq::QqAdapter;use crate::provider::{ResolvedStream, SearchHit, SourceAdapter};
use crate::state::AppState;
use crate::util::now_ms;

/// M1 的固定归类规则。M2 改成从 settings 表读，并支持用户编辑。
const DEFAULT_DIR_TEMPLATE: &str = "{source}/{artist}";
const DEFAULT_FILE_TEMPLATE: &str = "{artist} - {title}.{ext}";
const UNKNOWN_ARTIST: &str = "未知艺术家";
const UNKNOWN_TITLE: &str = "未命名音轨";

#[tauri::command]
pub fn ping(name: String) -> String {
    format!("pong: {name}")
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub platform: String,
    pub db_tracks: i64,
    pub schema_version: i64,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppResult<AppInfo> {
    let connection = lock_db(&state)?;

    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
        db_tracks: repo::count_tracks(&connection)?,
        schema_version: crate::db::schema_version(&connection)?,
    })
}

fn lock_db<'a>(
    state: &'a State<'_, AppState>,
) -> AppResult<std::sync::MutexGuard<'a, rusqlite::Connection>> {
    state
        .db
        .lock()
        .map_err(|_| AppError::Internal("数据库连接锁被毒化".to_string()))
}

// ───────────────────────────── 搜索 ─────────────────────────────

/// 网易云搜索（匿名态）。返回空数组是合法结果，前端不能因此卡住。
#[tauri::command]
pub async fn search_netease(
    state: State<'_, AppState>,
    keyword: String,
    offset: Option<u32>,
    limit: Option<u32>,
) -> AppResult<Vec<SearchHit>> {
    let keyword = keyword.trim().to_string();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }

    let adapter = state.netease.clone();
    adapter
        .search(&keyword, offset.unwrap_or(0), limit.unwrap_or(30).min(50))
        .await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFailure {
    pub source: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchAggregate {
    pub hits: Vec<SearchHit>,
    pub errors: Vec<SourceFailure>,
}

/// 多源并发搜索：单源失败只进 `errors`，不影响其他源。
#[tauri::command]
pub async fn search_sources(
    state: State<'_, AppState>,
    keyword: String,
    sources: Vec<String>,
) -> AppResult<SearchAggregate> {
    let keyword = keyword.trim().to_string();
    if keyword.is_empty() {
        return Ok(SearchAggregate {
            hits: Vec::new(),
            errors: Vec::new(),
        });
    }

    let all = sources.is_empty() || sources.iter().any(|name| name == "all");
    let wanted = |name: &str| all || sources.iter().any(|item| item == name);

    let mut handles: Vec<(
        &'static str,
        tokio::task::JoinHandle<AppResult<Vec<SearchHit>>>,
    )> = Vec::new();
    let mut errors = Vec::new();

    if wanted("netease") {
        let adapter = state.netease.clone();
        let keyword = keyword.clone();
        handles.push((
            "netease",
            tokio::spawn(async move { adapter.search(&keyword, 0, 30).await }),
        ));
    }

    if wanted("bilibili") {
        let adapter = BilibiliAdapter::new(state.client.as_ref().clone());
        let keyword = keyword.clone();
        handles.push((
            "bilibili",
            tokio::spawn(async move { adapter.search(&keyword, 0, 30).await }),
        ));
    }

    if wanted("qq") {
        let adapter = QqAdapter::new(state.client.as_ref().clone());
        let keyword = keyword.clone();
        handles.push((
            "qq",
            tokio::spawn(async move { adapter.search(&keyword, 0, 30).await }),
        ));
    }

    if wanted("navidrome") {
        let config = {
            let connection = lock_db(&state)?;
            NavidromeConfig::from_settings(&connection)
        };

        match config {
            Some(config) => {
                let adapter = NavidromeAdapter::new(state.client.as_ref().clone(), config);
                let keyword = keyword.clone();
                handles.push((
                    "navidrome",
                    tokio::spawn(async move { adapter.search(&keyword, 0, 30).await }),
                ));
            }
            None => errors.push(SourceFailure {
                source: "navidrome".to_string(),
                message: "未配置 Navidrome 服务器".to_string(),
            }),
        }
    }

    let mut hits = Vec::new();
    for (source, handle) in handles {
        // 所有源同时 spawn，这里按 15s 封顶逐个收口：卡死的源不能拖垮整次搜索
        match tokio::time::timeout(std::time::Duration::from_secs(15), handle).await {
            Ok(Ok(Ok(mut found))) => hits.append(&mut found),
            Ok(Ok(Err(err))) => errors.push(SourceFailure {
                source: source.to_string(),
                message: err.to_string(),
            }),
            Ok(Err(err)) => errors.push(SourceFailure {
                source: source.to_string(),
                message: format!("搜索任务异常: {err}"),
            }),
            Err(_) => errors.push(SourceFailure {
                source: source.to_string(),
                message: "搜索超时（15s），已跳过该音源".to_string(),
            }),
        }
    }

    Ok(SearchAggregate { hits, errors })
}

// ───────────────────────────── 歌单（M8） ─────────────────────────────
/// 预检：只取歌单元数据，供歌单页顶部展示；成功即写入最近歌单。
#[tauri::command]
pub async fn playlist_preview(
    state: State<'_, AppState>,
    url: String,
) -> AppResult<PlaylistPreview> {
    let parsed = playlist::resolve_link(state.client.as_ref(), &url).await?;
    let cookie = state.bilibili.cookie();
    let preview = playlist::preview(state.client.as_ref(), cookie.as_deref(), &parsed).await?;

    {
        let connection = lock_db(&state)?;
        repo::upsert_recent_playlist(
            &connection,
            preview.source.as_str(),
            &preview.id,
            &preview.title,
            preview.cover_url.as_deref(),
            preview.total as i64,
            now_ms(),
        )?;
    }

    Ok(preview)
}

#[tauri::command]
pub fn recent_playlists(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> AppResult<Vec<RecentPlaylist>> {
    let connection = lock_db(&state)?;
    let rows = repo::list_recent_playlists(&connection, limit.unwrap_or(8).min(20))?;

    Ok(rows
        .into_iter()
        .filter_map(|(source, id, name, cover_url, total)| {
            let source = source.parse::<SourceId>().ok()?;
            Some(RecentPlaylist {
                source,
                id,
                name,
                cover_url,
                total: total.max(0) as u32,
            })
        })
        .collect())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyPlaylistItem {
    pub source: SourceId,
    pub id: String,
    pub name: String,
    pub cover_url: Option<String>,
    pub total: u32,
}

fn https_cover(url: Option<String>) -> Option<String> {
    url.map(|value| {
        if let Some(rest) = value.strip_prefix("//") {
            format!("https://{rest}")
        } else if let Some(rest) = value.strip_prefix("http://") {
            format!("https://{rest}")
        } else {
            value
        }
    })
}

/// 当前账号的歌单：网易云走 account/UID，B站走 nav/MID；QQ 暂空。
#[tauri::command]
pub async fn my_playlists(
    state: State<'_, AppState>,
    source: SourceId,
) -> AppResult<Vec<MyPlaylistItem>> {
    match source {
        SourceId::Netease => Ok(state
            .netease
            .my_playlists()
            .await?
            .into_iter()
            .map(|item| MyPlaylistItem {
                source: SourceId::Netease,
                id: item.id,
                name: item.name,
                cover_url: https_cover(item.cover_url),
                total: item.total,
            })
            .collect()),
        SourceId::Bilibili => {
            let cookie = state
                .bilibili
                .cookie()
                .ok_or_else(|| AppError::BadInput("B站未登录".to_string()))?;

            let nav = state
                .client
                .get("https://api.bilibili.com/x/web-interface/nav")
                .header(reqwest::header::COOKIE, cookie.as_str())
                .header(reqwest::header::USER_AGENT, BROWSER_UA)
                .header(reqwest::header::REFERER, "https://www.bilibili.com/")
                .send()
                .await
                .map_err(|err| AppError::Network(format!("B站 nav 失败: {err}")))?
                .json::<serde_json::Value>()
                .await
                .map_err(|err| AppError::Network(format!("B站 nav 解析失败: {err}")))?;

            let mid = nav["data"]["mid"].as_i64().unwrap_or(0);
            if mid == 0 {
                return Err(AppError::BadInput("B站登录状态不可用".to_string()));
            }

            let body = state
                .client
                .get(format!(
                    "https://api.bilibili.com/x/v3/fav/folder/created/list-all?up_mid={mid}"
                ))
                .header(reqwest::header::COOKIE, cookie.as_str())
                .header(reqwest::header::USER_AGENT, BROWSER_UA)
                .header(reqwest::header::REFERER, "https://www.bilibili.com/")
                .send()
                .await
                .map_err(|err| AppError::Network(format!("B站收藏夹失败: {err}")))?
                .json::<serde_json::Value>()
                .await
                .map_err(|err| AppError::Network(format!("B站收藏夹解析失败: {err}")))?;

            Ok(body["data"]["list"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|item| {
                    let id = item["id"]
                        .as_i64()
                        .map(|value| value.to_string())
                        .or_else(|| item["id"].as_str().map(str::to_string))?;
                    let name = item["title"].as_str().unwrap_or("").to_string();
                    if name.is_empty() {
                        return None;
                    }

                    Some(MyPlaylistItem {
                        source: SourceId::Bilibili,
                        id,
                        name,
                        cover_url: https_cover(item["cover"].as_str().map(str::to_string)),
                        total: item["media_count"].as_u64().unwrap_or(0) as u32,
                    })
                })
                .collect())
        }
        SourceId::Qq => Ok(Vec::new()),
        _ => Ok(Vec::new()),
    }
}

/// 分页取歌单曲目。前端固定 50 首/页，这里只按 offset/limit 取对应区间。
#[tauri::command]
pub async fn playlist_page(
    state: State<'_, AppState>,
    source: String,
    id: String,
    offset: Option<u32>,
    limit: Option<u32>,
) -> AppResult<PlaylistPage> {
    let source_id: SourceId = source
        .parse()
        .map_err(|_| AppError::BadInput(format!("未知音源: {source}")))?;
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(AppError::BadInput("缺少歌单 id".to_string()));
    }

    let cookie = state.bilibili.cookie();
    let requested = limit.unwrap_or(50);
    // 0 = 无限制：一次拉到后端可承受的上限；具体条数由歌单 total 截断
    let limit = if requested == 0 { 10_000 } else { requested };
    playlist::page(
        state.client.as_ref(),
        cookie.as_deref(),
        source_id,
        &id,
        offset.unwrap_or(0),
        limit,
    )
    .await
}

// ───────────────────────────── 入队 ─────────────────────────────
/// 所有音源共用的入队逻辑：固化快照 → 落库 → 起后台任务。
#[allow(clippy::too_many_arguments)]
fn enqueue_download(
    app: &AppHandle,
    state: &AppState,
    source: SourceId,
    source_track_id: String,
    title: String,
    artist: String,
    download_url: Option<String>,
    quality: QualityPref,
) -> AppResult<DownloadTask> {
    let (root_id, root) = {
        let connection = state
            .db
            .lock()
            .map_err(|_| AppError::Internal("数据库连接锁被毒化".to_string()))?;
        paths::library_root(app, &connection)?
    };

    // 快照在这里固化：之后改模板/策略都不影响这个任务
    let ctx = PathCtx {
        source: source.as_str(),
        quality: "best",
        artist: &artist,
        title: &title,
        album: "",
        track_no: None,
        ext: "bin",
    };
    let relative_dir = render_relative_dir(DEFAULT_DIR_TEMPLATE, &ctx)?;

    let id = Uuid::new_v4();
    let now = now_ms();
    let part_name = format!("{id}.part");

    let task = DownloadTask {
        id,
        source,
        source_track_id,
        title,
        artist,
        state: TaskState::Queued,
        snapshot: TaskSnapshot {
            root_id,
            dir_template: DEFAULT_DIR_TEMPLATE.to_string(),
            file_template: DEFAULT_FILE_TEMPLATE.to_string(),
            relative_dir,
            collision: CollisionPolicy::Rename,
            quality,
            cover_embed: false,
            playlist_id: None,
            download_url,
        },
        bytes_done: 0,
        bytes_total: None,
        attempts: 0,
        error: None,
        temp_path: Some(paths::downloads_dir(&root).join(&part_name)),
        final_path: None,
        created_at: now,
        updated_at: now,
    };

    {
        let connection = state
            .db
            .lock()
            .map_err(|_| AppError::Internal("数据库连接锁被毒化".to_string()))?;
        repo::insert_task(&connection, &task)?;
    }

    let cancel = state
        .tasks
        .register(id)
        .ok_or_else(|| AppError::Internal("任务 id 冲突，请重试".to_string()))?;
    let manager = state.tasks.clone();
    let config = RunConfig {
        app: app.clone(),
        db_path: state.db_path.clone(),
        root,
        root_id,
        task: task.clone(),
        cancel,
        permits: state.tasks.permits(),
        client: state.client.clone(),
        throttle: state.throttle.clone(),
    };

    tauri::async_runtime::spawn(async move {
        runner::run(config).await;
        manager.finish(id);
    });

    Ok(task)
}

/// 直链入队：URL 本身就是要下载的地址
#[tauri::command]
pub fn add_direct_download_task(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    title: Option<String>,
    artist: Option<String>,
) -> AppResult<DownloadTask> {
    let parsed = reqwest::Url::parse(url.trim())
        .map_err(|err| AppError::BadInput(format!("不是合法的 URL：{err}")))?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AppError::BadInput("只支持 http/https 直链".to_string()));
    }

    let final_title = title
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| filename_from_url(&parsed))
        .unwrap_or_else(|| UNKNOWN_TITLE.to_string());

    let final_artist = artist
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| UNKNOWN_ARTIST.to_string());

    enqueue_download(
        &app,
        &state,
        SourceId::Direct,
        parsed.as_str().to_string(),
        final_title,
        final_artist,
        None,
        // 直链源没有音质档位；用默认档占位（目录模板基本不引用 {quality}）
        QualityPref::default(),
    )
}

/// 网易云入队结果。
///
/// `quality` 是**实际生效**的档位，`fallback` 说明为什么降级（None = 用户选的档位拿到了）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeteaseEnqueue {
    pub task: DownloadTask,
    pub quality: QualityPref,
    pub fallback: Option<QualityFallback>,
}

/// 网易云入队：先解析出带签名的 CDN 直链，再走同一条下载流水线
#[tauri::command]
pub async fn add_netease_download_task(
    app: AppHandle,
    state: State<'_, AppState>,
    song_id: String,
    title: Option<String>,
    artist: Option<String>,
    quality: Option<String>,
) -> AppResult<NeteaseEnqueue> {
    let song_id = song_id.trim().to_string();
    if song_id.is_empty() {
        return Err(AppError::BadInput("缺少歌曲 id".to_string()));
    }

    let requested = QualityPref::parse(quality.as_deref().unwrap_or_default());

    let adapter = state.netease.clone();
    // 解析 + 校验 + 降级链都在适配器里完成，快照记实际档位
    let resolved = adapter.resolve_stream(&song_id, requested).await?;

    let final_title = title
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| UNKNOWN_TITLE.to_string());
    let final_artist = artist
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| UNKNOWN_ARTIST.to_string());

    let task = enqueue_download(
        &app,
        &state,
        SourceId::Netease,
        song_id,
        final_title,
        final_artist,
        Some(resolved.stream.url),
        resolved.quality,
    )?;

    Ok(NeteaseEnqueue {
        task,
        quality: resolved.quality,
        fallback: resolved.fallback,
    })
}

/// 试听结果：本地代理地址 + 实际档位
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeteaseStream {
    pub url: String,
    pub quality: QualityPref,
    pub fallback: Option<QualityFallback>,
}

/// 在线试听：解析直链 → 注册本地票据 → 返回 127.0.0.1 地址。
///
/// 前端 `<audio>` 永远不会接触真实 CDN 地址，也就不会因为缺 Referer 被 403。
#[tauri::command]
pub async fn netease_stream_url(
    state: State<'_, AppState>,
    song_id: String,
) -> AppResult<NeteaseStream> {
    let song_id = song_id.trim().to_string();
    if song_id.is_empty() {
        return Err(AppError::BadInput("缺少歌曲 id".to_string()));
    }

    let adapter = state.netease.clone();
    // 试听固定按无损解析（单曲一次请求，别为预览拉几百兆母带）
    let resolved = adapter
        .resolve_stream(&song_id, QualityPref::Lossless)
        .await?;

    let token = state
        .proxy
        .add_stream(resolved.stream.url, resolved.stream.referer);

    Ok(NeteaseStream {
        url: state.proxy.stream_url(&token),
        quality: resolved.quality,
        fallback: resolved.fallback,
    })
}

/// 歌词：网易云走 API，本地文件读同名 `.lrc`
#[tauri::command]
pub async fn get_lyrics(
    app: AppHandle,
    state: State<'_, AppState>,
    track_id: String,
    local_path: Option<String>,
) -> AppResult<String> {
    if let Some(raw) = local_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        let root = {
            let connection = lock_db(&state)?;
            paths::library_root(&app, &connection)?.1
        };

        let candidate = std::path::PathBuf::from(raw);
        let absolute = if candidate.is_absolute() {
            candidate
        } else {
            root.join(candidate)
        };

        // 传进来的既可能是音频文件，也可能直接是 .lrc
        let is_lrc = absolute
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("lrc"))
            .unwrap_or(false);

        let lrc = if is_lrc {
            absolute.clone()
        } else {
            absolute.with_extension("lrc")
        };

        if lrc.is_file() {
            return Ok(std::fs::read_to_string(&lrc)?);
        }

        // ② 内嵌歌词（USLT / LYRICS）
        if !is_lrc && absolute.is_file() {
            if let Ok(embedded) = scrape::read_lyrics(&absolute) {
                if !embedded.trim().is_empty() {
                    return Ok(embedded);
                }
            }
        }

        // 本地文件没有同名 lrc / 内嵌歌词时，不再误打网络接口
        if is_lrc {
            return Err(AppError::BadInput(
                "没有找到同名 .lrc 歌词文件".to_string(),
            ));
        }
    }

    let track_id = track_id.trim().to_string();
    if track_id.is_empty() || track_id.starts_with("local:") {
        return Ok(String::new());
    }

    // ③ 在线网络 API
    let adapter = state.netease.clone();
    adapter.lyric(&track_id).await
}

/// 各音源解析直链的唯一入口：首次入队与「重新下载」共用。
async fn resolve_source_stream(
    state: &State<'_, AppState>,
    source: SourceId,
    track_id: &str,
    requested: QualityPref,
) -> AppResult<(ResolvedStream, QualityPref, Option<QualityFallback>)> {
    match source {
        SourceId::Netease => {
            let adapter = state.netease.clone();
            let info = adapter.resolve_stream(track_id, requested).await?;
            Ok((info.stream, info.quality, info.fallback))
        }
        SourceId::Bilibili => {
            let adapter = BilibiliAdapter::new(state.client.as_ref().clone());
            let stream = adapter.get_stream_url(track_id, requested).await?;
            Ok((stream, requested, None))
        }
        SourceId::Navidrome => {
            let config = {
                let connection = lock_db(state)?;
                NavidromeConfig::from_settings(&connection).ok_or_else(|| {
                    AppError::BadInput("尚未配置 Navidrome 服务器".to_string())
                })?
            };
            let adapter = NavidromeAdapter::new(state.client.as_ref().clone(), config);
            let stream = adapter.get_stream_url(track_id, requested).await?;
            Ok((stream, requested, None))
        }
        SourceId::Qq => {
            let adapter = QqAdapter::new(state.client.as_ref().clone());
            let stream = adapter.get_stream_url(track_id, requested).await?;
            // 匿名 vkey 只给 C400 m4a（128K）。快照按 standard 记，
            // 避免任务页把 QQ 任务显示成"无损"；降级提示交给前端 QQ 专用文案。
            Ok((stream, QualityPref::Standard, None))
        }
        SourceId::Direct => Err(AppError::BadInput(
            "直链请走 add_direct_download_task".to_string(),
        )),
    }
}

/// 多源统一入队：按来源路由到各自的直链解析。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn add_source_download_task(
    app: AppHandle,
    state: State<'_, AppState>,
    source: String,
    track_id: String,
    title: Option<String>,
    artist: Option<String>,
    quality: Option<String>,
) -> AppResult<NeteaseEnqueue> {
    let source_id: SourceId = source
        .parse()
        .map_err(|_| AppError::BadInput(format!("未知音源: {source}")))?;

    let track_id = track_id.trim().to_string();
    if track_id.is_empty() {
        return Err(AppError::BadInput("缺少曲目 id".to_string()));
    }

    let requested = QualityPref::parse(quality.as_deref().unwrap_or_default());

    let (resolved, effective, fallback) =
        resolve_source_stream(&state, source_id, &track_id, requested).await?;

    // B站要 Referer、Navidrome URL 带密码 —— 都走本地代理，
    // 既补齐防盗链头，也不把鉴权信息写进任务快照。
    let download_url = match source_id {
        SourceId::Bilibili | SourceId::Navidrome | SourceId::Qq => {
            let token = state.proxy.add_stream(resolved.url, resolved.referer);
            state.proxy.stream_url(&token)
        }
        _ => resolved.url,
    };

    let final_title = title
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| UNKNOWN_TITLE.to_string());
    let final_artist = artist
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| UNKNOWN_ARTIST.to_string());

    let task = enqueue_download(
        &app,
        &state,
        source_id,
        track_id,
        final_title,
        final_artist,
        Some(download_url),
        effective,
    )?;

    Ok(NeteaseEnqueue {
        task,
        quality: effective,
        fallback,
    })
}

// ───────────────────────────── Navidrome 配置 ─────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavidromeStatus {
    pub configured: bool,
    pub url: String,
    pub user: String,
}

#[tauri::command]
pub fn navidrome_status(state: State<'_, AppState>) -> AppResult<NavidromeStatus> {
    let connection = lock_db(&state)?;
    let url = repo::get_setting(&connection, navidrome::URL_KEY)?.unwrap_or_default();
    let user = repo::get_setting(&connection, navidrome::USER_KEY)?.unwrap_or_default();

    Ok(NavidromeStatus {
        configured: !url.trim().is_empty(),
        url,
        user,
    })
}

/// 登录 Navidrome：先用 ping 验证，成功后落库（密码之后不再回传前端）
#[tauri::command]
pub async fn navidrome_login(
    state: State<'_, AppState>,
    url: String,
    user: String,
    password: String,
) -> AppResult<NavidromeStatus> {
    let base = url.trim().trim_end_matches('/').to_string();
    if !base.starts_with("http://") && !base.starts_with("https://") {
        return Err(AppError::BadInput(
            "服务器地址需以 http:// 或 https:// 开头".to_string(),
        ));
    }

    let user = user.trim().to_string();
    let config = NavidromeConfig {
        base: base.clone(),
        user: user.clone(),
        password: password.clone(),
    };

    let adapter = NavidromeAdapter::new(state.client.as_ref().clone(), config);
    adapter.ping().await?;

    {
        let connection = lock_db(&state)?;
        repo::set_setting(&connection, navidrome::URL_KEY, &base)?;
        repo::set_setting(&connection, navidrome::USER_KEY, &user)?;
        repo::set_setting(&connection, navidrome::PASS_KEY, &password)?;
    }

    Ok(NavidromeStatus {
        configured: true,
        url: base,
        user,
    })
}

#[tauri::command]
pub fn set_navidrome_settings(
    state: State<'_, AppState>,
    url: String,
    user: String,
    password: String,
) -> AppResult<NavidromeStatus> {
    let url = url.trim().trim_end_matches('/').to_string();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(AppError::BadInput(
            "服务器地址需以 http:// 或 https:// 开头".to_string(),
        ));
    }

    {
        let connection = lock_db(&state)?;
        repo::set_setting(&connection, navidrome::URL_KEY, &url)?;
        repo::set_setting(&connection, navidrome::USER_KEY, user.trim())?;
        repo::set_setting(&connection, navidrome::PASS_KEY, &password)?;
    }

    Ok(NavidromeStatus {
        configured: true,
        url,
        user: user.trim().to_string(),
    })
}

// ───────────────────────────── 网易云账号 ─────────────────────────────

const COOKIE_KEY: &str = "netease_cookie";
const PROFILE_KEY: &str = "netease_profile";
const DEFAULT_QUALITY_KEY: &str = "netease_default_quality";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeteaseStatus {
    pub logged_in: bool,
    pub profile: Option<NeteaseProfile>,
}

fn store_cookie(state: &AppState, cookie: Option<&str>) -> AppResult<()> {
    let connection = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("数据库连接锁被毒化".to_string()))?;
    match cookie {
        Some(value) if !value.is_empty() => repo::set_setting(&connection, COOKIE_KEY, value),
        _ => {
            repo::delete_setting(&connection, COOKIE_KEY)?;
            repo::delete_setting(&connection, PROFILE_KEY)
        }
    }
}

/// 申请扫码 unikey。前端把它拼成 `https://music.163.com/login?codekey=<key>` 并渲染二维码
#[tauri::command]
pub async fn netease_qr_key(state: State<'_, AppState>) -> AppResult<String> {
    let adapter = state.netease.clone();
    adapter.qr_key().await
}

/// 轮询扫码状态；803 成功时落盘 cookie
#[tauri::command]
pub async fn netease_qr_check(state: State<'_, AppState>, key: String) -> AppResult<QrState> {
    let adapter = state.netease.clone();
    let result = adapter.qr_check(&key).await?;

    if result.code == 803 {
        if let Some(cookie) = adapter.cookie() {
            store_cookie(&state, Some(&cookie))?;
        }
        // cookie 生效后立刻缓存一份资料，前端无需再等一次请求
        if let Ok(Some(profile)) = adapter.profile().await {
            if let Ok(json) = serde_json::to_string(&profile) {
                let connection = state
                    .db
                    .lock()
                    .map_err(|_| AppError::Internal("数据库连接锁被毒化".to_string()))?;
                let _ = repo::set_setting(&connection, PROFILE_KEY, &json);
            }
        }
    }

    Ok(result)
}

#[tauri::command]
pub async fn netease_status(state: State<'_, AppState>) -> AppResult<NeteaseStatus> {
    let adapter = state.netease.clone();

    if !adapter.is_logged_in() {
        return Ok(NeteaseStatus {
            logged_in: false,
            profile: None,
        });
    }

    // 优先用线上资料；失败则回落到缓存
    match adapter.profile().await {
        Ok(Some(profile)) => Ok(NeteaseStatus {
            logged_in: true,
            profile: Some(profile),
        }),
        _ => {
            let cached = {
                let connection = state
                    .db
                    .lock()
                    .map_err(|_| AppError::Internal("数据库连接锁被毒化".to_string()))?;
                repo::get_setting(&connection, PROFILE_KEY)?
            };

            let profile = cached
                .and_then(|raw| serde_json::from_str::<NeteaseProfile>(&raw).ok());

            Ok(NeteaseStatus {
                logged_in: true,
                profile,
            })
        }
    }
}

#[tauri::command]
pub async fn netease_logout(state: State<'_, AppState>) -> AppResult<()> {
    state.netease.set_cookie(None);
    store_cookie(&state, None)
}

// ───────────────────────────── M8：B 站扫码 ─────────────────────────────

fn store_bilibili_cookie(state: &AppState, cookie: Option<&str>) -> AppResult<()> {
    let connection = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("数据库连接锁被毒化".to_string()))?;

    match cookie {
        Some(value) if !value.is_empty() => {
            repo::set_setting(&connection, crate::provider::auth::COOKIE_KEY, value)
        }
        _ => repo::delete_setting(&connection, crate::provider::auth::COOKIE_KEY),
    }
}

#[tauri::command]
pub async fn bilibili_qr_start(state: State<'_, AppState>) -> AppResult<QrLoginStart> {
    state.bilibili.start_qr().await
}

#[tauri::command]
pub async fn bilibili_qr_poll(
    state: State<'_, AppState>,
    key: String,
) -> AppResult<QrLoginState> {
    let result = state.bilibili.poll_qr(&key).await?;

    if result.code == 0 {
        if let Some(cookie) = state.bilibili.cookie() {
            store_bilibili_cookie(&state, Some(&cookie))?;
        }
    }

    Ok(result)
}

#[tauri::command]
pub async fn bilibili_status(state: State<'_, AppState>) -> AppResult<BilibiliStatus> {
    state.bilibili.status().await
}

#[tauri::command]
pub async fn bilibili_logout(state: State<'_, AppState>) -> AppResult<()> {
    state.bilibili.logout().await?;
    store_bilibili_cookie(&state, None)
}

// ───────────────────────────── Web 登录兜底（M9） ─────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebLoginState {
    pub logged_in: bool,
    pub message: String,
}

fn web_login_target(source: &str) -> AppResult<(String, &'static str, &'static str)> {
    // (window label, 登录页, 读取 Cookie 的 URL)
    match source {
        "netease" => Ok((
            "web-login-netease".to_string(),
            "https://music.163.com/login?targetUrl=%2F",
            "https://music.163.com/",
        )),
        "bilibili" => Ok((
            "web-login-bilibili".to_string(),
            "https://passport.bilibili.com/login",
            "https://www.bilibili.com/",
        )),
        "qq" => Ok((
            "web-login-qq".to_string(),
            "https://y.qq.com/",
            "https://y.qq.com/",
        )),
        _ => Err(AppError::BadInput(format!("未知登录源: {source}"))),
    }
}

#[tauri::command]
pub fn open_web_login(app: AppHandle, source: String) -> AppResult<()> {
    let (label, login_url, _) = web_login_target(&source)?;

    if let Some(window) = app.get_webview_window(&label) {
        window.set_focus()?;
        return Ok(());
    }

    let url = login_url
        .parse::<tauri::Url>()
        .map_err(|err| AppError::BadInput(format!("登录 URL 无效: {err}")))?;

    WebviewWindowBuilder::new(&app, label, WebviewUrl::External(url))
        .title("登录")
        .inner_size(420.0, 720.0)
        .build()?;

    Ok(())
}

/// 从登录 WebView 的 CookieManager 抓核心 Cookie 并落库。
#[tauri::command]
pub fn capture_web_login(
    app: AppHandle,
    state: State<'_, AppState>,
    source: String,
) -> AppResult<WebLoginState> {
    let (label, _, cookie_url) = web_login_target(&source)?;
    let Some(window) = app.get_webview_window(&label) else {
        return Ok(WebLoginState {
            logged_in: false,
            message: "登录窗口已关闭".to_string(),
        });
    };

    let url = cookie_url
        .parse::<tauri::Url>()
        .map_err(|err| AppError::BadInput(format!("Cookie URL 无效: {err}")))?;
    let cookies = window.cookies_for_url(url)?;

    let names: &[&str] = match source.as_str() {
        "netease" => &["MUSIC_U", "__csrf"],
        "bilibili" => &[
            "SESSDATA",
            "bili_jct",
            "DedeUserID",
            "DedeUserID__ckMd5",
        ],
        "qq" => &[
            "qqmusic_key",
            "qm_keyst",
            "uin",
            "qqmusic_uin",
            "p_skey",
        ],
        _ => &[],
    };

    let mut map: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for cookie in cookies {
        let name = cookie.name().to_string();
        if names.contains(&name.as_str()) {
            map.insert(name, cookie.value().to_string());
        }
    }

    let required = match source.as_str() {
        "netease" => "MUSIC_U",
        "bilibili" => "SESSDATA",
        "qq" => {
            if map.contains_key("qqmusic_key") {
                "qqmusic_key"
            } else {
                "qm_keyst"
            }
        }
        _ => "",
    };

    if !map.contains_key(required) {
        return Ok(WebLoginState {
            logged_in: false,
            message: "尚未检测到登录 Cookie，请完成登录".to_string(),
        });
    }

    let cookie = map
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");

    match source.as_str() {
        "netease" => {
            state.netease.set_cookie(Some(cookie.clone()));
            store_cookie(&state, Some(&cookie))?;
        }
        "bilibili" => {
            state.bilibili.set_cookie(Some(cookie.clone()));
            store_bilibili_cookie(&state, Some(&cookie))?;
        }
        "qq" => {
            crate::provider::qq::set_login_cookie(Some(cookie.clone()));
            let connection = lock_db(&state)?;
            repo::set_setting(&connection, "qq_cookie", &cookie)?;
        }
        _ => {}
    }

    Ok(WebLoginState {
        logged_in: true,
        message: "登录成功".to_string(),
    })
}

#[tauri::command]
pub fn close_web_login(app: AppHandle, source: String) -> AppResult<()> {
    let (label, _, _) = web_login_target(&source)?;
    if let Some(window) = app.get_webview_window(&label) {
        window.close()?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QqLoginStatus {
    pub logged_in: bool,
}

#[tauri::command]
pub fn qq_status(state: State<'_, AppState>) -> AppResult<QqLoginStatus> {
    let connection = lock_db(&state)?;
    let cookie = repo::get_setting(&connection, "qq_cookie")?;
    Ok(QqLoginStatus {
        logged_in: cookie.map(|value| !value.trim().is_empty()).unwrap_or(false),
    })
}

/// 多 P 选集：返回 pagelist；单 P 时前端直接入队。
#[tauri::command]
pub async fn bilibili_pages(
    state: State<'_, AppState>,
    bvid: String,
) -> AppResult<Vec<BiliPage>> {
    let bvid = bvid.trim().to_string();
    if bvid.is_empty() {
        return Err(AppError::BadInput("缺少 bvid".to_string()));
    }
    BilibiliAdapter::new(state.client.as_ref().clone())
        .pages(&bvid)
        .await
}

/// 默认下载音质：settings 表里存 level 名，读不到就回落到默认档（无损）
#[tauri::command]
pub fn netease_default_quality(state: State<'_, AppState>) -> AppResult<QualityPref> {
    let stored = {
        let connection = state
            .db
            .lock()
            .map_err(|_| AppError::Internal("数据库连接锁被毒化".to_string()))?;
        repo::get_setting(&connection, DEFAULT_QUALITY_KEY)?
    };

    Ok(stored.as_deref().map(QualityPref::parse).unwrap_or_default())
}

/// 持久化默认下载音质。`default` 不是可落库的档位，按默认档（无损）处理。
#[tauri::command]
pub fn set_netease_default_quality(
    state: State<'_, AppState>,
    quality: String,
) -> AppResult<QualityPref> {
    let parsed = QualityPref::parse(&quality);

    {
        let connection = state
            .db
            .lock()
            .map_err(|_| AppError::Internal("数据库连接锁被毒化".to_string()))?;
        repo::set_setting(&connection, DEFAULT_QUALITY_KEY, parsed.level())?;
    }

    Ok(parsed)
}

// ───────────────────────────── 任务管理 ─────────────────────────────

#[tauri::command]
pub fn list_download_tasks(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> AppResult<Vec<DownloadTask>> {
    let connection = lock_db(&state)?;
    repo::list_tasks(&connection, limit.unwrap_or(50).min(200))
}

#[tauri::command]
pub fn cancel_download_task(state: State<'_, AppState>, id: String) -> AppResult<bool> {
    let uuid = id
        .parse::<Uuid>()
        .map_err(|_| AppError::BadInput("无效的任务 id".to_string()))?;
    // false = 任务已经结束，前端据此提示"来不及了"
    Ok(state.tasks.cancel(uuid))
}

/// 删除一条历史下载记录（已完成 / 已失败 / 已取消）。
///
/// 只清理 `download_tasks` 行，不删除 `.part`，也不动已归档的音频文件。
#[tauri::command]
pub fn delete_download_task(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let uuid = id
        .parse::<Uuid>()
        .map_err(|_| AppError::BadInput("无效的任务 id".to_string()))?;

    let connection = lock_db(&state)?;
    let task = repo::get_task(&connection, uuid)?
        .ok_or_else(|| AppError::BadInput("任务不存在".to_string()))?;

    if !task.state.is_terminal() {
        return Err(AppError::BadInput(
            "任务仍在进行中，暂停并等待状态结束后再删除".to_string(),
        ));
    }

    if !repo::delete_task(&connection, uuid)? {
        return Err(AppError::BadInput(
            "任务状态已变化，请刷新后重试".to_string(),
        ));
    }

    Ok(())
}

/// 继续一个已失败/取消的任务。
///
/// 关键点：**复用同一个 task id**，所以 `.part` 路径不变，
/// 下载层会直接从文件末尾用 Range 续传，而不是从头再来。
#[tauri::command]
pub fn resume_download_task(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> AppResult<DownloadTask> {
    let uuid = id
        .parse::<Uuid>()
        .map_err(|_| AppError::BadInput("无效的任务 id".to_string()))?;

    let mut task = {
        let connection = lock_db(&state)?;
        repo::get_task(&connection, uuid)?
            .ok_or_else(|| AppError::BadInput("任务不存在".to_string()))?
    };

    {
        let connection = lock_db(&state)?;
        repo::reset_for_resume(&connection, uuid, now_ms())?;
    }

    let (root_id, root) = {
        let connection = lock_db(&state)?;
        paths::library_root(&app, &connection)?
    };

    task.state = TaskState::Queued;
    task.error = None;
    task.updated_at = now_ms();
    if task.temp_path.is_none() {
        // 老数据没有 temp_path 时按 uuid 补一个，保证续传落在同一路径
        task.temp_path = Some(paths::downloads_dir(&root).join(format!("{uuid}.part")));
    }

    let cancel = state
        .tasks
        .register(uuid)
        .ok_or_else(|| AppError::Internal("任务已在运行".to_string()))?;
    let manager = state.tasks.clone();
    let config = RunConfig {
        app: app.clone(),
        db_path: state.db_path.clone(),
        root,
        root_id,
        task: task.clone(),
        cancel,
        permits: state.tasks.permits(),
        client: state.client.clone(),
        throttle: state.throttle.clone(),
    };

    tauri::async_runtime::spawn(async move {
        runner::run(config).await;
        manager.finish(uuid);
    });

    Ok(task)
}

/// 重新下载一个终态任务：复用任务 id，重新解析直链、清空进度，
/// 并把快照的落位策略改成 Overwrite（成功时覆盖旧归档文件）。
#[tauri::command]
pub async fn restart_download_task(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> AppResult<DownloadTask> {
    let uuid = id
        .parse::<Uuid>()
        .map_err(|_| AppError::BadInput("无效的任务 id".to_string()))?;

    let mut task = {
        let connection = lock_db(&state)?;
        repo::get_task(&connection, uuid)?
            .ok_or_else(|| AppError::BadInput("任务不存在".to_string()))?
    };

    if !task.state.is_terminal() {
        return Err(AppError::BadInput(
            "任务仍在进行中，暂时不能重新下载".to_string(),
        ));
    }

    let requested = task.snapshot.quality;
    let (resolved, effective) = match task.source {
        SourceId::Direct => {
            let url = task
                .snapshot
                .download_url
                .clone()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| {
                    let raw = task.source_track_id.trim().to_string();
                    if raw.starts_with("http") {
                        Some(raw)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| AppError::BadInput("直链任务缺少原始下载地址".to_string()))?;

            (ResolvedStream { url, referer: None }, requested)
        }
        source => {
            let (stream, quality, _fallback) =
                resolve_source_stream(&state, source, &task.source_track_id, requested).await?;
            (stream, quality)
        }
    };

    // B站/Navidrome/QQ 的鉴权与防盗链统一走本地代理，重新生成票据
    let download_url = match task.source {
        SourceId::Bilibili | SourceId::Navidrome | SourceId::Qq => {
            let token = state.proxy.add_stream(resolved.url, resolved.referer);
            state.proxy.stream_url(&token)
        }
        _ => resolved.url,
    };

    let (root_id, root) = {
        let connection = lock_db(&state)?;
        paths::library_root(&app, &connection)?
    };

    // 重新下载是全新的字节流：旧 .part 必须丢掉，不能从中间续
    if let Some(old_part) = task.temp_path.take() {
        let _ = std::fs::remove_file(old_part);
    }

    let now = now_ms();
    task.state = TaskState::Queued;
    task.error = None;
    task.bytes_done = 0;
    task.bytes_total = None;
    task.attempts = 0;
    task.final_path = None;
    task.temp_path = Some(paths::downloads_dir(&root).join(format!("{uuid}.part")));
    task.snapshot.root_id = root_id;
    task.snapshot.collision = CollisionPolicy::Overwrite;
    task.snapshot.download_url = Some(download_url);
    task.snapshot.quality = effective;
    task.updated_at = now;

    {
        let connection = lock_db(&state)?;
        repo::prepare_redownload(&connection, &task, now)?;
    }

    let cancel = state
        .tasks
        .register(uuid)
        .ok_or_else(|| AppError::Internal("任务已在运行".to_string()))?;
    let manager = state.tasks.clone();
    let config = RunConfig {
        app: app.clone(),
        db_path: state.db_path.clone(),
        root,
        root_id,
        task: task.clone(),
        cancel,
        permits: state.tasks.permits(),
        client: state.client.clone(),
        throttle: state.throttle.clone(),
    };

    tauri::async_runtime::spawn(async move {
        runner::run(config).await;
        manager.finish(uuid);
    });

    Ok(task)
}

// ───────────────────────────── 下载设置（M9） ─────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSettings {
    /// 1–5
    pub concurrency: u32,
    /// bytes/sec，0 = 不限速
    pub speed_limit: u64,
    /// 50 / 100 / 200 / 0（0 = 无限制）
    pub playlist_page_size: u32,
}

fn read_download_settings(state: &AppState) -> AppResult<DownloadSettings> {
    let connection = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("数据库连接锁被毒化".to_string()))?;

    let concurrency = repo::get_setting(&connection, crate::state::CONCURRENCY_KEY)?
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(3)
        .clamp(1, 5);
    let speed_limit = repo::get_setting(&connection, crate::state::SPEED_LIMIT_KEY)?
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let playlist_page_size = repo::get_setting(&connection, crate::state::PLAYLIST_PAGE_SIZE_KEY)?
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| matches!(value, 0 | 50 | 100 | 200))
        .unwrap_or(50);

    Ok(DownloadSettings {
        concurrency,
        speed_limit,
        playlist_page_size,
    })
}

#[tauri::command]
pub fn download_settings(state: State<'_, AppState>) -> AppResult<DownloadSettings> {
    read_download_settings(&state)
}

#[tauri::command]
pub fn set_download_settings(
    state: State<'_, AppState>,
    concurrency: Option<u32>,
    speed_limit: Option<u64>,
    playlist_page_size: Option<u32>,
) -> AppResult<DownloadSettings> {
    let next_concurrency = concurrency.map(|value| value.clamp(1, 5));
    let next_speed = speed_limit.map(|value| value.min(50 * 1024 * 1024));
    let next_playlist = playlist_page_size.map(|value| {
        if matches!(value, 0 | 50 | 100 | 200) {
            value
        } else {
            50
        }
    });

    {
        let connection = lock_db(&state)?;
        if let Some(value) = next_concurrency {
            repo::set_setting(
                &connection,
                crate::state::CONCURRENCY_KEY,
                &value.to_string(),
            )?;
        }
        if let Some(value) = next_speed {
            repo::set_setting(&connection, crate::state::SPEED_LIMIT_KEY, &value.to_string())?;
        }
        if let Some(value) = next_playlist {
            repo::set_setting(
                &connection,
                crate::state::PLAYLIST_PAGE_SIZE_KEY,
                &value.to_string(),
            )?;
        }
    }

    if let Some(value) = next_concurrency {
        state.tasks.set_concurrency(value as usize);
    }
    if let Some(value) = next_speed {
        state.throttle.set_rate(value);
    }

    read_download_settings(&state)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackPage {
    pub tracks: Vec<Track>,
    pub total: u32,
}

/// 分页读取音乐库：默认 50，前端滚到底部再带 offset 请求下一页。
#[tauri::command]
pub fn list_tracks(
    state: State<'_, AppState>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> AppResult<TrackPage> {
    let connection = lock_db(&state)?;
    let limit = limit.unwrap_or(50).clamp(1, 200);
    let offset = offset.unwrap_or(0);
    let total = repo::count_visible_tracks(&connection)?.max(0) as u32;
    let tracks = repo::list_tracks_page(&connection, limit, offset)?;

    Ok(TrackPage { tracks, total })
}

// ───────────────────────────── 文件管理与刮削 ─────────────────────────────

use crate::library::{self, DirListing, TrashEntry};
use crate::scrape::{self, ScrapeCandidate, ScrapePreview};

#[tauri::command]
pub fn list_library_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    space: Option<String>,
    rel_path: Option<String>,
) -> AppResult<DirListing> {
    let connection = lock_db(&state)?;
    let (_, root) = paths::library_root(&app, &connection)?;
    library::list_space(
        &root,
        &connection,
        space.as_deref().unwrap_or("app"),
        rel_path.as_deref().unwrap_or(""),
    )
}

/// 应用私有库根目录的绝对路径（前端用于解析手输路径）
#[tauri::command]
pub fn library_root_path(app: AppHandle, state: State<'_, AppState>) -> AppResult<String> {
    let connection = lock_db(&state)?;
    let (_, root) = paths::library_root(&app, &connection)?;
    Ok(root.to_string_lossy().to_string())
}

/// 按 id 读取曲目（文件管理器详情跳转编辑/补全用）。
#[tauri::command]
pub fn get_track(state: State<'_, AppState>, track_id: i64) -> AppResult<Track> {
    let connection = lock_db(&state)?;
    let (title, artist, album, rel_path) = repo::get_track_meta(&connection, track_id)?
        .ok_or_else(|| AppError::BadInput("曲目不存在".to_string()))?;

    Ok(Track {
        id: track_id,
        title,
        artist,
        album,
        duration_ms: None,
        bitrate: None,
        size_bytes: None,
        format: None,
        rel_path,
        path_status: PathStatus::Ok,
        sources: Vec::new(),
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPlayInfo {
    pub url: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_url: Option<String>,
    pub lyrics: String,
    pub duration_ms: Option<u64>,
}

/// 本地播放完整载荷：URL + 标签 + 内嵌封面 + LRC。
#[tauri::command]
pub fn track_play_info(
    app: AppHandle,
    state: State<'_, AppState>,
    track_id: i64,
) -> AppResult<LocalPlayInfo> {
    use lofty::file::TaggedFileExt;
    use lofty::prelude::{Accessor, AudioFile};

    let (root, rel_path, db_title, db_artist, db_album) = {
        let connection = lock_db(&state)?;
        let (_, root) = paths::library_root(&app, &connection)?;
        let (title, artist, album, rel) = repo::get_track_meta(&connection, track_id)?
            .ok_or_else(|| AppError::BadInput("曲目不存在".to_string()))?;
        (root, rel, title, artist, album)
    };

    let path = root.join(&rel_path);
    if !path.is_file() {
        return Err(AppError::BadInput("文件已丢失，无法播放".to_string()));
    }

    let mut title = db_title;
    let mut artist = db_artist;
    let mut album = db_album;
    let mut cover_url = None;
    let mut duration_ms = None;

    if let Ok(tagged) = lofty::probe::Probe::open(&path).and_then(|probe| Ok(probe.read()?)) {
        duration_ms = Some(tagged.properties().duration().as_millis() as u64);
        if let Some(tag) = tagged.primary_tag() {
            if let Some(value) = tag.title() {
                if !value.trim().is_empty() {
                    title = value.to_string();
                }
            }
            if let Some(value) = tag.artist() {
                if !value.trim().is_empty() {
                    artist = value.to_string();
                }
            }
            if let Some(value) = tag.album() {
                if !value.trim().is_empty() {
                    album = value.to_string();
                }
            }
            if let Some(picture) = tag.pictures().first() {
                let data = picture.data();
                if !data.is_empty() {
                    let mime = picture
                        .mime_type()
                        .map(|m| m.to_string())
                        .unwrap_or_else(|| "image/jpeg".to_string());
                    cover_url = Some(format!(
                        "data:{mime};base64,{}",
                        base64_encode(data)
                    ));
                }
            }
        }
    }

    // 优先内嵌歌词（编辑表单已确认能读到），没有再读同名 .lrc
    let embedded = scrape::read_lyrics(&path).unwrap_or_default();
    let lyrics = if !embedded.trim().is_empty() {
        embedded
    } else {
        std::fs::read_to_string(path.with_extension("lrc")).unwrap_or_default()
    };

    let token = state.proxy.add_file(path);
    Ok(LocalPlayInfo {
        url: state.proxy.file_url(&token),
        title,
        artist,
        album,
        cover_url,
        lyrics,
        duration_ms,
    })
}

/// 本地播放：返回 127.0.0.1 代理 URL，避免 Android WebView 的 asset 限制。
#[tauri::command]
pub fn track_play_url(
    app: AppHandle,
    state: State<'_, AppState>,
    track_id: i64,
) -> AppResult<String> {
    let (root, rel_path) = {
        let connection = lock_db(&state)?;
        let (_, root) = paths::library_root(&app, &connection)?;
        let (_, _, _, rel) = repo::get_track_meta(&connection, track_id)?
            .ok_or_else(|| AppError::BadInput("曲目不存在".to_string()))?;
        (root, rel)
    };

    let path = root.join(&rel_path);
    if !path.is_file() {
        return Err(AppError::BadInput("文件已丢失，无法播放".to_string()));
    }

    let token = state.proxy.add_file(path);
    Ok(state.proxy.file_url(&token))
}

/// 本地播放：返回绝对路径，前端用 asset protocol 交给 MiniPlayer。
#[tauri::command]
pub fn track_play_path(
    app: AppHandle,
    state: State<'_, AppState>,
    track_id: i64,
) -> AppResult<String> {
    let (root, rel_path) = {
        let connection = lock_db(&state)?;
        let (_, root) = paths::library_root(&app, &connection)?;
        let (_, _, _, rel) = repo::get_track_meta(&connection, track_id)?
            .ok_or_else(|| AppError::BadInput("曲目不存在".to_string()))?;
        (root, rel)
    };

    let path = root.join(&rel_path);
    if !path.is_file() {
        return Err(AppError::BadInput("文件已丢失，无法播放".to_string()));
    }

    Ok(path.to_string_lossy().to_string())
}

/// 修改音乐库根目录：必须是绝对路径且当前可写。
///
/// 之后的下载归档、扫描、回收站都会以新路径为准（`paths::library_root`
/// 每次调用都会重新读 settings，所以不需要重启）。
#[tauri::command]
pub fn set_library_root_path(state: State<'_, AppState>, path: String) -> AppResult<String> {
    let trimmed = path.trim().to_string();
    if trimmed.is_empty() {
        return Err(AppError::BadInput("路径不能为空".to_string()));
    }

    let root = std::path::PathBuf::from(&trimmed);
    if !root.is_absolute() {
        return Err(AppError::BadInput(
            "请输入绝对路径，例如 /storage/emulated/0/MyMusic".to_string(),
        ));
    }

    std::fs::create_dir_all(&root)?;

    // 能建目录 ≠ 能写文件（只读挂载 / 权限被拒都会在这里暴露）
    let probe = root.join(".sonotheca_write_probe");
    std::fs::write(&probe, b"ok")
        .map_err(|err| AppError::BadInput(format!("该目录不可写：{err}")))?;
    let _ = std::fs::remove_file(&probe);

    let stored = serde_json::to_string(&trimmed)
        .map_err(|err| AppError::Internal(format!("路径序列化失败: {err}")))?;

    {
        let connection = lock_db(&state)?;
        repo::set_setting(&connection, paths::LIBRARY_ROOT_SETTING, &stored)?;
    }

    Ok(trimmed)
}

/// mode: "move" | "copy"
#[tauri::command]
pub fn transfer_entries(
    app: AppHandle,
    state: State<'_, AppState>,
    from_space: Option<String>,
    rel_paths: Vec<String>,
    to_space: Option<String>,
    target_dir: String,
    mode: String,
) -> AppResult<u32> {
    let connection = lock_db(&state)?;
    let (_, root) = paths::library_root(&app, &connection)?;
    library::transfer_between(
        &root,
        &connection,
        from_space.as_deref().unwrap_or("app"),
        &rel_paths,
        to_space.as_deref().unwrap_or("app"),
        &target_dir,
        &mode,
    )
}

/// 启动后台扫描；进度通过 `library://scan-progress` 推送
#[tauri::command]
pub fn scan_library_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    rel_path: Option<String>,
) -> AppResult<()> {
    let (root_id, root) = {
        let connection = lock_db(&state)?;
        paths::library_root(&app, &connection)?
    };

    let db_path = state.db_path.clone();
    let handle = app.clone();
    let rel = rel_path.unwrap_or_default();

    tauri::async_runtime::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            library::scan_tree(&handle, &db_path, &root, root_id, &rel);
        })
        .await;
    });

    Ok(())
}

#[tauri::command]
pub fn trash_track(app: AppHandle, state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let connection = lock_db(&state)?;
    let (_, root) = paths::library_root(&app, &connection)?;
    library::trash_track(&root, &connection, id)
}

#[tauri::command]
pub fn list_trash(state: State<'_, AppState>) -> AppResult<Vec<TrashEntry>> {
    let connection = lock_db(&state)?;
    library::list_trash(&connection)
}

#[tauri::command]
pub fn restore_trash(app: AppHandle, state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let connection = lock_db(&state)?;
    let (_, root) = paths::library_root(&app, &connection)?;
    library::restore_trash(&root, &connection, id)
}

#[tauri::command]
pub fn purge_trash(app: AppHandle, state: State<'_, AppState>, id: Option<i64>) -> AppResult<u32> {
    let connection = lock_db(&state)?;
    let (_, root) = paths::library_root(&app, &connection)?;
    library::purge_trash(&root, &connection, id)
}

/// 补全元数据的动态路由：登录平台优先，全部失败后走 iTunes 开源兜底。
async fn scrape_preview_for(
    state: &State<'_, AppState>,
    title: &str,
    artist: &str,
    album: &str,
) -> AppResult<ScrapePreview> {
    if state.netease.is_logged_in() {
        if let Ok(preview) = scrape::preview_with_source(
            state.netease.as_ref(),
            "netease",
            title,
            artist,
            album,
        )
        .await
        {
            if !preview.candidates.is_empty() {
                return Ok(preview);
            }
        }
    }

    let qq_logged = {
        let connection = lock_db(state)?;
        repo::get_setting(&connection, "qq_cookie")?
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    };
    if qq_logged {
        let adapter = QqAdapter::new(state.client.as_ref().clone());
        if let Ok(preview) =
            scrape::preview_with_source(&adapter, "qq", title, artist, album).await
        {
            if !preview.candidates.is_empty() {
                return Ok(preview);
            }
        }
    }

    if state.bilibili.cookie().is_some() {
        let adapter = BilibiliAdapter::new(state.client.as_ref().clone());
        if let Ok(preview) =
            scrape::preview_with_source(&adapter, "bilibili", title, artist, album).await
        {
            if !preview.candidates.is_empty() {
                return Ok(preview);
            }
        }
    }

    match scrape::itunes_preview(state.client.as_ref(), title, artist, album).await {
        Ok(preview) => Ok(preview),
        Err(_) => Ok(scrape::empty_preview(title, artist, album)),
    }
}

/// 只读预览：拿到当前信息与候选列表，不改文件
#[tauri::command]
pub async fn scrape_preview(
    state: State<'_, AppState>,
    track_id: i64,
) -> AppResult<ScrapePreview> {
    let (title, artist, album) = {
        let connection = lock_db(&state)?;
        let (title, artist, album, _) = repo::get_track_meta(&connection, track_id)?
            .ok_or_else(|| AppError::BadInput("曲目不存在".to_string()))?;
        (title, artist, album)
    };

    scrape_preview_for(&state, &title, &artist, &album).await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MetadataProgress {
    current: u32,
    total: u32,
}

fn emit_metadata_progress(app: &AppHandle, current: u32, total: u32) {
    let _ = app.emit(
        "metadata_progress",
        MetadataProgress { current, total },
    );
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchScrapeResult {
    pub applied: u32,
    pub skipped: u32,
    pub failed: u32,
}

fn track_from_written(
    track_id: i64,
    title: &str,
    artist: &str,
    album: &str,
    rel_path: &str,
    path: &std::path::Path,
) -> Track {
    let size_bytes = std::fs::metadata(path)
        .ok()
        .map(|meta| meta.len() as i64);
    let format = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());

    Track {
        id: track_id,
        title: title.to_string(),
        artist: artist.to_string(),
        album: album.to_string(),
        duration_ms: None,
        bitrate: None,
        size_bytes,
        format,
        rel_path: rel_path.to_string(),
        path_status: PathStatus::Ok,
        sources: Vec::new(),
    }
}

/// 单曲写回的实际逻辑：single / batch 共用。
async fn apply_scrape_candidate(
    app: &AppHandle,
    state: &State<'_, AppState>,
    track_id: i64,
    candidate: &ScrapeCandidate,
) -> AppResult<Track> {
    let (root, rel_path) = {
        let connection = lock_db(state)?;
        let (_, root) = paths::library_root(app, &connection)?;
        let (_, _, _, rel) = repo::get_track_meta(&connection, track_id)?
            .ok_or_else(|| AppError::BadInput("曲目不存在".to_string()))?;
        (root, rel)
    };

    let path = root.join(&rel_path);
    if !path.is_file() {
        return Err(AppError::BadInput("文件已丢失，无法写入标签".to_string()));
    }

    scrape::apply(&state.client, &path, candidate).await?;

    {
        let connection = lock_db(state)?;
        repo::update_track_meta(
            &connection,
            track_id,
            &candidate.title,
            &candidate.artist,
            &candidate.album,
        )?;
    }

    // 不再回读列表：直接基于刚写入的数据构造，避免二次读盘/路径锁冲突
    Ok(track_from_written(
        track_id,
        &candidate.title,
        &candidate.artist,
        &candidate.album,
        &rel_path,
        &path,
    ))
}

/// 用户确认后才写回文件（标签 + 封面）
#[tauri::command]
pub async fn scrape_apply(
    app: AppHandle,
    state: State<'_, AppState>,
    track_id: i64,
    candidate: ScrapeCandidate,
) -> AppResult<Track> {
    emit_metadata_progress(&app, 1, 1);
    apply_scrape_candidate(&app, &state, track_id, &candidate).await
}

/// 手动编辑元数据：Title / Artist / Album，可选替换封面。
#[tauri::command]
pub fn update_track_metadata(
    app: AppHandle,
    state: State<'_, AppState>,
    track_id: i64,
    absolute_path: Option<String>,
    title: String,
    artist: String,
    album: String,
    cover_data_url: Option<String>,
    lyrics: Option<String>,
) -> AppResult<Track> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err(AppError::BadInput("标题不能为空".to_string()));
    }

    let cover = match cover_data_url {
        Some(value) if !value.trim().is_empty() => {
            let (meta, data) = value
                .split_once(',')
                .ok_or_else(|| AppError::BadInput("封面数据格式错误".to_string()))?;
            if !meta.contains("base64") {
                return Err(AppError::BadInput("封面必须是 base64 data URL".to_string()));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|err| AppError::BadInput(format!("封面解码失败: {err}")))?;
            if bytes.len() > 8 * 1024 * 1024 {
                return Err(AppError::BadInput("封面不能超过 8MB".to_string()));
            }
            Some(bytes)
        }
        _ => None,
    };

    let (root, rel_path) = {
        let connection = lock_db(&state)?;
        let (_, root) = paths::library_root(&app, &connection)?;
        let (_, _, _, rel) = repo::get_track_meta(&connection, track_id)?
            .ok_or_else(|| AppError::BadInput("曲目不存在".to_string()))?;
        (root, rel)
    };

    let path = absolute_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join(&rel_path));
    if !path.is_file() {
        return Err(AppError::BadInput("文件已丢失，无法写入标签".to_string()));
    }

    scrape::write_manual(
        &path,
        &title,
        artist.trim(),
        album.trim(),
        cover,
        lyrics.as_deref(),
    )?;

    {
        let connection = lock_db(&state)?;
        repo::update_track_meta(&connection, track_id, &title, artist.trim(), album.trim())?;
    }

    // 直接内存构造返回，不回读磁盘/SQLite 列表，避免 "写回后读取曲目失败"
    Ok(track_from_written(
        track_id,
        &title,
        artist.trim(),
        album.trim(),
        &rel_path,
        &path,
    ))
}

/// 读取文件内嵌歌词，供编辑表单回显。
#[tauri::command]
pub fn read_track_lyrics(
    app: AppHandle,
    state: State<'_, AppState>,
    track_id: i64,
) -> AppResult<String> {
    let (root, rel_path) = {
        let connection = lock_db(&state)?;
        let (_, root) = paths::library_root(&app, &connection)?;
        let (_, _, _, rel) = repo::get_track_meta(&connection, track_id)?
            .ok_or_else(|| AppError::BadInput("曲目不存在".to_string()))?;
        (root, rel)
    };

    let path = root.join(&rel_path);
    if !path.is_file() {
        return Err(AppError::BadInput("文件已丢失，无法读取歌词".to_string()));
    }

    scrape::read_lyrics(&path)
}

/// 按当前 Title / Artist 搜网易云，取第一条的 LRC。
#[tauri::command]
pub async fn sync_track_lyrics(
    state: State<'_, AppState>,
    track_id: i64,
) -> AppResult<String> {
    let (title, artist) = {
        let connection = lock_db(&state)?;
        let (title, artist, _, _) = repo::get_track_meta(&connection, track_id)?
            .ok_or_else(|| AppError::BadInput("曲目不存在".to_string()))?;
        (title, artist)
    };

    let keyword = format!("{} {}", title.trim(), artist.trim());
    let hits = state.netease.search(keyword.trim(), 0, 5).await?;
    let Some(hit) = hits.into_iter().find(|hit| hit.source == SourceId::Netease) else {
        return Ok(String::new());
    };

    state.netease.lyric(&hit.id).await
}

/// 批量补全：Rust 侧顺序执行并逐首发 `metadata_progress` 事件。
#[tauri::command]
pub async fn batch_scrape_metadata(
    app: AppHandle,
    state: State<'_, AppState>,
    track_ids: Vec<i64>,
) -> AppResult<BatchScrapeResult> {
    let total = track_ids.len() as u32;
    let mut applied = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;

    for (index, track_id) in track_ids.into_iter().enumerate() {
        emit_metadata_progress(&app, index as u32 + 1, total);

        let meta = {
            let connection = lock_db(&state)?;
            repo::get_track_meta(&connection, track_id)
        };
        let Some((title, artist, album, _)) = meta? else {
            skipped += 1;
            continue;
        };

        let preview = match scrape_preview_for(&state, &title, &artist, &album).await {
            Ok(preview) => preview,
            Err(_) => {
                failed += 1;
                continue;
            }
        };

        let Some(best) = preview
            .candidates
            .into_iter()
            .next()
            .filter(|candidate| candidate.score >= 60)
        else {
            skipped += 1;
            continue;
        };

        match apply_scrape_candidate(&app, &state, track_id, &best).await {
            Ok(_) => applied += 1,
            Err(_) => failed += 1,
        }
    }

    Ok(BatchScrapeResult {
        applied,
        skipped,
        failed,
    })
}

/// 重命名：改磁盘文件名 + 同步 DB 的 rel_path 与标题。
/// 注意：只改文件名与库内记录，**不重写音频标签**（避免动到用户原始标签）。
#[tauri::command]
pub fn rename_track(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    new_name: String,
) -> AppResult<Track> {
    let connection = lock_db(&state)?;
    let (_, root) = paths::library_root(&app, &connection)?;

    let (_, artist, album, rel_path) = repo::get_track_meta(&connection, id)?
        .ok_or_else(|| AppError::BadInput("曲目不存在".to_string()))?;

    let source = root.join(&rel_path);
    if !source.is_file() {
        return Err(AppError::BadInput("文件已丢失，无法重命名".to_string()));
    }

    // 非法字符直接替换，避免用户输入把路径搞坏
    let cleaned = new_name.trim().replace(['/', '\\'], "_");
    if cleaned.is_empty() {
        return Err(AppError::BadInput("文件名不能为空".to_string()));
    }

    let ext = source
        .extension()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();

    // 用户可能只改了主干，自动补回原扩展名
    let has_ext = std::path::Path::new(&cleaned).extension().is_some();
    let file_name = if has_ext || ext.is_empty() {
        cleaned
    } else {
        format!("{cleaned}.{ext}")
    };

    let parent = source
        .parent()
        .ok_or_else(|| AppError::Internal("取不到父目录".to_string()))?;
    let destination = parent.join(&file_name);

    if destination != source && destination.exists() {
        return Err(AppError::BadInput("同名文件已存在".to_string()));
    }

    std::fs::rename(&source, &destination)?;

    let new_rel = destination
        .strip_prefix(&root)
        .map_err(|_| AppError::Internal("路径越界".to_string()))?
        .to_string_lossy()
        .replace('\\', "/");

    repo::set_track_path(&connection, id, &new_rel)?;

    let stem = std::path::Path::new(&file_name)
        .file_stem()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.clone());
    repo::update_track_meta(&connection, id, &stem, &artist, &album)?;

    let tracks = repo::list_tracks(&connection, 500)?;
    tracks
        .into_iter()
        .find(|track| track.id == id)
        .ok_or_else(|| AppError::Internal("重命名后读取曲目失败".to_string()))
}

// ───────────────────────────── 文件级操作（双窗格用） ─────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDetails {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub format: String,
    pub bitrate: Option<u32>,
    pub duration_ms: Option<u64>,
    /// 位深（bit depth，flac 常见 16/24）
    pub bit_depth: Option<u8>,
    /// 采样率（Hz）
    pub sample_rate: Option<u32>,
    /// 修改时间（unix 秒）
    pub modified: Option<i64>,
    /// 对应 SQLite 曲目 id；非音乐库文件为 None
    pub track_id: Option<i64>,
    /// data URL（内嵌封面），没有则 None
    pub cover: Option<String>,
}

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(triple >> 18) as usize & 63] as char);
        out.push(B64[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[triple as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[tauri::command]
pub fn file_details(
    app: AppHandle,
    state: State<'_, AppState>,
    space: Option<String>,
    rel_path: String,
) -> AppResult<FileDetails> {
    use lofty::file::TaggedFileExt;
    use lofty::prelude::AudioFile;

    let connection = lock_db(&state)?;
    let (_, root) = paths::library_root(&app, &connection)?;
    let track_id = if space.as_deref().unwrap_or("app") == "app" {
        repo::find_track_by_path(&connection, &rel_path)?
    } else {
        None
    };
    drop(connection);

    let base = crate::library::space_base(&root, space.as_deref().unwrap_or("app"));
    let path = base.join(rel_path.trim_start_matches('/'));
    if !path.is_file() {
        return Err(AppError::BadInput("文件不存在".to_string()));
    }

    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let name = path
        .file_name()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();
    let format = path
        .extension()
        .map(|v| v.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    let mut bitrate = None;
    let mut duration_ms = None;
    let mut bit_depth = None;
    let mut sample_rate = None;
    let mut cover = None;

    if let Ok(tagged) = lofty::probe::Probe::open(&path).and_then(|probe| Ok(probe.read()?)) {
        let properties = tagged.properties();
        duration_ms = Some(properties.duration().as_millis() as u64);
        bitrate = properties.audio_bitrate();
        bit_depth = properties.bit_depth();
        sample_rate = properties.sample_rate();

        if let Some(tag) = tagged.primary_tag() {
            if let Some(picture) = tag.pictures().first() {
                let data = picture.data();
                if !data.is_empty() {
                    let mime = picture
                        .mime_type()
                        .map(|m| m.to_string())
                        .unwrap_or_else(|| "image/jpeg".to_string());
                    cover = Some(format!("data:{mime};base64,{}", base64_encode(data)));
                }
            }
        }
    }

    Ok(FileDetails {
        name,
        path: path.to_string_lossy().to_string(),
        size,
        format,
        bitrate,
        duration_ms,
        bit_depth,
        sample_rate,
        track_id,
        modified: std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64),
        cover,
    })
}

/// 删除：一律 fs::rename 到 {library}/.temp/trash/（跨分区自动降级 copy+remove）
#[tauri::command]
pub fn trash_paths(
    app: AppHandle,
    state: State<'_, AppState>,
    space: Option<String>,
    rel_paths: Vec<String>,
) -> AppResult<u32> {
    let connection = lock_db(&state)?;
    let (_, root) = paths::library_root(&app, &connection)?;

    let space = space.as_deref().unwrap_or("app");
    let base = crate::library::space_base(&root, space);
    let trash_dir = root.join(".temp").join("trash");
    std::fs::create_dir_all(&trash_dir)?;

    let mut moved = 0u32;

    for rel in rel_paths {
        let source = base.join(rel.trim_start_matches('/'));
        if !source.is_file() {
            continue;
        }

        let ext = source
            .extension()
            .map(|v| v.to_string_lossy().to_string())
            .unwrap_or_default();
        let trash_name = if ext.is_empty() {
            format!("{}.bin", uuid::Uuid::new_v4())
        } else {
            format!("{}.{}", uuid::Uuid::new_v4(), ext)
        };
        let destination = trash_dir.join(&trash_name);

        crate::library::move_file(&source, &destination)?;

        let size = std::fs::metadata(&destination).map(|m| m.len()).unwrap_or(0);
        if space == "app" {
            if let Ok(Some(track_id)) = repo::find_track_by_path(&connection, &rel) {
                let _ = repo::insert_trash(&connection, 0, &rel, &trash_name, size as i64, crate::util::now_ms());
                let _ = repo::set_track_status(&connection, track_id, "missing");
            }
        }

        moved += 1;
    }

    Ok(moved)
}

/// 按路径重命名（双窗格用，文件可能不在库内）
#[tauri::command]
pub fn rename_path(
    app: AppHandle,
    state: State<'_, AppState>,
    space: Option<String>,
    rel_path: String,
    new_name: String,
) -> AppResult<String> {
    let connection = lock_db(&state)?;
    let (_, root) = paths::library_root(&app, &connection)?;

    let base = crate::library::space_base(&root, space.as_deref().unwrap_or("app"));
    let source = base.join(rel_path.trim_start_matches('/'));
    if !source.is_file() {
        return Err(AppError::BadInput("文件不存在".to_string()));
    }

    let cleaned = new_name.trim().replace(['/', '\\'], "_");
    if cleaned.is_empty() {
        return Err(AppError::BadInput("文件名不能为空".to_string()));
    }

    let ext = source
        .extension()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();
    let has_ext = std::path::Path::new(&cleaned).extension().is_some();
    let file_name = if has_ext || ext.is_empty() {
        cleaned
    } else {
        format!("{cleaned}.{ext}")
    };

    let parent = source
        .parent()
        .ok_or_else(|| AppError::Internal("取不到父目录".to_string()))?;
    let destination = parent.join(&file_name);

    if destination != source && destination.exists() {
        return Err(AppError::BadInput("同名文件已存在".to_string()));
    }

    std::fs::rename(&source, &destination)?;

    if let Ok(Some(track_id)) = repo::find_track_by_path(&connection, &rel_path) {
        if let Ok(new_rel) = destination.strip_prefix(&root) {
            let new_rel = new_rel.to_string_lossy().replace('\\', "/");
            let _ = repo::set_track_path(&connection, track_id, &new_rel);
            return Ok(new_rel);
        }
    }

    Ok(destination
        .strip_prefix(&base)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or(file_name))
}

// ───────────────────────────── 工具 ─────────────────────────────

/// 从 URL 末段猜一个标题：/a/b/周杰伦%20-%20晴天.flac → "周杰伦 - 晴天"
fn filename_from_url(url: &reqwest::Url) -> Option<String> {
    let last = url.path_segments()?.last()?;
    let decoded = percent_decode(last);
    let stem = decoded
        .rsplit_once('.')
        .map(|(left, _)| left)
        .unwrap_or(decoded.as_str());
    let trimmed = stem.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 极简百分号解码：不引 percent-encoding，够用且无依赖
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex_value(bytes[index + 1]), hex_value(bytes[index + 2])) {
                out.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&out).to_string()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
