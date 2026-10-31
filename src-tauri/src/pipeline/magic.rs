//! 容器魔数嗅探：以字节为准，不信文件后缀。
//!
//! 为什么必须做：网易云偶尔给出 `.mp3` 后缀但内容其实是 FLAC 的地址，
//! 直接按后缀归档会让播放器用错解码器；反过来，后缀写 `.flac` 的 128K
//! MP3 也会让部分播放器卡住。这里只在归档前读一次头部，成本可忽略。
//!
//! 判定只看容器签名；Ogg 外壳里的 Opus 按内层编码归到 [`Container::Opus`]。

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// 需要读多少字节。Ogg 的编码识别头在第 28 字节之后，16 字节不够。
const HEAD_LEN: usize = 64;

/// 我们认识的音频容器。`Unknown` 交给上层（lofty / Content-Type）兜底。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Flac,
    Mp3,
    Ogg,
    Opus,
    Wav,
    M4a,
    Wma,
    Ape,
    /// 网易云加密容器，需要走 ncmdump
    Ncm,
    Unknown,
}

impl Container {
    pub fn extension(self) -> Option<&'static str> {
        match self {
            Self::Flac => Some("flac"),
            Self::Mp3 => Some("mp3"),
            Self::Ogg => Some("ogg"),
            Self::Opus => Some("opus"),
            Self::Wav => Some("wav"),
            Self::M4a => Some("m4a"),
            Self::Wma => Some("wma"),
            Self::Ape => Some("ape"),
            Self::Ncm | Self::Unknown => None,
        }
    }

    pub fn is_encrypted(self) -> bool {
        matches!(self, Self::Ncm)
    }
}

/// 只读文件头，开销可忽略
pub fn sniff(path: &Path) -> Container {
    let Ok(file) = File::open(path) else {
        return Container::Unknown;
    };

    // 用 take 限读，避免把小文件读成空、也不受"单次 read 可能短读"影响
    let mut head = Vec::with_capacity(HEAD_LEN);
    if file.take(HEAD_LEN as u64).read_to_end(&mut head).is_err() {
        return Container::Unknown;
    }

    sniff_bytes(&head)
}

/// 真正的判定逻辑。单独拆出来，测试不需要临时文件。
pub fn sniff_bytes(head: &[u8]) -> Container {
    if head.len() < 4 {
        return Container::Unknown;
    }

    if head.starts_with(b"fLaC") {
        return Container::Flac;
    }
    if head.starts_with(b"ID3") {
        return Container::Mp3;
    }
    // MPEG 帧同步：11 个 1
    if head[0] == 0xFF && (head[1] & 0xE0) == 0xE0 {
        return Container::Mp3;
    }
    if head.starts_with(b"OggS") {
        if head.get(28..36) == Some(&b"OpusHead"[..]) {
            return Container::Opus;
        }
        return Container::Ogg;
    }
    if head.starts_with(b"RIFF") && head.get(8..12) == Some(&b"WAVE"[..]) {
        return Container::Wav;
    }
    // ISO-BMFF：前 4 字节是 box 长度，紧接着 ftyp
    if head.get(4..8) == Some(&b"ftyp"[..]) {
        return Container::M4a;
    }
    // ASF (WMA) 文件头 GUID 的前 4 字节
    if head.starts_with(&[0x30, 0x26, 0xB2, 0x75]) {
        return Container::Wma;
    }
    if head.starts_with(b"MAC ") {
        return Container::Ape;
    }
    if head.starts_with(b"CTENFDAM") {
        return Container::Ncm;
    }
    // 少数 NCM 变体的魔数
    if head.starts_with(b"USAC") {
        return Container::Ncm;
    }

    Container::Unknown
}

/// 真实扩展名；加密容器给出明确错误而不是 panic。
///
/// 预留给扫描 / 导入前的校验入口：下载流水线自己 `sniff` 后逐级降级，
/// 不能用 `Unknown` 把还能播的文件判死。
#[allow(dead_code)]
pub fn resolve_extension(path: &Path) -> crate::error::AppResult<&'static str> {
    let container = sniff(path);

    if container.is_encrypted() {
        return Err(crate::error::AppError::EncryptedFormat(
            "下载内容为网易云加密容器（NCM），需要解密后再归档".to_string(),
        ));
    }

    container
        .extension()
        .ok_or_else(|| crate::error::AppError::NotAudio("无法识别的音频容器".to_string()))
}

#[cfg(test)]
mod tests {
    use super::{sniff_bytes, Container};

    #[test]
    fn sniffs_magic_fragments() {
        assert_eq!(sniff_bytes(b"fLaC\x00\x00\x00\x22"), Container::Flac);
        assert_eq!(sniff_bytes(b"ID3\x04\x00\x00\x00\x00\x00\x21"), Container::Mp3);
        assert_eq!(sniff_bytes(&[0xFF, 0xFB, 0x90, 0x64]), Container::Mp3);
        assert_eq!(sniff_bytes(b"OggS\x00\x02\x00\x00"), Container::Ogg);
        assert_eq!(sniff_bytes(b"RIFF\x24\x08\x00\x00WAVEfmt "), Container::Wav);
        assert_eq!(sniff_bytes(b"\x00\x00\x00\x20ftypM4A "), Container::M4a);
        assert_eq!(
            sniff_bytes(&[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11]),
            Container::Wma
        );
        assert_eq!(sniff_bytes(b"MAC \x96\x0F\x00\x00"), Container::Ape);
    }

    #[test]
    fn distinguishes_ogg_opus() {
        let mut page = vec![0u8; 40];
        page[..4].copy_from_slice(b"OggS");
        page[28..36].copy_from_slice(b"OpusHead");
        assert_eq!(sniff_bytes(&page), Container::Opus);
    }

    #[test]
    fn flags_ncm() {
        assert_eq!(sniff_bytes(b"CTENFDAM\x01\x70"), Container::Ncm);
        assert_eq!(sniff_bytes(b"USAC\x00\x00\x00\x00"), Container::Ncm);
        assert!(Container::Ncm.is_encrypted());
        assert_eq!(Container::Ncm.extension(), None);
    }

    #[test]
    fn unknown_and_short_inputs() {
        assert_eq!(sniff_bytes(b""), Container::Unknown);
        assert_eq!(sniff_bytes(b"fL"), Container::Unknown);
        assert_eq!(sniff_bytes(b"\x00\x01\x02\x03\x04\x05"), Container::Unknown);
        assert_eq!(Container::Unknown.extension(), None);
    }
}
