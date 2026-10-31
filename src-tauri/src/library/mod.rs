//! 文件管理：目录浏览、递归扫描建档、回收站。
//!
//! 三条纪律：
//!   1. **扫描绝不改文件名**，只读元数据入库；
//!   2. 删除一律先进 `library/.temp/trash/`，可恢复；
//!   3. 所有路径都是相对 library root 的，UI 只认相对路径。

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::db::{self, repo};
use crate::error::{AppError, AppResult};
use crate::util::now_ms;

const AUDIO_EXTS: &[&str] = &["flac", "mp3", "m4a", "ogg", "opus", "wav", "aac", "ape"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub rel_path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub track_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListing {
    pub rel_path: String,
    pub parent: Option<String>,
    pub dirs: Vec<DirEntry>,
    pub files: Vec<DirEntry>,
}

fn safe_join(root: &Path, rel: &str) -> AppResult<PathBuf> {
    let rel = rel.trim_start_matches(['/', '\\']).replace('\\', "/");
    let joined = root.join(&rel);

    // 防目录穿越：拼出来的路径必须仍在 root 下
    if !joined.starts_with(root) {
        return Err(AppError::BadInput("路径越界".to_string()));
    }
    Ok(joined)
}

pub fn list_dir(root: &Path, conn: &rusqlite::Connection, rel_path: &str) -> AppResult<DirListing> {
    let dir = safe_join(root, rel_path)?;
    if !dir.is_dir() {
        return Err(AppError::BadInput("目录不存在".to_string()));
    }

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();

        // .temp 属于内部目录，不在文件管理器里展示
        if name.starts_with('.') {
            continue;
        }

        let meta = entry.metadata().ok();
        let child_rel = if rel_path.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", rel_path.trim_end_matches('/'), name)
        };

        if entry.path().is_dir() {
            dirs.push(DirEntry {
                name,
                rel_path: child_rel,
                is_dir: true,
                size: None,
                track_id: None,
            });
        } else {
            let ext = entry
                .path()
                .extension()
                .map(|v| v.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if !AUDIO_EXTS.contains(&ext.as_str()) {
                continue;
            }
            let track_id = repo::find_track_by_path(conn, &child_rel).ok().flatten();
            files.push(DirEntry {
                name,
                rel_path: child_rel,
                is_dir: false,
                size: meta.map(|m| m.len()),
                track_id,
            });
        }
    }

    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    files.sort_by(|a, b| a.name.cmp(&b.name));

    let parent = if rel_path.is_empty() {
        None
    } else {
        Some(
            rel_path
                .rsplit_once('/')
                .map(|(head, _)| head.to_string())
                .unwrap_or_default(),
        )
    };

    Ok(DirListing {
        rel_path: rel_path.to_string(),
        parent,
        dirs,
        files,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub scanned: u32,
    pub inserted: u32,
    pub current: String,
    pub done: bool,
}

/// 递归扫描建档。**只读元数据，绝不改文件名。**
pub fn scan_tree(app: &AppHandle, db_path: &Path, root: &Path, root_id: i64, rel_path: &str) {
    let dir = match safe_join(root, rel_path) {
        Ok(dir) => dir,
        Err(_) => return,
    };

    let mut queue: Vec<PathBuf> = Vec::new();
    collect_audio(&dir, &mut queue);

    let total = queue.len() as u32;
    let mut scanned = 0u32;
    let mut inserted = 0u32;
    let mut last_emit = std::time::Instant::now();

    for path in queue {
        scanned += 1;

        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let rel = relative.to_string_lossy().replace('\\', "/");

        // 用真实标签建档；读不出来就退回文件名
        let fallback_title = path
            .file_stem()
            .map(|v| v.to_string_lossy().to_string())
            .unwrap_or_else(|| "未命名".to_string());

        let (title, artist, album, duration_ms) =
            resolve_metadata(db_path, root_id, &rel, &path, fallback_title);

        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let bitrate = match duration_ms {
            Some(ms) if ms > 0 => {
                let bps = (size as u128 * 8 * 1000) / ms as u128;
                if bps > 0 {
                    Some(bps as i64)
                } else {
                    None
                }
            }
            _ => None,
        };
        let format = path
            .extension()
            .map(|v| v.to_string_lossy().to_ascii_lowercase());

        let _ = db::with_conn(db_path, |conn| {
            repo::upsert_track(
                conn,
                &repo::NewTrack {
                    root_id,
                    rel_path: &rel,
                    title: &title,
                    artist: &artist,
                    album: &album,
                    duration_ms: duration_ms.map(|v| v as i64),
                    bitrate,
                    size_bytes: Some(size as i64),
                    format: format.as_deref(),
                },
                now_ms(),
            )
            .map(|_| ())
        });

        inserted += 1;

        if last_emit.elapsed() >= std::time::Duration::from_millis(200) || scanned == total {
            last_emit = std::time::Instant::now();
            let _ = app.emit(
                "library://scan-progress",
                ScanProgress {
                    scanned,
                    inserted,
                    current: rel.clone(),
                    done: false,
                },
            );
        }
    }

    let _ = app.emit(
        "library://scan-progress",
        ScanProgress {
            scanned,
            inserted,
            current: String::new(),
            done: true,
        },
    );

    let _ = app.emit("library://changed", ());
}

fn collect_audio(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            collect_audio(&path, out);
        } else {
            let ext = path
                .extension()
                .map(|v| v.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if AUDIO_EXTS.contains(&ext.as_str()) {
                out.push(path);
            }
        }
    }
}

/// 只读标签，不写文件（扫描阶段不能改动任何东西）
fn probe_metadata(path: &Path) -> Option<(String, String, String, Option<u64>)> {
    use lofty::file::TaggedFileExt;
    use lofty::prelude::{Accessor, AudioFile};

    let tagged = lofty::probe::Probe::open(path).ok()?.read().ok()?;
    let duration = tagged.properties().duration().as_millis() as u64;

    let title = tagged
        .primary_tag()
        .and_then(|tag| tag.title())
        .map(|v| v.to_string())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|v| v.to_string_lossy().to_string())
                .unwrap_or_else(|| "未命名".to_string())
        });

    let artist = tagged
        .primary_tag()
        .and_then(|tag| tag.artist())
        .map(|v| v.to_string())
        .unwrap_or_default();

    let album = tagged
        .primary_tag()
        .and_then(|tag| tag.album())
        .map(|v| v.to_string())
        .unwrap_or_default();

    Some((
        title,
        artist,
        album,
        if duration > 0 { Some(duration) } else { None },
    ))
}

// ───────────────────────────── 回收站 ─────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    pub id: i64,
    pub name: String,
    pub original_rel_path: String,
    pub size_bytes: Option<i64>,
    pub deleted_at: i64,
}

pub fn trash_track(root: &Path, conn: &rusqlite::Connection, track_id: i64) -> AppResult<()> {
    let rel_path = repo::get_track_rel_path(conn, track_id)?
        .ok_or_else(|| AppError::BadInput("曲目不存在".to_string()))?;

    let source = safe_join(root, &rel_path)?;
    if !source.is_file() {
        repo::set_track_status(conn, track_id, "missing")?;
        return Ok(());
    }

    let trash_dir = root.join(".temp").join("trash");
    std::fs::create_dir_all(&trash_dir)?;

    let ext = source
        .extension()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();

    let trash_name = if ext.is_empty() {
        format!("{}.bin", Uuid::new_v4())
    } else {
        format!("{}.{}", Uuid::new_v4(), ext)
    };
    let trash_path = trash_dir.join(&trash_name);

    std::fs::rename(&source, &trash_path)?;

    let size = std::fs::metadata(&trash_path).map(|m| m.len()).unwrap_or(0);
    repo::insert_trash(conn, 0, &rel_path, &trash_name, size as i64, now_ms())?;
    repo::set_track_status(conn, track_id, "missing")?;

    Ok(())
}

pub fn list_trash(conn: &rusqlite::Connection) -> AppResult<Vec<TrashEntry>> {
    repo::list_trash(conn)
}

pub fn restore_trash(root: &Path, conn: &rusqlite::Connection, trash_id: i64) -> AppResult<()> {
    let (original_rel, trash_name) = repo::get_trash(conn, trash_id)?
        .ok_or_else(|| AppError::BadInput("回收站记录不存在".to_string()))?;

    let source = root.join(".temp").join("trash").join(&trash_name);
    let destination = safe_join(root, &original_rel)?;

    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if source.is_file() {
        std::fs::rename(&source, &destination)?;
    }

    repo::delete_trash(conn, trash_id)?;

    if let Ok(Some(track_id)) = repo::find_track_by_path(conn, &original_rel) {
        repo::set_track_status(conn, track_id, "ok")?;
    }

    Ok(())
}

pub fn purge_trash(
    root: &Path,
    conn: &rusqlite::Connection,
    trash_id: Option<i64>,
) -> AppResult<u32> {
    let entries = match trash_id {
        Some(id) => repo::get_trash(conn, id)?
            .map(|(_, name)| vec![(id, name)])
            .unwrap_or_default(),
        None => repo::list_trash_raw(conn)?,
    };

    let mut removed = 0;
    for (id, name) in entries {
        let path = root.join(".temp").join("trash").join(&name);
        let _ = std::fs::remove_file(path);
        let _ = repo::delete_trash(conn, id);
        removed += 1;
    }
    Ok(removed)
}

// ───────────────────────────── 存储空间（全局/应用） ─────────────────────────────

/// "app" = 应用私有库根目录；"global" = /storage/emulated/0
pub fn space_base(app_root: &Path, space: &str) -> PathBuf {
    if space == "global" {
        PathBuf::from("/storage/emulated/0")
    } else {
        app_root.to_path_buf()
    }
}

pub fn list_space(
    app_root: &Path,
    conn: &rusqlite::Connection,
    space: &str,
    rel_path: &str,
) -> AppResult<DirListing> {
    let base = space_base(app_root, space);
    if !base.is_dir() {
        return Err(AppError::BadInput(
            "该存储空间不可用（可能没有授权）".to_string(),
        ));
    }
    list_dir(&base, conn, rel_path)
}

/// 跨空间移动/复制。跨分区 rename 会 EXDEV，自动降级为 copy + remove。
#[allow(clippy::too_many_arguments)]
pub fn transfer_between(
    app_root: &Path,
    conn: &rusqlite::Connection,
    from_space: &str,
    rel_paths: &[String],
    to_space: &str,
    target_dir: &str,
    mode: &str,
) -> AppResult<u32> {
    let source_base = space_base(app_root, from_space);
    let target_base = space_base(app_root, to_space);

    let target = safe_join(&target_base, target_dir)?;
    std::fs::create_dir_all(&target)?;

    let mut moved = 0u32;

    for rel in rel_paths {
        let source = safe_join(&source_base, rel)?;
        if !source.is_file() {
            continue;
        }
        let Some(name) = source.file_name() else {
            continue;
        };

        let destination = unique_destination(&target, name);

        if mode == "copy" {
            std::fs::copy(&source, &destination)?;
            moved += 1;
            continue;
        }

        move_file(&source, &destination)?;

        if from_space == "app" {
            if let Ok(Some(track_id)) = repo::find_track_by_path(conn, rel) {
                if to_space == "app" {
                    if let Ok(new_rel) = destination.strip_prefix(app_root) {
                        let new_rel = new_rel.to_string_lossy().replace('\\', "/");
                        let _ = repo::set_track_path(conn, track_id, &new_rel);
                    }
                } else {
                    // 移出应用目录 = 离开音乐库，让卡片从列表消失
                    let _ = repo::set_track_status(conn, track_id, "missing");
                }
            }
        }

        moved += 1;
    }

    Ok(moved)
}

fn unique_destination(target: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let mut destination = target.join(name);
    let stem = Path::new(name)
        .file_stem()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = Path::new(name)
        .extension()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut index = 1;
    while destination.exists() {
        let candidate = if ext.is_empty() {
            format!("{stem} ({index})")
        } else {
            format!("{stem} ({index}).{ext}")
        };
        destination = target.join(candidate);
        index += 1;
    }
    destination
}

pub fn move_file(source: &Path, destination: &Path) -> AppResult<()> {
    match std::fs::rename(source, destination) {
        Ok(_) => Ok(()),
        // 应用私有目录 → 公共存储是跨分区，rename 不可用
        Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
            std::fs::copy(source, destination)?;
            std::fs::remove_file(source)?;
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}


/// 扫描时的元数据裁决：**绝不拿文件名覆盖已有标签**。
///
/// 规则：
///   1. lofty 读到了 Title/Artist → 直接用（这是文件里的真相）
///   2. lofty 什么都没读到（标题退化成文件名、艺术家为空）→ 库里若已有非空标题就保留
///   3. 库里也没有 → 才用文件名兜底
fn resolve_metadata(
    db_path: &Path,
    root_id: i64,
    rel: &str,
    path: &Path,
    fallback_title: String,
) -> (String, String, String, Option<u64>) {
    let stem = path
        .file_stem()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();

    let Some((title, artist, album, duration)) = probe_metadata(path) else {
        return (fallback_title, String::new(), String::new(), None);
    };

    // 没有任何 tag 的典型特征：艺术家与专辑都空，标题正好等于文件名
    let tags_missing = artist.trim().is_empty() && album.trim().is_empty() && title == stem;

    if !tags_missing {
        return (title, artist, album, duration);
    }

    // 保留库里已有的标签
    let existing = db::with_conn(db_path, |conn| {
        repo::get_track_meta_by_path(conn, rel).map(|row| row)
    })
    .ok()
    .flatten();

    if let Some((stored_title, stored_artist, stored_album)) = existing {
        if !stored_title.trim().is_empty() {
            return (stored_title, stored_artist, stored_album, duration);
        }
    }

    let _ = root_id;
    (fallback_title, artist, album, duration)
}
