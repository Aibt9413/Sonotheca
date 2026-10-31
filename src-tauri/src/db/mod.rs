//! SQLite 打开与迁移。WAL 是硬要求：读写并发 + 崩溃安全。
//!
//! 两种连接：
//!   - `open()`   启动时用，负责迁移 + 校验 WAL 是否真的生效
//!   - `with_conn()` 流水线/命令里用，短连接、用完即弃
//!
//! 为什么流水线不共享一个 `Connection`：`Connection` 是 `Send` 但不是 `Sync`，
//! 在 async 任务里抱着它跨 await 会让 future 不再是 `Send`，是最常见的编译墙。
//! 短连接 + WAL 的成本远低于为此引入 `spawn_blocking` 的复杂度。

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use crate::error::{AppError, AppResult};

pub mod repo;

pub const SCHEMA_VERSION: i64 = 1;

pub fn open(path: &Path) -> AppResult<Connection> {
    let connection = Connection::open(path)?;

    // journal_mode 用 query 读回确认：在不支持 WAL 的文件系统上 SQLite 会静默回落
    let mode: String = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(AppError::Internal(format!(
            "WAL 未能启用，当前 journal_mode = {mode}"
        )));
    }

    connection.execute_batch(
        "PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA temp_store = MEMORY;
         PRAGMA busy_timeout = 8000;",
    )?;

    connection.execute_batch(include_str!("migrations/0001_init.sql"))?;
    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;

    Ok(connection)
}

/// 轻量连接：不跑迁移、不改 journal_mode（WAL 已写进库头，新连接自动继承）
pub fn connect(path: &Path) -> AppResult<Connection> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(8))?;
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
    Ok(connection)
}

pub fn with_conn<T>(path: &Path, f: impl FnOnce(&Connection) -> AppResult<T>) -> AppResult<T> {
    let connection = connect(path)?;
    f(&connection)
}

pub fn schema_version(connection: &Connection) -> AppResult<i64> {
    Ok(connection.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}
