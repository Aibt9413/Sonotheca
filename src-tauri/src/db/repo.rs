//! 任务与曲目的读写。所有函数都是同步短操作，不做重活。

use std::path::PathBuf;

use rusqlite::{params, Connection, Row};

use crate::error::{AppError, AppResult};
use crate::model::source::SourceId;
use crate::model::task::{
    DownloadTask, ErrorKind, TaskError, TaskSnapshot, TaskState,
};
use crate::model::track::{PathStatus, Track};

fn parse_state(value: &str) -> TaskState {
    match value {
        "queued" => TaskState::Queued,
        "preflight" => TaskState::Preflight,
        "resolving" => TaskState::Resolving,
        "downloading" => TaskState::Downloading,
        "processing" => TaskState::Processing,
        "verifying" => TaskState::Verifying,
        "completed" => TaskState::Completed,
        "failed" => TaskState::Failed,
        "canceled" => TaskState::Canceled,
        _ => TaskState::Failed,
    }
}

fn parse_path_status(value: &str) -> PathStatus {
    match value {
        "missing" => PathStatus::Missing,
        "moved" => PathStatus::Moved,
        _ => PathStatus::Ok,
    }
}

const TASK_COLUMNS: &str = "id, source, source_track_id, title, artist, state, error_kind, \
     error_msg, http_status, attempts, bytes_done, bytes_total, snapshot_json, \
     temp_path, final_path, created_at, updated_at";

fn row_to_task(row: &Row<'_>) -> rusqlite::Result<DownloadTask> {
    let snapshot_json: String = row.get(12)?;
    let snapshot: TaskSnapshot = serde_json::from_str(&snapshot_json).unwrap_or(TaskSnapshot {
        root_id: 0,
        dir_template: String::new(),
        file_template: String::new(),
        relative_dir: String::new(),
        collision: Default::default(),
        quality: Default::default(),
        cover_embed: false,
        playlist_id: None,
        download_url: None,
    });

    let error_kind: Option<String> = row.get(6)?;
    let error_msg: Option<String> = row.get(7)?;
    let http_status: Option<i64> = row.get(8)?;

    let error = match (error_kind, error_msg) {
        (Some(kind), Some(message)) => Some(TaskError {
            kind: ErrorKind::parse(&kind),
            message,
            http_status: http_status.map(|v| v as u16),
        }),
        _ => None,
    };

    let source_str: String = row.get(1)?;

    Ok(DownloadTask {
        id: row
            .get::<_, String>(0)?
            .parse()
            .unwrap_or_else(|_| uuid::Uuid::nil()),
        source: source_str.parse().unwrap_or(SourceId::Direct),
        source_track_id: row.get(2)?,
        title: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        artist: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        state: parse_state(&row.get::<_, String>(5)?),
        snapshot,
        bytes_done: row.get::<_, i64>(10)?.max(0) as u64,
        bytes_total: row.get::<_, Option<i64>>(11)?.map(|v| v.max(0) as u64),
        attempts: row.get::<_, i64>(9)?.max(0) as u32,
        error,
        temp_path: row.get::<_, Option<String>>(13)?.map(PathBuf::from),
        final_path: row.get::<_, Option<String>>(14)?.map(PathBuf::from),
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
    })
}

pub fn insert_task(conn: &Connection, task: &DownloadTask) -> AppResult<()> {
    let snapshot = serde_json::to_string(&task.snapshot)
        .map_err(|e| AppError::Internal(format!("快照序列化失败: {e}")))?;

    conn.execute(
        "INSERT INTO download_tasks
           (id, source, source_track_id, title, artist, state,
            bytes_done, bytes_total, snapshot_json, temp_path, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            task.id.to_string(),
            task.source.as_str(),
            &task.source_track_id,
            &task.title,
            &task.artist,
            task.state.as_str(),
            task.bytes_done as i64,
            task.bytes_total.map(|v| v as i64),
            snapshot,
            task.temp_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            task.created_at,
            task.updated_at,
        ],
    )?;
    Ok(())
}

pub fn update_progress(
    conn: &Connection,
    id: uuid::Uuid,
    bytes_done: u64,
    bytes_total: Option<u64>,
    now: i64,
) -> AppResult<()> {
    conn.execute(
        "UPDATE download_tasks SET bytes_done = ?1, bytes_total = ?2, updated_at = ?3 WHERE id = ?4",
        params![
            bytes_done as i64,
            bytes_total.map(|v| v as i64),
            now,
            id.to_string()
        ],
    )?;
    Ok(())
}

pub fn update_state(
    conn: &Connection,
    id: uuid::Uuid,
    state: TaskState,
    error: Option<&TaskError>,
    final_path: Option<&std::path::Path>,
    now: i64,
) -> AppResult<()> {
    let (kind, message, status) = match error {
        Some(e) => (
            Some(e.kind.as_str()),
            Some(e.message.clone()),
            e.http_status.map(|v| v as i64),
        ),
        None => (None, None, None),
    };

    conn.execute(
        "UPDATE download_tasks
            SET state = ?1, error_kind = ?2, error_msg = ?3, http_status = ?4,
                final_path = COALESCE(?5, final_path), updated_at = ?6,
                finished_at = CASE WHEN ?1 IN ('completed','failed','canceled')
                                   THEN ?6 ELSE finished_at END
          WHERE id = ?7",
        params![
            state.as_str(),
            kind,
            message,
            status,
            final_path.map(|p| p.to_string_lossy().to_string()),
            now,
            id.to_string(),
        ],
    )?;
    Ok(())
}

pub fn list_tasks(conn: &Connection, limit: u32) -> AppResult<Vec<DownloadTask>> {
    let sql = format!(
        "SELECT {TASK_COLUMNS} FROM download_tasks ORDER BY created_at DESC LIMIT ?1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![limit as i64], |row| row_to_task(row))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub struct NewTrack<'a> {
    pub root_id: i64,
    pub rel_path: &'a str,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub duration_ms: Option<i64>,
    pub bitrate: Option<i64>,
    pub size_bytes: Option<i64>,
    pub format: Option<&'a str>,
}

/// 以 (root_id, rel_path) 为唯一键 upsert，返回行 id
pub fn upsert_track(conn: &Connection, track: &NewTrack<'_>, now: i64) -> AppResult<i64> {
    let id = conn.query_row(
        "INSERT INTO tracks
           (root_id, rel_path, title, artist, album, duration_ms, bitrate, size_bytes,
            format, path_status, added_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'ok',?10,?10)
         ON CONFLICT(root_id, rel_path) DO UPDATE SET
           title = excluded.title, artist = excluded.artist, album = excluded.album,
           duration_ms = excluded.duration_ms, bitrate = excluded.bitrate,
           size_bytes = excluded.size_bytes, format = excluded.format,
           path_status = 'ok', updated_at = excluded.updated_at
         RETURNING id",
        params![
            track.root_id,
            track.rel_path,
            track.title,
            track.artist,
            track.album,
            track.duration_ms,
            track.bitrate,
            track.size_bytes,
            track.format,
            now,
        ],
        |row| row.get(0),
    )?;
    Ok(id)
}

pub fn list_tracks(conn: &Connection, limit: u32) -> AppResult<Vec<Track>> {
    list_tracks_page(conn, limit, 0)
}

/// 分页读取可见曲目；`offset` 由前端按已加载数量传入。
pub fn list_tracks_page(
    conn: &Connection,
    limit: u32,
    offset: u32,
) -> AppResult<Vec<Track>> {
    // 只列出还在库里的文件：移入回收站后 path_status 会变 'missing'，
    // 必须在这里过滤，否则前端刷新一次又会把它画回来
    let mut stmt = conn.prepare(
        "SELECT id, title, artist, album, duration_ms, bitrate, size_bytes, format,
                rel_path, path_status
           FROM tracks
          WHERE path_status <> 'missing'
          ORDER BY added_at DESC
          LIMIT ?1 OFFSET ?2",
    )?;

    let rows = stmt.query_map(params![limit as i64, offset as i64], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, Option<i64>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (id, title, artist, album, duration_ms, bitrate, size_bytes, format, rel_path, status) =
            row?;
        out.push(Track {
            id,
            title,
            artist,
            album,
            duration_ms,
            bitrate,
            size_bytes,
            format,
            rel_path,
            path_status: parse_path_status(&status),
            // 跨源可用性 M2 才填
            sources: Vec::new(),
        });
    }
    Ok(out)
}

pub fn count_tracks(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?)
}

pub fn count_visible_tracks(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE path_status <> 'missing'",
        [],
        |row| row.get(0),
    )?)
}

// ───────────────────────────── settings ─────────────────────────────

pub fn get_setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    let mut stmt = conn.prepare("SELECT value_json FROM settings WHERE key = ?1")?;
    let mut rows = stmt.query([key])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO settings (key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![key, value],
    )?;
    Ok(())
}

pub fn delete_setting(conn: &Connection, key: &str) -> AppResult<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
    Ok(())
}

// ───────────────────────────── 最近歌单（M8） ─────────────────────────────

pub fn upsert_recent_playlist(
    conn: &Connection,
    source: &str,
    id: &str,
    name: &str,
    cover_url: Option<&str>,
    total: i64,
    now: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO recent_playlists (source, id, name, cover_url, total, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(source, id) DO UPDATE SET
           name = excluded.name,
           cover_url = excluded.cover_url,
           total = excluded.total,
           updated_at = excluded.updated_at",
        params![source, id, name, cover_url, total, now],
    )?;
    Ok(())
}

/// (source, id, name, cover_url, total)
pub fn list_recent_playlists(
    conn: &Connection,
    limit: u32,
) -> AppResult<Vec<(String, String, String, Option<String>, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT source, id, name, cover_url, total
           FROM recent_playlists
          ORDER BY updated_at DESC
          LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

// ───────────────────────────── 文件管理 ─────────────────────────────

pub fn find_track_by_path(conn: &Connection, rel_path: &str) -> AppResult<Option<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM tracks WHERE rel_path = ?1 LIMIT 1")?;
    let mut rows = stmt.query([rel_path])?;
    Ok(match rows.next()? {
        Some(row) => Some(row.get(0)?),
        None => None,
    })
}

pub fn get_track_rel_path(conn: &Connection, id: i64) -> AppResult<Option<String>> {    let mut stmt = conn.prepare("SELECT rel_path FROM tracks WHERE id = ?1")?;
    let mut rows = stmt.query([id])?;
    Ok(match rows.next()? {
        Some(row) => Some(row.get(0)?),
        None => None,
    })
}

pub fn set_track_path(conn: &Connection, id: i64, rel_path: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE tracks SET rel_path = ?1, updated_at = ?2 WHERE id = ?3",
        params![rel_path, crate::util::now_ms(), id],
    )?;
    Ok(())
}

pub fn set_track_status(conn: &Connection, id: i64, status: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE tracks SET path_status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status, crate::util::now_ms(), id],
    )?;
    Ok(())
}

/// (title, artist, album, rel_path)
pub fn get_track_meta(conn: &Connection, id: i64) -> AppResult<Option<(String, String, String, String)>> {
    let mut stmt = conn.prepare("SELECT title, artist, album, rel_path FROM tracks WHERE id = ?1")?;
    let mut rows = stmt.query([id])?;
    Ok(match rows.next()? {
        Some(row) => Some((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        None => None,
    })
}

/// 按相对路径取已有标签（扫描时用于"不覆盖已有标签"的保护）
pub fn get_track_meta_by_path(
    conn: &Connection,
    rel_path: &str,
) -> AppResult<Option<(String, String, String)>> {
    let mut stmt =
        conn.prepare("SELECT title, artist, album FROM tracks WHERE rel_path = ?1 LIMIT 1")?;
    let mut rows = stmt.query([rel_path])?;
    Ok(match rows.next()? {
        Some(row) => Some((row.get(0)?, row.get(1)?, row.get(2)?)),
        None => None,
    })
}

pub fn update_track_meta(
    conn: &Connection,
    id: i64,
    title: &str,
    artist: &str,
    album: &str,
) -> AppResult<()> {
    conn.execute(
        "UPDATE tracks SET title = ?1, artist = ?2, album = ?3, updated_at = ?4 WHERE id = ?5",
        params![title, artist, album, crate::util::now_ms(), id],
    )?;
    Ok(())
}

// ───────────────────────────── 回收站 ─────────────────────────────

pub fn insert_trash(
    conn: &Connection,
    root_id: i64,
    original_rel_path: &str,
    trash_rel_path: &str,
    size_bytes: i64,
    now: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO trash (root_id, original_rel_path, trash_rel_path, size_bytes, deleted_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![root_id, original_rel_path, trash_rel_path, size_bytes, now],
    )?;
    Ok(())
}

pub fn list_trash(conn: &Connection) -> AppResult<Vec<crate::library::TrashEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, original_rel_path, trash_rel_path, size_bytes, deleted_at
           FROM trash ORDER BY deleted_at DESC LIMIT 200",
    )?;

    let rows = stmt.query_map([], |row| {
        let original: String = row.get(1)?;
        let name = original
            .rsplit('/')
            .next()
            .unwrap_or(&original)
            .to_string();
        Ok(crate::library::TrashEntry {
            id: row.get(0)?,
            name,
            original_rel_path: original,
            size_bytes: row.get(3)?,
            deleted_at: row.get(4)?,
        })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn get_trash(conn: &Connection, id: i64) -> AppResult<Option<(String, String)>> {
    let mut stmt = conn.prepare("SELECT original_rel_path, trash_rel_path FROM trash WHERE id = ?1")?;
    let mut rows = stmt.query([id])?;
    Ok(match rows.next()? {
        Some(row) => Some((row.get(0)?, row.get(1)?)),
        None => None,
    })
}

pub fn delete_trash(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM trash WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn list_trash_raw(conn: &Connection) -> AppResult<Vec<(i64, String)>> {
    let mut stmt = conn.prepare("SELECT id, trash_rel_path FROM trash")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn get_task(conn: &Connection, id: uuid::Uuid) -> AppResult<Option<DownloadTask>> {
    let sql = format!("SELECT {TASK_COLUMNS} FROM download_tasks WHERE id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![id.to_string()])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_task(row)?)),
        None => Ok(None),
    }
}

/// 删除一条**已终结**任务的记录。
///
/// 只删 SQLite 行，不动 `.part` / 已归档文件；活动任务必须先在 UI 暂停并等状态落定。
/// 返回 false 表示 id 不存在或状态已变成活动态（并发兜底）。
pub fn delete_task(conn: &Connection, id: uuid::Uuid) -> AppResult<bool> {
    let changed = conn.execute(
        "DELETE FROM download_tasks
          WHERE id = ?1 AND state IN ('completed', 'failed', 'canceled')",
        params![id.to_string()],
    )?;
    Ok(changed > 0)
}

/// 把终态任务重置为 Queued，并写入新的快照（Overwrite + 重新解析的 URL）。
///
/// 与 `reset_for_resume` 的区别：这里清零进度、丢弃旧 `.part`、强制覆盖旧归档，
/// 用于「重新下载」而不是「断点续传」。
pub fn prepare_redownload(conn: &Connection, task: &DownloadTask, now: i64) -> AppResult<()> {
    let snapshot = serde_json::to_string(&task.snapshot)
        .map_err(|e| AppError::Internal(format!("快照序列化失败: {e}")))?;

    let changed = conn.execute(
        "UPDATE download_tasks
            SET state = 'queued', error_kind = NULL, error_msg = NULL, http_status = NULL,
                attempts = 0, bytes_done = 0, bytes_total = NULL, snapshot_json = ?1,
                temp_path = ?2, final_path = NULL, updated_at = ?3,
                started_at = NULL, finished_at = NULL
          WHERE id = ?4 AND state IN ('completed', 'failed', 'canceled')",
        params![
            snapshot,
            task.temp_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            now,
            task.id.to_string(),
        ],
    )?;

    if changed == 0 {
        return Err(AppError::BadInput(
            "任务不在可重新下载状态（可能正在运行）".to_string(),
        ));
    }

    Ok(())
}

/// 把已终结的任务重新放回队列（保持同一个 id，因此复用同一个 `.part`）
pub fn reset_for_resume(conn: &Connection, id: uuid::Uuid, now: i64) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE download_tasks
            SET state = 'queued', error_kind = NULL, error_msg = NULL, http_status = NULL,
                finished_at = NULL, updated_at = ?1
          WHERE id = ?2 AND state IN ('failed', 'canceled')",
        params![now, id.to_string()],
    )?;

    if changed == 0 {
        return Err(AppError::BadInput(
            "任务不在可继续状态（可能已完成或正在运行）".to_string(),
        ));
    }
    Ok(())
}

/// 启动时调用：上次进程被强杀，状态会永远停在 downloading。
/// 这里统一收口成 failed + network，保留 .part 供用户点「继续」。
pub fn mark_interrupted(conn: &Connection, now: i64) -> AppResult<usize> {
    let changed = conn.execute(
        "UPDATE download_tasks
            SET state = 'failed', error_kind = 'network',
                error_msg = '上次退出时任务中断；.part 已保留，可继续断点续传',
                finished_at = ?1, updated_at = ?1
          WHERE state NOT IN ('completed', 'failed', 'canceled')",
        params![now],
    )?;
    Ok(changed)
}
