//! 分类器：模板变量替换 → 非法字符清洗 → 重名探测 → 最终绝对路径。
//!
//! 纯函数、不依赖 DB 与 AppState，方便单测与后续压测。
//! 三条硬规则：
//!   1. 所有分量都按 UTF-8 字符边界截断，绝不切出半个汉字。
//!   2. 任何 `..` / 根前缀都不会被解出，最终路径必须仍在 root 下。
//!   3. 结果路径是"候选"，真正的占位由调用方用 create_new 抢锁，再 rename 覆盖。

use std::path::{Component, Path, PathBuf};

use crate::model::task::CollisionPolicy;

/// 跨平台非法字符（Windows 最严，取并集）
const ILLEGAL: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'];
/// Windows 保留设备名（桌面端才需要，但统一处理成本更低）
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "LPT1", "LPT2", "LPT3",
];
/// ext4 / NTFS 单个路径分量的字节上限
const MAX_SEGMENT_BYTES: usize = 255;
const MAX_RENAME_TRIES: u32 = 64;

#[derive(Debug, Clone)]
pub struct PathCtx<'a> {
    pub source: &'a str,
    pub quality: &'a str,
    pub artist: &'a str,
    pub title: &'a str,
    pub album: &'a str,
    pub track_no: Option<u32>,
    /// 不带点，例如 "flac"
    pub ext: &'a str,
}

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("模板渲染后没有有效路径分量")]
    EmptyTemplate,
    #[error("路径逃逸出归档根目录")]
    Escapes,
    #[error("重名探测 {0} 次后仍无可用名字")]
    CollisionExhausted(u32),
}



/// 单段清洗。
///
/// 非法字符与空白都折叠成一个空格而不是直接删除 —— "AC/DC" 应该变成 "AC DC"
/// 而不是 "ACDC"，否则归档后的可读性会被破坏。
pub fn sanitize_segment(raw: &str, max_bytes: usize) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    let mut pending_sep = false;

    for ch in raw.chars() {
        if ch.is_control() || ILLEGAL.contains(&ch) || ch.is_whitespace() {
            pending_sep = true;
        } else {
            if pending_sep && !cleaned.is_empty() {
                cleaned.push(' ');
            }
            pending_sep = false;
            cleaned.push(ch);
        }
    }

    // 尾部不能是 '.' 或 ' '（Windows 会截断出问题）
    let trimmed = cleaned.trim().trim_end_matches('.');
    let mut out = if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    };

    if out.len() > max_bytes {
        let mut cut = max_bytes;
        while cut > 0 && !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out = out.trim().trim_end_matches('.').to_string();
    }

    if out.is_empty() {
        return "untitled".to_string();
    }

    if RESERVED.iter().any(|name| out.eq_ignore_ascii_case(name)) {
        out.insert(0, '_');
    }

    out
}

fn render(template: &str, ctx: &PathCtx<'_>) -> String {
    let track_no = ctx.track_no.map(|n| format!("{n:02}")).unwrap_or_default();
    let pairs: [(&str, &str); 7] = [
        ("{source}", ctx.source),
        ("{quality}", ctx.quality),
        ("{artist}", ctx.artist),
        ("{title}", ctx.title),
        ("{album}", ctx.album),
        ("{track_no}", track_no.as_str()),
        ("{ext}", ctx.ext),
    ];

    let mut out = template.to_string();
    for (needle, value) in pairs {
        out = out.replace(needle, value);
    }
    out
}

/// 模板 → 安全的相对路径（已逐段清洗）
fn build_relative(template: &str, ctx: &PathCtx<'_>) -> Result<PathBuf, PathError> {
    let rendered = render(template, ctx);
    let mut relative = PathBuf::new();

    for segment in rendered.split(|c: char| c == '/' || c == '\\') {
        if segment.trim().is_empty() {
            continue;
        }
        relative.push(sanitize_segment(segment, MAX_SEGMENT_BYTES));
    }

    if relative.as_os_str().is_empty() {
        return Err(PathError::EmptyTemplate);
    }
    Ok(relative)
}

/// 拼接 root + 快照目录 + 文件名；只接受 Normal 分量，ParentDir 一律丢弃
fn join_under_root(root: &Path, snapshot_dir: &str, file: &Path) -> PathBuf {
    let mut full = root.to_path_buf();

    for segment in snapshot_dir.split(|c: char| c == '/' || c == '\\') {
        if segment.trim().is_empty() {
            continue;
        }
        full.push(sanitize_segment(segment, MAX_SEGMENT_BYTES));
    }

    for component in file.components() {
        if let Component::Normal(part) = component {
            full.push(part);
        }
    }

    full
}

/// 渲染目录模板（入队时用），返回以 `/` 分隔的相对目录。
///
/// 变量值里如果混进 `/`，会被拆成多级目录并逐段清洗 —— 这是有意的：
/// 让 `{artist}` 里的 "AC/DC" 也能自然地落到 `AC/DC` 两个目录，而不是一个含斜杠的非法名字。
pub fn render_relative_dir(dir_template: &str, ctx: &PathCtx<'_>) -> Result<String, PathError> {
    let relative = build_relative(dir_template, ctx)?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

/// 生成最终路径。
///
/// 返回 `Ok(None)` 表示策略是 Skip 且已存在（调用方把任务标记为 Completed-skipped）。
pub fn resolve_final_path(
    root: &Path,
    snapshot_dir: &str,
    file_template: &str,
    ctx: &PathCtx<'_>,
    policy: CollisionPolicy,
) -> Result<Option<PathBuf>, PathError> {
    let relative_file = build_relative(file_template, ctx)?;
    let candidate = join_under_root(root, snapshot_dir, &relative_file);

    if !candidate.starts_with(root) {
        return Err(PathError::Escapes);
    }

    if !candidate.exists() {
        return Ok(Some(candidate));
    }

    match policy {
        CollisionPolicy::Overwrite => Ok(Some(candidate)),
        CollisionPolicy::Skip => Ok(None),
        CollisionPolicy::Rename => nth_available(&candidate, MAX_RENAME_TRIES).map(Some),
    }
}

/// "标题 (1).flac" / "标题 (2).flac" …… 加序号后仍要保证分量不超长
fn nth_available(base: &Path, cap: u32) -> Result<PathBuf, PathError> {
    let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("track");
    let extension = base.extension().and_then(|s| s.to_str()).unwrap_or("bin");
    let parent = base.parent().unwrap_or_else(|| Path::new("."));

    for index in 1..=cap {
        let suffix = format!(" ({index})");
        let budget = MAX_SEGMENT_BYTES
            .saturating_sub(extension.len() + 1 + suffix.len())
            .max(1);

        let mut name = sanitize_segment(stem, budget);
        name.push_str(&suffix);

        let candidate = parent.join(format!("{name}.{extension}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(PathError::CollisionExhausted(cap))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(artist: &'a str, title: &'a str) -> PathCtx<'a> {
        PathCtx {
            source: "direct",
            quality: "flac",
            artist,
            title,
            album: "",
            track_no: None,
            ext: "flac",
        }
    }

    #[test]
    fn illegal_chars_become_single_spaces() {
        assert_eq!(sanitize_segment("AC/DC: Back*In?Black", 255), "AC DC Back In Black");
    }

    #[test]
    fn utf8_truncation_never_splits_a_char() {
        let long = "水母".repeat(100); // 600 字节
        let out = sanitize_segment(&long, 20);
        assert!(out.len() <= 20, "got {} bytes", out.len());
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        assert!(!out.contains('\u{FFFD}'));
    }

    #[test]
    fn reserved_device_name_is_prefixed() {
        assert_eq!(sanitize_segment("CON", 255), "_CON");
        assert_eq!(sanitize_segment("nul", 255), "_nul");
    }

    #[test]
    fn empty_and_dots_fall_back_to_untitled() {
        assert_eq!(sanitize_segment("   ", 255), "untitled");
        assert_eq!(sanitize_segment("...", 255), "untitled");
    }

    #[test]
    fn template_variables_are_replaced() {
        let rendered = render("{artist} - {title}.{ext}", &ctx("水母", "深海"));
        assert_eq!(rendered, "水母 - 深海.flac");
    }

    #[test]
    fn traversal_attempts_stay_inside_root() {
        let root = Path::new("/library");
        let evil = ctx("../../etc", "passwd");
        let out = resolve_final_path(
            root,
            "../../escape",
            "{artist}/{title}.{ext}",
            &evil,
            CollisionPolicy::Rename,
        )
        .expect("should not error")
        .expect("should produce a path");

        assert!(out.starts_with(root), "escaped to {out:?}");
        assert!(!out.to_string_lossy().contains(".."));
    }

    #[test]
    fn rename_policy_adds_incrementing_suffix() {
        let root = std::env::temp_dir().join(format!("sonotheca-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("mkdir");

        let c = ctx("水母", "深海");
        let first = resolve_final_path(&root, "direct/flac", "{artist} - {title}.{ext}", &c, CollisionPolicy::Rename)
            .expect("ok")
            .expect("some");
        // resolve_final_path 只负责算路径，建目录是 commit_part 的职责
        if let Some(parent) = first.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent");
        }
        std::fs::write(&first, b"x").expect("write");

        let second = resolve_final_path(&root, "direct/flac", "{artist} - {title}.{ext}", &c, CollisionPolicy::Rename)
            .expect("ok")
            .expect("some");

        assert_ne!(first, second);
        assert!(second.to_string_lossy().ends_with("(1).flac"));
        assert!(second.starts_with(&root));

        let _ = std::fs::remove_dir_all(&root);
    }
}
