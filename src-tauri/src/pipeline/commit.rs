//! 最终路径落位：先用 `create_new` 抢一个空文件占位，再 `rename` 覆盖。
//!
//! 为什么不能直接 rename：
//!   - 两个并发任务可能算出同一个目标路径，最后写入的一方会静默覆盖另一方；
//!   - `create_new` 是内核级原子的 O_EXCL，抢到的人才有资格 rename。
//!
//! rename 本身在同一挂载点上是原子的（Linux/Android 的 rename(2)）；
//! 跨挂载点会返回 EXDEV —— 这也是 `.temp` 必须放在 root 内的原因。

use std::fs::OpenOptions;
use std::io::ErrorKind as IoErrorKind;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};
use crate::model::task::{CollisionPolicy, TaskSnapshot};
use crate::pipeline::path::{resolve_final_path, PathCtx};

#[derive(Debug)]
pub enum CommitOutcome {
    Moved(PathBuf),
    /// 策略为 Skip 且目标已存在：part 已删除，任务按"跳过"完成
    Skipped,
}

pub fn commit_part(
    part: &Path,
    root: &Path,
    snapshot: &TaskSnapshot,
    ctx: &PathCtx<'_>,
) -> AppResult<CommitOutcome> {
    for _ in 0..4 {
        let candidate =
            resolve_final_path(root, &snapshot.relative_dir, &snapshot.file_template, ctx, snapshot.collision)?;

        let Some(destination) = candidate else {
            let _ = std::fs::remove_file(part);
            return Ok(CommitOutcome::Skipped);
        };

        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Overwrite：先把旧文件删掉，create_new 才能抢到占位。
        // 不直接 rename 覆盖是为了保留"创建占位 → rename"的原子语义，
        // 同时避免 Windows 上 rename 遇到已存在文件直接失败。
        if snapshot.collision == CollisionPolicy::Overwrite {
            match std::fs::remove_file(&destination) {
                Ok(()) => {}
                Err(err) if err.kind() == IoErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }

        match OpenOptions::new().write(true).create_new(true).open(&destination) {
            Ok(_placeholder) => {
                // 占位文件句柄在 rename 前必须关闭（Windows 语义）
                drop(_placeholder);
                std::fs::rename(part, &destination)?;
                return Ok(CommitOutcome::Moved(destination));
            }
            // 被别的任务抢先占了同名 → 重新算名字
            Err(err) if err.kind() == IoErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }

    Err(AppError::Internal("连续抢占最终路径失败".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::task::QualityPref;

    fn snapshot(relative_dir: &str) -> TaskSnapshot {
        TaskSnapshot {
            root_id: 0,
            dir_template: "{source}/{artist}".to_string(),
            file_template: "{artist} - {title}.{ext}".to_string(),
            relative_dir: relative_dir.to_string(),
            collision: CollisionPolicy::Overwrite,
            quality: QualityPref::Standard,
            cover_embed: false,
            playlist_id: None,
            download_url: None,
        }
    }

    #[test]
    fn overwrite_replaces_existing_file() {
        let root = std::env::temp_dir().join(format!("sonotheca-commit-{}", uuid::Uuid::new_v4()));
        let target_dir = root.join("direct").join("flac");
        std::fs::create_dir_all(&target_dir).expect("mkdir");
        let existing = target_dir.join("Artist - Title.flac");
        std::fs::write(&existing, b"old").expect("write old");

        let part = root.join("new.part");
        std::fs::write(&part, b"new").expect("write part");

        let ctx = PathCtx {
            source: "direct",
            quality: "flac",
            artist: "Artist",
            title: "Title",
            album: "",
            track_no: None,
            ext: "flac",
        };

        let outcome = commit_part(&part, &root, &snapshot("direct/flac"), &ctx).expect("commit");
        match outcome {
            CommitOutcome::Moved(path) => {
                assert_eq!(std::fs::read(path).expect("read"), b"new");
            }
            CommitOutcome::Skipped => panic!("should not skip"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }
}
