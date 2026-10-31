//! NCM 容器解密（网易云加密格式）。
//!
//! 用 `ncmdump` crate 实现算法，这里只做三件事：
//!   1. 用 magic `CTENFDAM` 判断是不是 NCM（下载前不知道扩展名，只能看魔数）
//!   2. 解出裸音频 + 容器内嵌的元数据（歌名/歌手/专辑/格式）
//!   3. 返回给流水线，由它落盘后继续走 lofty 写标签 → rename 归档
//!
//! 注意：`Ncmdump::get_data()` 依赖 reader 游标位置，而 `get_info()` 会 seek。
//! 所以这里每次都用**独立的 reader**重新解析，避免顺序依赖。

use std::fs::File;
use std::io::Read;
use std::path::Path;

use ncmdump::Ncmdump;

use crate::error::{AppError, AppResult};

const MAGIC: &[u8; 8] = b"CTENFDAM";

#[derive(Debug)]
pub struct NcmContent {
    pub audio: Vec<u8>,
    pub title: Option<String>,
    pub artist: Option<String>,
}

/// 只看 8 字节魔数，不整文件读入
pub fn is_ncm(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 8];
    if file.read_exact(&mut magic).is_err() {
        return false;
    }
    &magic == MAGIC
}

pub fn decrypt(path: &Path) -> AppResult<NcmContent> {
    let info = read_info(path);

    let audio = {
        let file = File::open(path)?;
        let mut dump = Ncmdump::from_reader(file)
            .map_err(|err| AppError::Decrypt(format!("NCM 头解析失败: {err}")))?;
        dump.get_data()
            .map_err(|err| AppError::Decrypt(format!("NCM 音频解密失败: {err}")))?
    };

    if audio.is_empty() {
        return Err(AppError::Decrypt("解密结果为空".to_string()));
    }

    let (title, artist) = match info {
        Some(info) => {
            let artist = info
                .artist
                .iter()
                .map(|(name, _)| name.clone())
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>()
                .join("/");
            (
                Some(info.name).filter(|v| !v.is_empty()),
                Some(artist).filter(|v| !v.is_empty()),
            )
        }
        None => (None, None),
    };

    Ok(NcmContent { audio, title, artist })
}

fn read_info(path: &Path) -> Option<ncmdump::NcmInfo> {
    let file = File::open(path).ok()?;
    let mut dump = Ncmdump::from_reader(file).ok()?;
    dump.get_info().ok()
}
