//! 统一错误类型。
//!
//! 对前端只暴露 `{ kind, message }`；对下载流水线则通过 `error_kind()`
//! 映射成 `ErrorKind`，由它决定"是否重试 / 是否换源 / 是否终止"。

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::model::task::ErrorKind;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("路径错误: {0}")]
    Path(#[from] crate::pipeline::path::PathError),
    #[error("参数错误: {0}")]
    BadInput(String),
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("网络错误: {0}")]
    Network(String),
    #[error("请求超时: {0}")]
    Timeout(String),
    #[error("音频解析失败: {0}")]
    NotAudio(String),
    #[error("该歌曲暂不可用（可能无版权或需要登录）")]
    Unavailable,
    /// 音源明确要求登录（例如 QQ 匿名 vkey 返回空 purl）。
    /// 消息本身已经是给用户看的完整文案，前端直接展示。
    #[error("{0}")]
    AuthRequired(String),
    #[error("音源返回的是加密格式（.ncm），暂不支持解密: {0}")]
    EncryptedFormat(String),
    #[error("NCM 解密失败: {0}")]
    Decrypt(String),
    #[error("标签写入失败: {0}")]
    Tag(String),
    #[error("任务已取消")]
    Canceled,
    #[error("内部错误: {0}")]
    Internal(String),
}

impl AppError {
    /// 决定重试策略的分类。401/403 → Auth（终止并提示登录），404 → NotFound（换源）。
    pub fn error_kind(&self) -> ErrorKind {
        match self {
            Self::Db(_) | Self::Io(_) | Self::Path(_) => ErrorKind::Io,
            Self::Http { status, .. } => match status {
                401 | 403 => ErrorKind::Auth,
                404 | 410 => ErrorKind::NotFound,
                408 | 504 => ErrorKind::Timeout,
                429 => ErrorKind::RateLimited,
                _ => ErrorKind::Network,
            },
            Self::Network(_) => ErrorKind::Network,
            Self::Timeout(_) => ErrorKind::Timeout,
            Self::Canceled => ErrorKind::Canceled,
            Self::Unavailable => ErrorKind::NotFound,
            Self::AuthRequired(_) => ErrorKind::Auth,
            Self::EncryptedFormat(_) | Self::Decrypt(_) => ErrorKind::Decrypt,
            Self::NotAudio(_) | Self::Tag(_) => ErrorKind::Unknown,
            Self::BadInput(_) | Self::Internal(_) => ErrorKind::Unknown,
        }
    }

    /// 给前端看的错误分类（与 ErrorKind 同源，避免两套字符串）
    pub fn kind(&self) -> &'static str {
        self.error_kind().as_str()
    }

    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

impl From<tauri::Error> for AppError {
    fn from(value: tauri::Error) -> Self {
        Self::Internal(value.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
