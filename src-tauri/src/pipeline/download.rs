//! HTTP 下载内核（M3：分块并发 + Range 续传 + If-Range 校验）。
//!
//! 三种模式，按服务器能力自动选择：
//!   1. **分块并发**：探测到 `Accept-Ranges` 且文件 > 2MB 时，切 8 块、4 路并发。
//!      每块独占自己的区间，用独立文件句柄 `seek` 到偏移写入 —— 不同句柄各自维护
//!      文件位置，因此并发写同一文件不会互相踩踏（这也是不用共享句柄的原因）。
//!   2. **单流**：不支持 Range 或小文件，退化为普通流式。
//!   3. **续传**：`.part.meta` 记录每块是否完成；重启后只补没完成的那几块。
//!
//! 安全底线：`If-Range` 校验器（强 ETag 或 Last-Modified）不匹配时，服务器回 200，
//! 我们立刻清空重下，绝不把新旧两版拼在一起。

use std::collections::VecDeque;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{self, HeaderMap};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::error::{AppError, AppResult};
use crate::model::task::ErrorKind;
use crate::pipeline::throttle::TokenBucket;
use crate::util::now_ms;

/// 浏览器 UA：很多源站会对非浏览器 UA 限速甚至直接 403
pub const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Sonotheca/0.1";

const MAX_ATTEMPTS: u32 = 4;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// 大于这个体积才值得分块
const MIN_CHUNKED_SIZE: u64 = 2 * 1024 * 1024;
/// 并发块数
const CHUNK_CONCURRENCY: usize = 4;
/// 目标块数（实际块大小 = total / MAX_CHUNKS，下限 256KB）
const MAX_CHUNKS: usize = 8;
const MIN_CHUNK_SIZE: u64 = 256 * 1024;

#[derive(Debug)]
pub struct DownloadOutcome {
    pub bytes: u64,
    pub content_type: Option<String>,
    /// 本次是从第几个字节开始写的（> 0 说明命中续传）
    pub resumed_from: u64,
    /// "chunked" | "single"
    pub mode: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct ChunkState {
    start: u64,
    end: u64,
    done: bool,
}

impl ChunkState {
    fn len(&self) -> u64 {
        self.end - self.start + 1
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResumeMeta {
    url: String,
    etag: Option<String>,
    last_modified: Option<String>,
    total: Option<u64>,
    updated_at: i64,
    #[serde(default)]
    chunks: Vec<ChunkState>,
}

impl ResumeMeta {
    fn load(path: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn save(&self, path: &Path) -> AppResult<()> {
        let json = serde_json::to_string(self)
            .map_err(|err| AppError::Internal(format!("续传元数据序列化失败: {err}")))?;
        let tmp = with_suffix(path, ".tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// `If-Range` 只认强校验器；弱 ETag 会被服务器忽略
    fn validator(&self) -> Option<String> {
        if let Some(etag) = &self.etag {
            if !etag.trim_start().starts_with("W/") {
                return Some(etag.clone());
            }
        }
        self.last_modified.clone()
    }
}

/// 服务器能力探测结果
struct Probe {
    total: Option<u64>,
    ranges: bool,
    etag: Option<String>,
    last_modified: Option<String>,
    content_type: Option<String>,
}

impl Probe {
    fn validator(&self) -> Option<String> {
        pick_validator(self.etag.as_deref(), self.last_modified.as_deref())
    }
}

pub async fn download_to_part(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    cancel: &AtomicBool,
    throttle: &TokenBucket,
    on_progress: &mut (dyn FnMut(u64, Option<u64>, u64) + Send),
) -> AppResult<DownloadOutcome> {
    let mut attempt: u32 = 0;

    loop {
        match attempt_download(client, url, part, cancel, throttle, on_progress).await {
            Ok(outcome) => {
                let _ = std::fs::remove_file(sidecar_path(part));
                return Ok(outcome);
            }
            Err(err) => {
                attempt += 1;
                if attempt >= MAX_ATTEMPTS || !is_retryable(&err) {
                    return Err(err);
                }
                let jitter = (now_ms().unsigned_abs() % 250) as u64;
                let backoff = Duration::from_millis(400 * 2u64.pow(attempt) + jitter);
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

fn is_retryable(err: &AppError) -> bool {
    matches!(
        err.error_kind(),
        ErrorKind::Network | ErrorKind::Timeout | ErrorKind::RateLimited
    )
}

async fn attempt_download(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    cancel: &AtomicBool,
    throttle: &TokenBucket,
    on_progress: &mut (dyn FnMut(u64, Option<u64>, u64) + Send),
) -> AppResult<DownloadOutcome> {
    let sidecar = sidecar_path(part);

    // 必须在写 sidecar 之前建目录，否则首次下载会 NotFound
    if let Some(parent) = part.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let previous = ResumeMeta::load(&sidecar).filter(|meta| meta.url == url);
    let previous_validator = previous.as_ref().and_then(ResumeMeta::validator);

    let probe = probe_server(client, url, previous_validator.as_deref()).await?;
    let total = probe.total;
    let chunked = probe.ranges && total.map(|t| t > MIN_CHUNKED_SIZE).unwrap_or(false);

    // 源变了吗？校验器不同就丢弃本地残片
    let same_source = previous
        .as_ref()
        .map(|meta| meta.validator().is_some() && meta.validator() == probe.validator())
        .unwrap_or(false);

    if chunked {
        let total = total.expect("chunked 模式下 total 必存在");
        download_chunked(
            client,
            url,
            part,
            &sidecar,
            total,
            probe,
            previous,
            same_source,
            cancel,
            throttle,
            on_progress,
        )
        .await
    } else {
        download_single(
            client,
            url,
            part,
            &sidecar,
            probe,
            previous_validator,
            same_source,
            cancel,
            throttle,
            on_progress,
        )
        .await
    }
}

/// 只取 1 个字节来问服务器"支不支持 Range / 总长多少 / 校验器是什么"
async fn probe_server(
    client: &reqwest::Client,
    url: &str,
    validator: Option<&str>,
) -> AppResult<Probe> {
    let mut request = client.get(url).header(header::RANGE, "bytes=0-0");
    if let Some(validator) = validator {
        request = request.header(header::IF_RANGE, validator);
    }

    let response = request.send().await.map_err(map_reqwest)?;
    let status = response.status();

    let etag = header_text(response.headers(), header::ETAG);
    let last_modified = header_text(response.headers(), header::LAST_MODIFIED);
    let content_type = header_text(response.headers(), header::CONTENT_TYPE).map(|value| {
        value
            .split(';')
            .next()
            .unwrap_or(&value)
            .trim()
            .to_ascii_lowercase()
    });

    match status {
        StatusCode::PARTIAL_CONTENT => {
            let content_range = header_text(response.headers(), header::CONTENT_RANGE)
                .ok_or_else(|| AppError::Network("206 缺少 Content-Range".to_string()))?;
            let (_, total) = parse_content_range(&content_range)
                .ok_or_else(|| AppError::Network(format!("无法解析 Content-Range: {content_range}")))?;
            Ok(Probe {
                total,
                ranges: true,
                etag,
                last_modified,
                content_type,
            })
        }
        StatusCode::OK => Ok(Probe {
            total: response.content_length(),
            ranges: false,
            etag,
            last_modified,
            content_type,
        }),
        other => Err(http_error(other)),
    }
}

// ───────────────────────────── 分块并发 ─────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn download_chunked(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    sidecar: &Path,
    total: u64,
    probe: Probe,
    previous: Option<ResumeMeta>,
    same_source: bool,
    cancel: &AtomicBool,
    throttle: &TokenBucket,
    on_progress: &mut (dyn FnMut(u64, Option<u64>, u64) + Send),
) -> AppResult<DownloadOutcome> {
    // 可复用旧的分块进度：同一源、总长一致、且旧 meta 里确实有分块信息
    let reusable = same_source
        && previous
            .as_ref()
            .map(|meta| meta.total == Some(total) && !meta.chunks.is_empty())
            .unwrap_or(false);

    let chunks = if reusable {
        previous.as_ref().unwrap().chunks.clone()
    } else {
        plan_chunks(total)
    };

    let already_done: u64 = chunks.iter().filter(|c| c.done).map(ChunkState::len).sum();

    let meta = ResumeMeta {
        url: url.to_string(),
        etag: probe.etag.clone(),
        last_modified: probe.last_modified.clone(),
        total: Some(total),
        updated_at: now_ms(),
        chunks: chunks.clone(),
    };
    meta.save(sidecar)?;

    // 预分配：让每个 worker 都能直接 seek 到自己的偏移写入
    {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(part)
            .await?;
        file.set_len(total).await?;
        file.sync_all().await?;
    }

    let pending: Vec<ChunkState> = chunks.iter().filter(|c| !c.done).copied().collect();
    if pending.is_empty() {
        return Ok(DownloadOutcome {
            bytes: total,
            content_type: probe.content_type,
            resumed_from: total,
            mode: "chunked",
        });
    }

    let done_bytes = Arc::new(AtomicU64::new(already_done));
    let meta_shared = Arc::new(Mutex::new(meta));
    let validator = probe.validator();

    // 按轮转把块分配给 worker，避免共享队列和锁竞争
    let mut per_worker: Vec<Vec<ChunkState>> = vec![Vec::new(); CHUNK_CONCURRENCY];
    for (index, chunk) in pending.iter().enumerate() {
        per_worker[index % CHUNK_CONCURRENCY].push(*chunk);
    }

    let mut workers: Vec<
        std::pin::Pin<Box<dyn std::future::Future<Output = AppResult<()>> + Send + '_>>,
    > = Vec::new();

    for work in per_worker.into_iter() {
        if work.is_empty() {
            continue;
        }
        let part = part.to_path_buf();
        let url = url.to_string();
        let sidecar = sidecar.to_path_buf();
        let validator = validator.clone();
        let done = Arc::clone(&done_bytes);
        let meta = Arc::clone(&meta_shared);
        workers.push(Box::pin(async move {
            let mut file = tokio::fs::OpenOptions::new().write(true).open(&part).await?;
            let mut queue: VecDeque<ChunkState> = work.into_iter().collect();

            while let Some(chunk) = queue.pop_front() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(AppError::Canceled);
                }
                fetch_chunk(
                    client,
                    &url,
                    &mut file,
                    &chunk,
                    validator.as_deref(),
                    cancel,
                    throttle,
                    &done,
                )
                .await?;

                // 记账并落盘，保证崩溃后能精确续传
                {
                    let mut guard = meta
                        .lock()
                        .map_err(|_| AppError::Internal("分块状态锁被毒化".to_string()))?;
                    if let Some(slot) = guard.chunks.iter_mut().find(|c| c.start == chunk.start) {
                        slot.done = true;
                    }
                    guard.updated_at = now_ms();
                    guard.save(&sidecar)?;
                }
            }
            Ok(())
        }));
    }

    // 进度心跳：worker 只更新原子量，这里统一节流回调
    let all = futures_util::future::join_all(workers);
    tokio::pin!(all);
    let mut ticker = tokio::time::interval(PROGRESS_INTERVAL);
    let mut last_tick = Instant::now();
    let mut last_bytes = already_done;

    loop {
        tokio::select! {
            results = &mut all => {
                for result in results {
                    result?;
                }
                break;
            }
            _ = ticker.tick() => {
                let now = done_bytes.load(Ordering::Relaxed);
                let seconds = last_tick.elapsed().as_secs_f64().max(0.001);
                let speed = ((now.saturating_sub(last_bytes)) as f64 / seconds) as u64;
                last_tick = Instant::now();
                last_bytes = now;
                on_progress(now, Some(total), speed);
            }
        }
    }

    // 收尾：把每个句柄的写入刷到磁盘
    {
        let file = tokio::fs::OpenOptions::new().write(true).open(part).await?;
        file.sync_all().await?;
    }

    let written = file_len(part);
    if written != total {
        return Err(AppError::Network(format!(
            "分块下载后大小不符：期望 {total}，实际 {written}"
        )));
    }

    on_progress(total, Some(total), 0);

    Ok(DownloadOutcome {
        bytes: total,
        content_type: probe.content_type,
        resumed_from: already_done,
        mode: "chunked",
    })
}

async fn fetch_chunk(
    client: &reqwest::Client,
    url: &str,
    file: &mut tokio::fs::File,
    chunk: &ChunkState,
    validator: Option<&str>,
    cancel: &AtomicBool,
    throttle: &TokenBucket,
    done: &AtomicU64,
) -> AppResult<()> {
    let expected = chunk.len();
    let mut attempt: u32 = 0;

    loop {
        match fetch_chunk_once(client, url, file, chunk, validator, cancel, throttle, done).await {
            Ok(written) => {
                if written != expected {
                    // 少写了：把进度补回去再重试
                    done.fetch_sub(written, Ordering::Relaxed);
                    return Err(AppError::Network(format!(
                        "分块 {}-{} 只收到 {written}/{expected} 字节",
                        chunk.start, chunk.end
                    )));
                }
                return Ok(());
            }
            Err((err, written)) => {
                done.fetch_sub(written, Ordering::Relaxed);
                attempt += 1;
                if attempt >= MAX_ATTEMPTS || !is_retryable(&err) {
                    return Err(err);
                }
                let backoff = Duration::from_millis(300 * 2u64.pow(attempt));
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

async fn fetch_chunk_once(
    client: &reqwest::Client,
    url: &str,
    file: &mut tokio::fs::File,
    chunk: &ChunkState,
    validator: Option<&str>,
    cancel: &AtomicBool,
    throttle: &TokenBucket,
    done: &AtomicU64,
) -> Result<u64, (AppError, u64)> {
    let mut request = client.get(url).header(
        header::RANGE,
        format!("bytes={}-{}", chunk.start, chunk.end),
    );
    if let Some(validator) = validator {
        request = request.header(header::IF_RANGE, validator);
    }

    let response = request.send().await.map_err(|err| (map_reqwest(err), 0))?;

    match response.status() {
        StatusCode::PARTIAL_CONTENT => {}
        StatusCode::OK => {
            // 服务器忽略了 Range（源变了 / 不支持）→ 不能继续拼接
            return Err((
                AppError::Network("分块请求被回退为 200，源可能已变更".to_string()),
                0,
            ));
        }
        other => return Err((http_error(other), 0)),
    }

    file.seek(SeekFrom::Start(chunk.start))
        .await
        .map_err(|err| (AppError::Io(err), 0))?;

    let mut written: u64 = 0;
    let mut stream = response.bytes_stream();

    while let Some(item) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            return Err((AppError::Canceled, written));
        }
        let bytes = item.map_err(|err| (map_reqwest(err), written))?;
        throttle.acquire(bytes.len() as u64).await;
        file.write_all(&bytes)
            .await
            .map_err(|err| (AppError::Io(err), written))?;
        written += bytes.len() as u64;
        done.fetch_add(bytes.len() as u64, Ordering::Relaxed);
    }

    Ok(written)
}

fn plan_chunks(total: u64) -> Vec<ChunkState> {
    let chunk_size = (total / MAX_CHUNKS as u64).max(MIN_CHUNK_SIZE);
    let mut chunks = Vec::new();
    let mut start = 0u64;
    while start < total {
        let end = (start + chunk_size - 1).min(total - 1);
        chunks.push(ChunkState {
            start,
            end,
            done: false,
        });
        start = end + 1;
    }
    chunks
}

// ───────────────────────────── 单流下载 ─────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn download_single(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    sidecar: &Path,
    probe: Probe,
    previous_validator: Option<String>,
    same_source: bool,
    cancel: &AtomicBool,
    throttle: &TokenBucket,
    on_progress: &mut (dyn FnMut(u64, Option<u64>, u64) + Send),
) -> AppResult<DownloadOutcome> {
    // 只有"同源且有强校验器"才允许续传
    let mut offset = file_len(part);
    if offset > 0 && (!same_source || previous_validator.is_none()) {
        offset = 0;
    }

    let mut request = client.get(url);
    if offset > 0 {
        request = request.header(header::RANGE, format!("bytes={offset}-"));
        if let Some(validator) = &previous_validator {
            request = request.header(header::IF_RANGE, validator.clone());
        }
    }

    let response = request.send().await.map_err(map_reqwest)?;
    let status = response.status();

    let (offset, total) = match status {
        StatusCode::PARTIAL_CONTENT => {
            let content_range = header_text(response.headers(), header::CONTENT_RANGE)
                .ok_or_else(|| AppError::Network("206 响应缺少 Content-Range".to_string()))?;
            let (start, total) = parse_content_range(&content_range)
                .ok_or_else(|| AppError::Network(format!("无法解析 Content-Range: {content_range}")))?;
            if start != offset {
                return Err(AppError::Network(format!(
                    "续传起点不一致：服务器 {start}，本地 {offset}"
                )));
            }
            (offset, total)
        }
        StatusCode::OK => (0, response.content_length()),
        StatusCode::RANGE_NOT_SATISFIABLE => {
            let remote_total = header_text(response.headers(), header::CONTENT_RANGE)
                .and_then(|value| value.rsplit('/').next().map(str::to_string))
                .and_then(|value| value.trim().parse::<u64>().ok());
            if offset > 0 && remote_total == Some(offset) {
                return Ok(DownloadOutcome {
                    bytes: offset,
                    content_type: probe.content_type,
                    resumed_from: offset,
                    mode: "single",
                });
            }
            return Err(AppError::Network(
                "服务器拒绝了 Range 请求，将从头重试".to_string(),
            ));
        }
        other if other.is_success() => (0, response.content_length()),
        other => return Err(http_error(other)),
    };

    let meta = ResumeMeta {
        url: url.to_string(),
        etag: header_text(response.headers(), header::ETAG),
        last_modified: header_text(response.headers(), header::LAST_MODIFIED),
        total,
        updated_at: now_ms(),
        chunks: Vec::new(),
    };
    meta.save(sidecar)?;

    let content_type = header_text(response.headers(), header::CONTENT_TYPE).map(|value| {
        value
            .split(';')
            .next()
            .unwrap_or(&value)
            .trim()
            .to_ascii_lowercase()
    });

    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(part)
        .await?;
    file.set_len(offset).await?;
    file.seek(SeekFrom::Start(offset)).await?;

    let resumed_from = offset;
    let mut done = offset;
    let mut last_tick = Instant::now();
    let mut last_bytes = done;
    on_progress(done, total, 0);

    let mut stream = response.bytes_stream();
    while let Some(item) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            let _ = file.flush().await;
            return Err(AppError::Canceled);
        }
        let bytes = item.map_err(map_reqwest)?;
        throttle.acquire(bytes.len() as u64).await;
        file.write_all(&bytes).await?;
        done += bytes.len() as u64;

        let elapsed = last_tick.elapsed();
        if elapsed >= PROGRESS_INTERVAL {
            let seconds = elapsed.as_secs_f64().max(0.001);
            let speed = ((done.saturating_sub(last_bytes)) as f64 / seconds) as u64;
            last_tick = Instant::now();
            last_bytes = done;
            on_progress(done, total, speed);
        }
    }

    file.flush().await?;
    file.sync_all().await?;
    on_progress(done, total, 0);

    if let Some(expected) = total {
        if done != expected {
            return Err(AppError::Network(format!(
                "长度不符：期望 {expected} 字节，实际 {done} 字节"
            )));
        }
    }

    Ok(DownloadOutcome {
        bytes: done,
        content_type,
        resumed_from,
        mode: "single",
    })
}

// ───────────────────────────── 工具 ─────────────────────────────

fn pick_validator(etag: Option<&str>, last_modified: Option<&str>) -> Option<String> {
    if let Some(etag) = etag {
        if !etag.trim_start().starts_with("W/") {
            return Some(etag.to_string());
        }
    }
    last_modified.map(|value| value.to_string())
}

fn parse_content_range(value: &str) -> Option<(u64, Option<u64>)> {
    let rest = value.trim().strip_prefix("bytes")?.trim();
    let (range, total) = rest.split_once('/')?;
    let start = range.split('-').next()?.trim().parse::<u64>().ok()?;
    let total = match total.trim() {
        "*" => None,
        other => other.parse::<u64>().ok(),
    };
    Some((start, total))
}

fn header_text(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn sidecar_path(part: &Path) -> PathBuf {
    with_suffix(part, ".meta")
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut raw = path.as_os_str().to_owned();
    raw.push(suffix);
    PathBuf::from(raw)
}

fn http_error(status: StatusCode) -> AppError {
    AppError::Http {
        status: status.as_u16(),
        message: status.canonical_reason().unwrap_or("请求失败").to_string(),
    }
}

fn map_reqwest(err: reqwest::Error) -> AppError {
    if err.is_timeout() {
        AppError::Timeout(err.to_string())
    } else {
        AppError::Network(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_parsing() {
        assert_eq!(parse_content_range("bytes 0-1023/4096"), Some((0, Some(4096))));
        assert_eq!(parse_content_range("bytes 2048-4095/4096"), Some((2048, Some(4096))));
        assert_eq!(parse_content_range("bytes 100-199/*"), Some((100, None)));
        assert_eq!(parse_content_range("garbage"), None);
    }

    #[test]
    fn weak_etag_rejected() {
        assert_eq!(
            pick_validator(Some("W/\"weak\""), Some("lm")).as_deref(),
            Some("lm")
        );
        assert_eq!(pick_validator(Some("W/\"weak\""), None), None);
        assert_eq!(pick_validator(Some("\"strong\""), None).as_deref(), Some("\"strong\""));
    }

    #[test]
    fn chunk_plan_covers_whole_file() {
        for total in [1u64, 1024, 2 * 1024 * 1024, 10 * 1024 * 1024 + 7] {
            let chunks = plan_chunks(total);
            assert_eq!(chunks.first().unwrap().start, 0);
            assert_eq!(chunks.last().unwrap().end, total - 1);
            for pair in chunks.windows(2) {
                assert_eq!(pair[0].end + 1, pair[1].start, "分块之间不能有缝隙");
            }
            let sum: u64 = chunks.iter().map(ChunkState::len).sum();
            assert_eq!(sum, total);
        }
    }

    #[test]
    fn chunk_count_is_bounded() {
        let chunks = plan_chunks(100 * 1024 * 1024);
        assert!(chunks.len() <= MAX_CHUNKS, "块数应受 MAX_CHUNKS 约束");
    }
}
