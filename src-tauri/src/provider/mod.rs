//! 音源适配层。
//!
//! `SourceAdapter` 是唯一允许出现"平台差异"的地方：上层（命令、下载流水线、前端）
//! 只认 `SearchHit` / `ResolvedStream` 这两个 DTO。
//!
//! M4 只接了网易云，且是**匿名态**（无 Cookie）：
//!   - 搜索可用
//!   - 取直链受版权限制，未登录通常只拿到 128K/320K，部分歌曲直接返回 null

pub mod auth;
pub mod bilibili;
pub mod navidrome;
pub mod netease;
pub mod qq;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::model::source::SourceId;
use crate::model::task::QualityPref;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub source: SourceId,
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_url: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ResolvedStream {
    pub url: String,
    /// 部分 CDN 需要 Referer，下载时透传
    pub referer: Option<String>,
}

#[async_trait]
pub trait SourceAdapter: Send + Sync {
    #[allow(dead_code)]
    fn source(&self) -> SourceId;

    async fn search(&self, keyword: &str, offset: u32, limit: u32) -> AppResult<Vec<SearchHit>>;

    async fn get_stream_url(
        &self,
        track_id: &str,
        quality: QualityPref,
    ) -> AppResult<ResolvedStream>;
}
