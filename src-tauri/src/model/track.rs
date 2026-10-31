use serde::{Deserialize, Serialize};

use super::source::SourceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathStatus {
    Ok,
    Missing,
    Moved,
}

/// 跨源可用性：决定前端源图标是否置灰
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Ok,
    NoCopyright,
    NoPermission,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAvailability {
    pub source: SourceId,
    pub availability: Availability,
    pub quality_max: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: Option<i64>,
    pub bitrate: Option<i64>,
    pub size_bytes: Option<i64>,
    pub format: Option<String>,
    /// 相对归档根的路径，是文件在库里的唯一身份
    pub rel_path: String,
    pub path_status: PathStatus,
    pub sources: Vec<SourceAvailability>,
}


