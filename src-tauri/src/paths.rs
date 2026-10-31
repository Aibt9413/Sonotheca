//! 归档根目录解析。
//!
//! 决策（R1）：归档主目录**必须是真实文件系统路径**，只有这种模式才能保证
//! `.temp` 与最终目录在同一挂载点，`fs::rename` 才是原子的。
//! SAF(content://) 仅用于后续的"导出/备份"，不参与下载落盘。

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};
use crate::util::now_ms;

pub const LIBRARY_ROOT_SETTING: &str = "library_root";

/// 返回 (root_id, 绝对路径)，并确保 `.temp/downloads`、`.temp/trash` 存在
pub fn library_root(app: &AppHandle, conn: &Connection) -> AppResult<(i64, PathBuf)> {
    let root = match stored_root(conn)? {
        Some(path) => path,
        None => default_root(app)?,
    };

    std::fs::create_dir_all(root.join(".temp").join("downloads"))?;
    std::fs::create_dir_all(root.join(".temp").join("trash"))?;

    let id = upsert_root(conn, &root)?;
    Ok((id, root))
}

fn stored_root(conn: &Connection) -> AppResult<Option<PathBuf>> {
    let mut stmt = conn.prepare("SELECT value_json FROM settings WHERE key = ?1")?;
    let mut rows = stmt.query([LIBRARY_ROOT_SETTING])?;
    if let Some(row) = rows.next()? {
        let raw: String = row.get(0)?;
        // settings 存 JSON；兼容被人手改成裸字符串的情况
        let value: String = serde_json::from_str(&raw).unwrap_or(raw);
        if !value.trim().is_empty() {
            return Ok(Some(PathBuf::from(value)));
        }
    }
    Ok(None)
}

fn default_root(app: &AppHandle) -> AppResult<PathBuf> {
    #[cfg(target_os = "android")]
    {
        // Android：私有目录，零权限、零 SAF 摩擦；卸载即清（正式版应引导用户导出）
        Ok(app.path().app_data_dir()?.join("library"))
    }
    #[cfg(not(target_os = "android"))]
    {
        // 桌面：放在音乐目录下，用户能直接看到归档结果
        let base = app
            .path()
            .audio_dir()
            .or_else(|_| app.path().app_data_dir())?;
        Ok(base.join("Sonotheca"))
    }
}

fn upsert_root(conn: &Connection, root: &Path) -> AppResult<i64> {
    let uri = root.to_string_lossy().to_string();

    if let Ok(id) = conn.query_row(
        "SELECT id FROM library_roots WHERE uri = ?1 ORDER BY id LIMIT 1",
        [&uri],
        |row| row.get::<_, i64>(0),
    ) {
        return Ok(id);
    }

    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "library".to_string());

    conn.execute(
        "INSERT INTO library_roots (kind, uri, display_name, is_default, created_at)
         VALUES ('native', ?1, ?2, 1, ?3)",
        rusqlite::params![uri, name, now_ms()],
    )?;

    let id = conn.last_insert_rowid();
    if id <= 0 {
        return Err(AppError::Internal("归档根目录写入失败".to_string()));
    }
    Ok(id)
}

/// 下载暂存目录：{root}/.temp/downloads
pub fn downloads_dir(root: &Path) -> PathBuf {
    root.join(".temp").join("downloads")
}


