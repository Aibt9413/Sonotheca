//! 单个下载任务的编排器：状态机 + 进度事件 + 落位 + 入库。
//!
//! 刻意**不持有 Connection** 跨 await —— 需要写库时用 `db::with_conn` 开短连接。
//! 这样 `run()` 的 future 天然是 `Send`，避免 async/rusqlite 的经典编译墙。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::AppHandle;
use tokio::sync::Semaphore;

use crate::db::{self, repo};
use crate::error::{AppError, AppResult};
use crate::events::{self, ProgressPayload};
use crate::model::task::{DownloadTask, ErrorKind, TaskError, TaskState};
use crate::pipeline::commit::{commit_part, CommitOutcome};
use crate::pipeline::download::download_to_part;
use crate::pipeline::magic::{self, Container};
use crate::pipeline::meta::{extension_from_content_type, probe_and_tag};
use crate::pipeline::path::PathCtx;
use crate::util::now_ms;

pub struct RunConfig {
    pub app: AppHandle,
    pub db_path: PathBuf,
    pub root: PathBuf,
    pub root_id: i64,
    pub task: DownloadTask,
    pub cancel: Arc<AtomicBool>,
    pub permits: Arc<Semaphore>,
    /// 全局复用的 HTTP 客户端（连接池跨任务共享）
    pub client: Arc<reqwest::Client>,
    /// 全局限速令牌桶
    pub throttle: Arc<crate::pipeline::throttle::TokenBucket>,
}

pub async fn run(cfg: RunConfig) {
    let RunConfig {
        app,
        db_path,
        root,
        root_id,
        mut task,
        cancel,
        permits,
        client,
        throttle,
    } = cfg;

    // 并发闸门放在最前面；排队期间状态保持 Queued
    let _permit = permits.acquire_owned().await;

    // 续传的关键：临时文件路径必须是从 DB 里带出来的那个。
    // 只有首次入队（temp_path 为空）才按 uuid 新建。
    let part = match task.temp_path.clone() {
        Some(path) => path,
        None => {
            let path = root
                .join(".temp")
                .join("downloads")
                .join(format!("{}.part", task.id));
            task.temp_path = Some(path.clone());
            path
        }
    };

    if cancel.load(Ordering::Relaxed) {
        cleanup_part(&part);
        finish_with_error(&app, &db_path, &mut task, AppError::Canceled);
        return;
    }

    if let Err(err) = transition(&app, &db_path, &mut task, TaskState::Downloading, None) {
        finish_with_error(&app, &db_path, &mut task, err);
        return;
    }

    let client = client;

    // 进度：事件每 250ms 一次（在 download 内部节流），DB 每 1s 落一次
    let progress_app = app.clone();
    let progress_db = db_path.clone();
    let task_id = task.id;
    let mut last_db_write = Instant::now();

    let mut on_progress = move |done: u64, total: Option<u64>, speed: u64| {
        events::emit_progress(
            &progress_app,
            ProgressPayload {
                id: task_id.to_string(),
                bytes_done: done,
                bytes_total: total,
                state: TaskState::Downloading,
                speed_bps: speed,
            },
        );

        if last_db_write.elapsed() >= Duration::from_secs(1) {
            last_db_write = Instant::now();
            let _ = db::with_conn(&progress_db, |conn| {
                repo::update_progress(conn, task_id, done, total, now_ms())
            });
        }
    };

    // 音源解析出的 CDN 直链优先；直链源则回退到 source_track_id 本身
    let source_url = task
        .snapshot
        .download_url
        .clone()
        .unwrap_or_else(|| task.source_track_id.clone());

    let outcome =
        match download_to_part(&client, &source_url, &part, &cancel, &throttle, &mut on_progress)
            .await
        {
            Ok(outcome) => outcome,
            Err(err) => {
                // 网络类失败保留 .part，让「继续」能真正断点续传；
                // 认证/404/取消等情况直接清掉，避免无意义占盘。
                if !should_keep_part(&err) {
                    cleanup_part(&part);
                }
                finish_with_error(&app, &db_path, &mut task, err);
                return;
            }
        };
    drop(on_progress);

    if outcome.resumed_from > 0 || outcome.mode == "chunked" {
        eprintln!(
            "[sonotheca] 任务 {} 完成：mode={} 从第 {} 字节起（共 {} 字节）",
            task.id, outcome.mode, outcome.resumed_from, outcome.bytes
        );
    }

    task.bytes_done = outcome.bytes;
    task.bytes_total = task.bytes_total.or(Some(outcome.bytes));

    if let Err(err) = transition(&app, &db_path, &mut task, TaskState::Processing, None) {
        cleanup_part(&part);
        finish_with_error(&app, &db_path, &mut task, err);
        return;
    }

    // ── 处理阶段：NCM 解密 → 读/写标签 ─────────────────────────────────
    let mut media_path = part.clone();

    if crate::pipeline::ncm::is_ncm(&part) {
        eprintln!("[sonotheca] 检测到 NCM 容器，开始解密");
        let content = match crate::pipeline::ncm::decrypt(&part) {
            Ok(content) => content,
            Err(err) => {
                cleanup_part(&part);
                finish_with_error(&app, &db_path, &mut task, err);
                return;
            }
        };

        let decrypted = part.with_extension("dec");
        if let Err(err) = std::fs::write(&decrypted, &content.audio) {
            cleanup_part(&part);
            finish_with_error(&app, &db_path, &mut task, AppError::Io(err));
            return;
        }
        let _ = std::fs::remove_file(&part);

        // 容器里自带的元数据比 URL 猜的准，优先采用
        if let Some(title) = content.title {
            if !title.trim().is_empty() {
                task.title = title;
            }
        }
        if let Some(artist) = content.artist {
            if !artist.trim().is_empty() {
                task.artist = artist;
            }
        }

        media_path = decrypted;
    }

    // ── Magic Number 纠正：以后缀之外的真相为准 ────────────────────────
    // 网易云偶尔给出后缀与内容不符的地址；在写标签 / 归档之前先按字节纠正，
    // 否则播放器会按错误的后缀选解码器。
    let magic = magic::sniff(&media_path);
    if magic == Container::Ncm {
        cleanup_temp(&part, &media_path);
        finish_with_error(
            &app,
            &db_path,
            &mut task,
            AppError::Decrypt("文件是 NCM 加密容器，但未在解密阶段处理".to_string()),
        );
        return;
    }

    let magic_ext = magic.extension();
    if let Some(ext) = magic_ext {
        if media_path.extension().and_then(|value| value.to_str()) != Some(ext) {
            eprintln!(
                "[sonotheca] 后缀纠正: {:?} -> {}",
                media_path.extension(),
                ext
            );
            let corrected = media_path.with_extension(ext);
            if std::fs::rename(&media_path, &corrected).is_ok() {
                media_path = corrected;
            }
        }
    }

    let probed = probe_and_tag(&media_path, &task.title, &task.artist);

    let (ext, final_title, final_artist, duration_ms) = match probed {
        Ok(meta) => {
            // lofty 看的是内容，优先信它；它给不出类型时再用魔数 / Content-Type 兜底
            let ext = if meta.ext != "bin" {
                meta.ext
            } else if let Some(ext) = magic_ext {
                ext.to_string()
            } else {
                extension_from_content_type(outcome.content_type.as_deref())
                    .map(str::to_string)
                    .unwrap_or_else(|| "bin".to_string())
            };
            (
                ext,
                meta.title.unwrap_or_else(|| task.title.clone()),
                meta.artist.unwrap_or_else(|| task.artist.clone()),
                meta.duration_ms,
            )
        }
        Err(err) => {
            // lofty 认不出来（少见容器 / 非音频）→ 魔数 → Content-Type
            let Some(ext) = magic_ext.map(str::to_string).or_else(|| {
                extension_from_content_type(outcome.content_type.as_deref()).map(str::to_string)
            }) else {
                cleanup_temp(&part, &media_path);
                finish_with_error(&app, &db_path, &mut task, err);
                return;
            };
            (ext, task.title.clone(), task.artist.clone(), None)
        }
    };

    let size_bytes = std::fs::metadata(&media_path)
        .map(|meta| meta.len())
        .unwrap_or(outcome.bytes);

    // ── 落位阶段：分类器 → create_new 占位 → rename ─────────────────────
    let snapshot = task.snapshot.clone();
    let ctx = PathCtx {
        source: task.source.as_str(),
        // {quality} 用接口 level 名（exhigh / lossless …），与网易云参数保持一致
        quality: snapshot.quality.level(),
        artist: &final_artist,
        title: &final_title,
        album: "",
        track_no: None,
        ext: &ext,
    };

    match commit_part(&media_path, &root, &snapshot, &ctx) {
        Ok(CommitOutcome::Moved(destination)) => {
            task.final_path = Some(destination.clone());
            record_track(
                &db_path,
                root_id,
                &root,
                &destination,
                &final_title,
                &final_artist,
                duration_ms,
                size_bytes,
                &ext,
            );
            let _ = transition(&app, &db_path, &mut task, TaskState::Completed, None);
            emit_finished(&app, &task);
        }
        Ok(CommitOutcome::Skipped) => {
            // 防重名策略为 Skip 且目标已存在：part 已在 commit 内删除
            let _ = transition(&app, &db_path, &mut task, TaskState::Completed, None);
            emit_finished(&app, &task);
        }
        Err(err) => {
            cleanup_temp(&part, &media_path);
            finish_with_error(&app, &db_path, &mut task, err);
        }
    }
}

fn transition(
    app: &AppHandle,
    db_path: &Path,
    task: &mut DownloadTask,
    state: TaskState,
    error: Option<TaskError>,
) -> AppResult<()> {
    task.state = state;
    task.error = error;
    task.updated_at = now_ms();

    let id = task.id;
    let error_ref = task.error.clone();
    let final_path = task.final_path.clone();
    let bytes_done = task.bytes_done;
    let bytes_total = task.bytes_total;
    let now = task.updated_at;

    db::with_conn(db_path, move |conn| {
        repo::update_state(conn, id, state, error_ref.as_ref(), final_path.as_deref(), now)?;
        repo::update_progress(conn, id, bytes_done, bytes_total, now)
    })?;

    events::emit_state(app, task);
    Ok(())
}

fn finish_with_error(app: &AppHandle, db_path: &Path, task: &mut DownloadTask, err: AppError) {
    let state = if matches!(&err, AppError::Canceled) {
        TaskState::Canceled
    } else {
        TaskState::Failed
    };

    let task_error = TaskError {
        kind: err.error_kind(),
        message: err.to_string(),
        http_status: err.http_status(),
    };

    let _ = transition(app, db_path, task, state, Some(task_error));
    emit_finished(app, task);
}

fn emit_finished(app: &AppHandle, task: &DownloadTask) {
    events::emit_progress(
        app,
        ProgressPayload {
            id: task.id.to_string(),
            bytes_done: task.bytes_done,
            bytes_total: task.bytes_total,
            state: task.state,
            speed_bps: 0,
        },
    );
}

fn cleanup_part(part: &Path) {
    let _ = std::fs::remove_file(part);
    // sidecar 一起清，避免留下"有元数据但没有数据体"的孤儿
    let mut meta = part.as_os_str().to_owned();
    meta.push(".meta");
    let _ = std::fs::remove_file(PathBuf::from(meta));
}

/// 处理阶段可能把 `.part` 改名成真实后缀（魔数纠正）或派生出 `.dec`。
/// 失败时两者都要清，别在 `.temp/downloads` 里留孤儿。
fn cleanup_temp(part: &Path, media: &Path) {
    cleanup_part(part);
    if media != part {
        let _ = std::fs::remove_file(media);
    }
}

/// 只有"再试一次有可能成功"的失败才保留残片。
///
/// 取消也要保留：取消的语义是**暂停**，用户随后点「继续」应当续传，
/// 而不是从 0 重下。真正丢弃残片的入口是删除任务（后续再补）。
fn should_keep_part(err: &AppError) -> bool {
    matches!(
        err.error_kind(),
        ErrorKind::Network | ErrorKind::Timeout | ErrorKind::RateLimited | ErrorKind::Canceled
    )
}

#[allow(clippy::too_many_arguments)]
fn record_track(
    db_path: &Path,
    root_id: i64,
    root: &Path,
    path: &Path,
    title: &str,
    artist: &str,
    duration_ms: Option<u64>,
    size_bytes: u64,
    ext: &str,
) {
    let Ok(relative) = path.strip_prefix(root) else {
        return;
    };
    let rel_path = relative.to_string_lossy().replace('\\', "/");

    // 码率用「文件大小 / 时长」推算，避免依赖 lofty 各版本不一致的 bitrate API
    let bitrate = match duration_ms {
        Some(ms) if ms > 0 => {
            let bps = (size_bytes as u128 * 8 * 1000) / ms as u128;
            if bps > 0 {
                Some(bps as i64)
            } else {
                None
            }
        }
        _ => None,
    };

    let now = now_ms();
    let _ = db::with_conn(db_path, |conn| {
        repo::upsert_track(
            conn,
            &repo::NewTrack {
                root_id,
                rel_path: &rel_path,
                title,
                artist,
                album: "",
                duration_ms: duration_ms.map(|v| v as i64),
                bitrate,
                size_bytes: Some(size_bytes as i64),
                format: Some(ext),
            },
            now,
        )
    });
}
