//! 元数据刮削：提取 → 搜索 → 打分 → 写回。
//!
//! 流程刻意分成两步：
//!   `preview` 只**读**，返回当前信息与候选列表给前端做对比；
//!   `apply` 才真正用 lofty 写标签与封面。
//!
//! 绝不在用户确认前改动文件 —— 这也是 UI 要弹 Bottom Sheet 的原因。

use std::path::Path;

use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{Picture, PictureType};
use lofty::prelude::{Accessor, AudioFile};
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::provider::SourceAdapter;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaView {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrapeCandidate {
    pub source: String,
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub score: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrapePreview {
    pub current: MetaView,
    pub candidates: Vec<ScrapeCandidate>,
}

/// 标题里常常自带 "Artist - Title"，标签为空时用它兜底
fn split_name(title: &str, artist: &str) -> (String, String) {
    let title = title.trim();
    let artist = artist.trim();

    if artist.is_empty() {
        if let Some((left, right)) = title.split_once(" - ") {
            return (right.trim().to_string(), left.trim().to_string());
        }
    }
    (title.to_string(), artist.to_string())
}

fn normalize(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn score(candidate: &ScrapeCandidate, title: &str, artist: &str) -> i32 {
    let mut score = 0;

    let ct = normalize(&candidate.title);
    let qt = normalize(title);
    if !qt.is_empty() {
        if ct == qt {
            score += 60;
        } else if ct.contains(&qt) || qt.contains(&ct) {
            score += 30;
        }
    }

    let ca = normalize(&candidate.artist);
    let qa = normalize(artist);
    if !qa.is_empty() && !ca.is_empty() {
        if ca == qa {
            score += 40;
        } else if ca.contains(&qa) || qa.contains(&ca) {
            score += 18;
        }
    }

    score
}

/// 指定来源的搜索预览；来源只影响候选展示，写回逻辑完全一致。
pub async fn preview_with_source(
    adapter: &dyn SourceAdapter,
    source: &str,
    title: &str,
    artist: &str,
    album: &str,
) -> AppResult<ScrapePreview> {
    let (query_title, query_artist) = split_name(title, artist);

    let keyword = if query_artist.is_empty() {
        query_title.clone()
    } else {
        format!("{query_artist} {query_title}")
    };

    let hits = adapter.search(&keyword, 0, 8).await?;

    let mut candidates: Vec<ScrapeCandidate> = hits
        .into_iter()
        .map(|hit| {
            let mut candidate = ScrapeCandidate {
                source: source.to_string(),
                id: hit.id,
                title: hit.title,
                artist: hit.artist,
                album: hit.album,
                cover_url: hit.cover_url,
                duration_ms: hit.duration_ms,
                score: 0,
            };
            candidate.score = score(&candidate, &query_title, &query_artist);
            candidate
        })
        .collect();

    candidates.sort_by(|a, b| b.score.cmp(&a.score));

    Ok(ScrapePreview {
        current: MetaView {
            title: title.to_string(),
            artist: artist.to_string(),
            album: album.to_string(),
            cover_url: None,
        },
        candidates,
    })
}

pub fn empty_preview(title: &str, artist: &str, album: &str) -> ScrapePreview {
    ScrapePreview {
        current: MetaView {
            title: title.to_string(),
            artist: artist.to_string(),
            album: album.to_string(),
            cover_url: None,
        },
        candidates: Vec::new(),
    }
}

/// 未登录/平台失败时的开源兜底：iTunes Search API。
pub async fn itunes_preview(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    album: &str,
) -> AppResult<ScrapePreview> {
    let (query_title, query_artist) = split_name(title, artist);
    let term = if query_artist.is_empty() {
        query_title.clone()
    } else {
        format!("{query_artist} {query_title}")
    };

    let mut url = reqwest::Url::parse("https://itunes.apple.com/search")
        .map_err(|err| AppError::Network(format!("iTunes URL 错误: {err}")))?;
    url.query_pairs_mut()
        .append_pair("term", &term)
        .append_pair("entity", "song")
        .append_pair("limit", "8")
        .append_pair("country", "CN");

    let body = client
        .get(url)
        .send()
        .await
        .map_err(|err| AppError::Network(format!("iTunes 搜索失败: {err}")))?
        .json::<serde_json::Value>()
        .await
        .map_err(|err| AppError::Network(format!("iTunes 响应解析失败: {err}")))?;

    let mut candidates = body["results"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            let title = item["trackName"].as_str()?.trim().to_string();
            if title.is_empty() {
                return None;
            }
            let artist = item["artistName"].as_str().unwrap_or("").to_string();
            let album = item["collectionName"].as_str().unwrap_or("").to_string();
            let cover = item["artworkUrl100"]
                .as_str()
                .map(|url| url.replace("100x100bb", "600x600bb"));

            let mut candidate = ScrapeCandidate {
                source: "itunes".to_string(),
                id: item["trackId"]
                    .as_i64()
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                title,
                artist,
                album,
                cover_url: cover,
                duration_ms: item["trackTimeMillis"].as_i64(),
                score: 0,
            };
            candidate.score = score(&candidate, &query_title, &query_artist);
            Some(candidate)
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|a, b| b.score.cmp(&a.score));

    Ok(ScrapePreview {
        current: MetaView {
            title: title.to_string(),
            artist: artist.to_string(),
            album: album.to_string(),
            cover_url: None,
        },
        candidates,
    })
}

/// 手动编辑：写入 Title/Artist/Album；传入封面时替换正面封面。
pub fn write_manual(
    path: &Path,
    title: &str,
    artist: &str,
    album: &str,
    cover: Option<Vec<u8>>,
    lyrics: Option<&str>,
) -> AppResult<()> {
    // 普通大小文件：先写同目录临时副本，再原子替换，绕开播放中的文件句柄/缓存
    let use_temp = std::fs::metadata(path)
        .map(|meta| meta.len() <= 64 * 1024 * 1024)
        .unwrap_or(false);

    let temp = if use_temp {
        let stem = path
            .file_stem()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| "track".to_string());
        let ext = path
            .extension()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let name = if ext.is_empty() {
            format!(".{stem}.sonotheca-tmp")
        } else {
            format!(".{stem}.sonotheca-tmp.{ext}")
        };
        let temp = path.with_file_name(name);
        let _ = std::fs::remove_file(&temp);
        std::fs::copy(path, &temp)?;
        Some(temp)
    } else {
        None
    };

    let target = temp.as_deref().unwrap_or(path);
    let cover_retry = cover.clone();
    let result = write_tags_to(target, title, artist, album, cover, lyrics);

    if let Err(err) = result {
        if let Some(temp) = temp {
            let _ = std::fs::remove_file(&temp);
        }
        return Err(err);
    }

    if let Some(temp) = temp {
        #[cfg(windows)]
        {
            let _ = std::fs::remove_file(path);
        }
        if let Err(rename_err) = std::fs::rename(&temp, path) {
            // Android FUSE / 某些外部存储不允许 rename 覆盖现有文件：回退为覆盖写入
            if std::fs::copy(&temp, path).is_err() {
                // 再兜底：直接对原文件写一次标签
                if let Err(write_err) =
                    write_tags_to(path, title, artist, album, cover_retry, lyrics)
                {
                    let _ = std::fs::remove_file(&temp);
                    return Err(AppError::Tag(format!(
                        "替换原文件失败: {rename_err}; 直接写入也失败: {write_err}"
                    )));
                }
            }
            let _ = std::fs::remove_file(&temp);
        }
    }

    // 落盘校验：确认标签真的写进了目标文件，而不是只改了内存
    verify_written(path, title, artist, album, lyrics)?;
    Ok(())
}

fn verify_written(
    path: &Path,
    title: &str,
    artist: &str,
    album: &str,
    lyrics: Option<&str>,
) -> AppResult<()> {
    let tagged = Probe::open(path)
        .map_err(|err| AppError::Tag(format!("写入校验打开失败: {err}")))?
        .read()
        .map_err(|err| AppError::Tag(format!("写入校验读取失败: {err}")))?;
    let tag = tagged
        .primary_tag()
        .ok_or_else(|| AppError::Tag("写入校验失败: 文件没有主标签".to_string()))?;

    if tag.title().as_deref().unwrap_or("") != title {
        return Err(AppError::Tag(format!(
            "写入校验失败: 标题仍为 {:?}",
            tag.title()
        )));
    }
    if tag.artist().as_deref().unwrap_or("") != artist {
        return Err(AppError::Tag(format!(
            "写入校验失败: 艺术家仍为 {:?}",
            tag.artist()
        )));
    }
    if tag.album().as_deref().unwrap_or("") != album {
        return Err(AppError::Tag(format!(
            "写入校验失败: 专辑仍为 {:?}",
            tag.album()
        )));
    }
    if let Some(expected) = lyrics {
        let actual = tag.get_string(&ItemKey::Lyrics).unwrap_or("");
        if actual.trim() != expected.trim() {
            return Err(AppError::Tag(
                "写入校验失败: 内嵌歌词未保存".to_string(),
            ));
        }
    }

    Ok(())
}

fn write_tags_to(
    path: &Path,
    title: &str,
    artist: &str,
    album: &str,
    cover: Option<Vec<u8>>,
    lyrics: Option<&str>,
) -> AppResult<()> {
    let mut tagged = Probe::open(path)
        .map_err(|err| AppError::NotAudio(err.to_string()))?
        .read()
        .map_err(|err| AppError::NotAudio(err.to_string()))?;

    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag_mut().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }

    if let Some(tag) = tagged.primary_tag_mut() {
        tag.set_title(title.to_string());
        tag.set_artist(artist.to_string());
        tag.set_album(album.to_string());

        if let Some(bytes) = cover {
            tag.remove_picture_type(PictureType::CoverFront);
            if let Ok(picture) = Picture::from_reader(&mut std::io::Cursor::new(bytes)) {
                tag.push_picture(picture);
            }
        }

        if let Some(text) = lyrics {
            if text.trim().is_empty() {
                tag.remove_key(&ItemKey::Lyrics);
            } else {
                // MP3 → USLT，FLAC → LYRICS，由 lofty 按容器自动映射
                tag.insert_text(ItemKey::Lyrics, text.to_string());
            }
        }
    }

    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|err| AppError::Tag(err.to_string()))?;

    Ok(())
}

/// 读取文件内嵌歌词（USLT / LYRICS）。
pub fn read_lyrics(path: &Path) -> AppResult<String> {
    let tagged = Probe::open(path)
        .map_err(|err| AppError::NotAudio(err.to_string()))?
        .read()
        .map_err(|err| AppError::NotAudio(err.to_string()))?;

    Ok(tagged
        .primary_tag()
        .and_then(|tag| tag.get_string(&ItemKey::Lyrics))
        .unwrap_or("")
        .to_string())
}

/// 真正写回：标签 + 封面。封面下载失败不致命，标签照样写。
pub async fn apply(
    client: &reqwest::Client,
    path: &Path,
    candidate: &ScrapeCandidate,
) -> AppResult<()> {    let mut cover: Option<Vec<u8>> = None;

    if let Some(url) = &candidate.cover_url {
        if let Ok(response) = client.get(url).send().await {
            if response.status().is_success() {
                if let Ok(bytes) = response.bytes().await {
                    if !bytes.is_empty() {
                        cover = Some(bytes.to_vec());
                    }
                }
            }
        }
    }

    let mut tagged = Probe::open(path)
        .map_err(|err| AppError::NotAudio(err.to_string()))?
        .read()
        .map_err(|err| AppError::NotAudio(err.to_string()))?;

    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag_mut().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }

    if let Some(tag) = tagged.primary_tag_mut() {
        tag.set_title(candidate.title.clone());
        tag.set_artist(candidate.artist.clone());
        if !candidate.album.is_empty() {
            tag.set_album(candidate.album.clone());
        }

        if let Some(bytes) = cover {
            if let Ok(picture) = Picture::from_reader(&mut std::io::Cursor::new(bytes)) {
                if tag.pictures().is_empty() {
                    tag.push_picture(picture);
                }
            }
        }
    }

    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|err| AppError::Tag(err.to_string()))?;

    Ok(())
}
