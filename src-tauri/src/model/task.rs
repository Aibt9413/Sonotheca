use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::source::SourceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Preflight,
    Resolving,
    Downloading,
    Processing,
    Verifying,
    Completed,
    Failed,
    Canceled,
}

impl TaskState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Preflight => "preflight",
            Self::Resolving => "resolving",
            Self::Downloading => "downloading",
            Self::Processing => "processing",
            Self::Verifying => "verifying",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// 401/403：停止重试，提示登录
    Auth,
    /// 404：自动换源
    NotFound,
    /// 429：退避
    RateLimited,
    /// 超时：指数退避
    Timeout,
    Network,
    Decrypt,
    TagWrite,
    Io,
    DiskFull,
    Canceled,
    Unknown,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::NotFound => "not_found",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::Decrypt => "decrypt",
            Self::TagWrite => "tag_write",
            Self::Io => "io",
            Self::DiskFull => "disk_full",
            Self::Canceled => "canceled",
            Self::Unknown => "unknown",
        }
    }

    /// 从数据库读回时用。未知值一律 Unknown，不让脏数据炸掉列表。
    pub fn parse(value: &str) -> Self {
        match value {
            "auth" => Self::Auth,
            "not_found" => Self::NotFound,
            "rate_limited" => Self::RateLimited,
            "timeout" => Self::Timeout,
            "network" => Self::Network,
            "decrypt" => Self::Decrypt,
            "tag_write" => Self::TagWrite,
            "io" => Self::Io,
            "disk_full" => Self::DiskFull,
            "canceled" => Self::Canceled,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskError {
    pub kind: ErrorKind,
    pub message: String,
    pub http_status: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPolicy {
    Skip,
    #[default]
    Rename,
    Overwrite,
}

/// 下载音质偏好（六档，覆盖 SVIP 全部档位）。
///
/// serde 保留旧别名：老 snapshot JSON 里的 `high` / `best` / `data_saver`
/// 都归到 [`QualityPref::Exhigh`]，避免老任务反序列化失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityPref {
    /// 默认：跟随设置（当前实现 = 无损）
    #[default]
    Lossless,
    Standard,
    /// 极高 320K（旧数据里的 high / best / data_saver 都归到这里）
    #[serde(alias = "high", alias = "best", alias = "data_saver")]
    Exhigh,
    /// snake_case 会拆成 `hi_res`，但接口 / 前端约定是 `hires`，这里显式覆写
    #[serde(rename = "hires", alias = "hi_res")]
    HiRes,
    Jymaster,
    Sky,
}

impl QualityPref {
    /// 网易云 `/song/enhance/player/url/v1` 的 level 参数
    pub fn level(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Exhigh => "exhigh",
            Self::Lossless => "lossless",
            Self::HiRes => "hires",
            Self::Jymaster => "jymaster",
            Self::Sky => "sky",
        }
    }

    /// 沉浸声需要额外参数
    pub fn immerse_type(self) -> Option<&'static str> {
        match self {
            Self::Sky => Some("c51"),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "标准 128K",
            Self::Exhigh => "极高 320K",
            Self::Lossless => "无损",
            Self::HiRes => "Hi-Res",
            Self::Jymaster => "超清母带",
            Self::Sky => "沉浸声",
        }
    }

    /// 从 UI / 命令参数解析。无法识别时按默认档（无损）处理。
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "standard" | "128" => Self::Standard,
            "exhigh" | "higher" | "320" => Self::Exhigh,
            "hires" | "hi-res" | "hi_res" => Self::HiRes,
            "jymaster" | "master" => Self::Jymaster,
            "sky" | "immersive" => Self::Sky,
            _ => Self::Lossless,
        }
    }

    /// 是否属于「必须有登录 Cookie 才能拿」的档位
    pub fn needs_login(self) -> bool {
        !matches!(self, Self::Standard | Self::Exhigh)
    }
}

/// 入队瞬间固化。之后用户改分类模板 / 质量偏好 / 防重名策略，都不影响在途任务。
///
/// 注意 `relative_dir`：它是**用入队时已知的变量**渲染出来的目录前缀。
/// 码率（`{quality}`）在下载完成前不可知，因此默认目录模板不含该变量 ——
/// 否则会产出 `direct/unknown/...` 这种垃圾目录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub root_id: i64,
    /// 目录模板，例："{source}/{artist}"
    pub dir_template: String,
    /// 文件名模板，例："{artist} - {title}.{ext}"
    pub file_template: String,
    /// 入队时渲染并冻结的目录前缀（不含文件名）
    pub relative_dir: String,
    pub collision: CollisionPolicy,
    pub quality: QualityPref,
    pub cover_embed: bool,
    pub playlist_id: Option<i64>,
    /// 入队时解析好的真实下载地址（音源 CDN 直链带时效签名）。
    /// 为空时回退用 `source_track_id` 当 URL（直链源的场景）。
    #[serde(default)]
    pub download_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTask {
    pub id: Uuid,
    pub source: SourceId,
    pub source_track_id: String,
    pub title: String,
    pub artist: String,
    pub state: TaskState,
    pub snapshot: TaskSnapshot,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub attempts: u32,
    pub error: Option<TaskError>,
    /// {root}/.temp/downloads/{uuid}.part
    pub temp_path: Option<PathBuf>,
    pub final_path: Option<PathBuf>,
    pub created_at: i64,
    pub updated_at: i64,
}


#[cfg(test)]
mod tests {
    use super::QualityPref;

    #[test]
    fn old_snapshot_values_still_deserialize() {
        // 老版本 snapshot_json 里可能还是 best / high / data_saver，不能炸
        let cases = [
            ("\"best\"", QualityPref::Exhigh),
            ("\"high\"", QualityPref::Exhigh),
            ("\"data_saver\"", QualityPref::Exhigh),
            ("\"lossless\"", QualityPref::Lossless),
            ("\"hires\"", QualityPref::HiRes),
            ("\"sky\"", QualityPref::Sky),
        ];

        for (raw, expected) in cases {
            let parsed: QualityPref = serde_json::from_str(raw).expect(raw);
            assert_eq!(parsed, expected, "raw = {raw}");
        }
    }

    #[test]
    fn parses_ui_values() {
        assert_eq!(QualityPref::parse("320"), QualityPref::Exhigh);
        assert_eq!(QualityPref::parse("higher"), QualityPref::Exhigh);
        assert_eq!(QualityPref::parse("  Hi-Res "), QualityPref::HiRes);
        assert_eq!(QualityPref::parse("jymaster"), QualityPref::Jymaster);
        // 未知 / 空 / default 一律回落到默认档
        assert_eq!(QualityPref::parse("default"), QualityPref::Lossless);
        assert_eq!(QualityPref::parse(""), QualityPref::Lossless);
    }

    #[test]
    fn serde_names_match_api_levels() {
        // 前端 bindings.ts 的 NeteaseQuality 与 settings 表都按 level 名走，
        // 序列化结果必须逐字一致（hires 而不是 hi_res）。
        let all = [
            QualityPref::Standard,
            QualityPref::Exhigh,
            QualityPref::Lossless,
            QualityPref::HiRes,
            QualityPref::Jymaster,
            QualityPref::Sky,
        ];

        for quality in all {
            let encoded = serde_json::to_string(&quality).expect("serialize");
            assert_eq!(encoded, format!("\"{}\"", quality.level()), "{quality:?}");
        }
    }

    #[test]
    fn sky_is_immersive_and_high_tiers_need_login() {
        assert_eq!(QualityPref::Sky.immerse_type(), Some("c51"));
        assert_eq!(QualityPref::Lossless.immerse_type(), None);
        assert!(QualityPref::Lossless.needs_login());
        assert!(QualityPref::Sky.needs_login());
        assert!(!QualityPref::Standard.needs_login());
        assert!(!QualityPref::Exhigh.needs_login());
    }
}
