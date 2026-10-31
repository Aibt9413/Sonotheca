//! 音频探测与标签写入（lofty）。
//!
//! 策略：**已有标签优先，缺什么补什么**。源站自带的元数据比我们从 URL 猜的靠谱，
//! 绝不覆盖；只有当 title/artist 缺失时才写入入队时的回退值。

use std::path::Path;

use lofty::config::WriteOptions;
use lofty::file::{FileType, TaggedFileExt};
use lofty::prelude::{Accessor, AudioFile};
use lofty::probe::Probe;
use lofty::tag::Tag;

use crate::error::{AppError, AppResult};

#[derive(Debug)]
pub struct AudioMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub ext: String,
    pub duration_ms: Option<u64>,
}

pub fn probe_and_tag(path: &Path, fallback_title: &str, fallback_artist: &str) -> AppResult<AudioMeta> {
    let mut tagged = Probe::open(path)
        .map_err(|e| AppError::NotAudio(e.to_string()))?
        .read()
        .map_err(|e| AppError::NotAudio(e.to_string()))?;

    let ext = extension_for(tagged.file_type());

    // 只取 duration，码率稍后用「文件大小 / 时长」算，避免依赖 lofty 版本的 bitrate API
    let duration_ms = {
        let d = tagged.properties().duration();
        let ms = d.as_millis() as u64;
        if ms > 0 {
            Some(ms)
        } else {
            None
        }
    };

    let mut title = tagged
        .primary_tag()
        .and_then(|tag| tag.title())
        .map(|v| v.to_string());
    let mut artist = tagged
        .primary_tag()
        .and_then(|tag| tag.artist())
        .map(|v| v.to_string());

    let need_title = title.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true);
    let need_artist = artist.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true);

    if need_title || need_artist {
        let tag_type = tagged.primary_tag_type();
        if tagged.primary_tag_mut().is_none() {
            tagged.insert_tag(Tag::new(tag_type));
        }
        if let Some(tag) = tagged.primary_tag_mut() {
            if need_title {
                tag.set_title(fallback_title.to_string());
            }
            if need_artist {
                tag.set_artist(fallback_artist.to_string());
            }
        }
        tagged
            .save_to_path(path, WriteOptions::default())
            .map_err(|e| AppError::Tag(e.to_string()))?;
    }

    if need_title {
        title = Some(fallback_title.to_string());
    }
    if need_artist {
        artist = Some(fallback_artist.to_string());
    }

    Ok(AudioMeta {
        title,
        artist,
        ext,
        duration_ms,
    })
}

fn extension_for(file_type: FileType) -> String {
    let ext = match file_type {
        FileType::Flac => "flac",
        FileType::Mpeg => "mp3",
        FileType::Mp4 => "m4a",
        FileType::Vorbis => "ogg",
        FileType::Opus => "opus",
        FileType::Wav => "wav",
        FileType::Aiff => "aiff",
        FileType::Ape => "ape",
        FileType::WavPack => "wv",
        FileType::Mpc => "mpc",
        FileType::Aac => "aac",
        FileType::Speex => "spx",
        // FileType 是 non_exhaustive，未来新增格式回落到 bin
        _ => "bin",
    };
    ext.to_string()
}

/// 当 lofty 认不出格式时，用 HTTP Content-Type 兜一层
pub fn extension_from_content_type(content_type: Option<&str>) -> Option<&'static str> {
    let content_type = content_type?;
    let ext = match content_type {
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => "m4a",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/aac" => "aac",
        _ => return None,
    };
    Some(ext)
}
