//! 本地流代理。
//!
//! 前端只拿到 `http://127.0.0.1:<port>/s/<token>`，真实上游 URL 与 Referer
//! 藏在票据里；票据 2 小时后失效。
//!
//! 刻意不引入 axum：只需要给 `<audio>` 转发 GET/HEAD + Range，
//! `std::net::TcpListener` + `reqwest::blocking` 足够，也省掉一整套 hyper 依赖。
//!
//! 安全前提：127.0.0.1 对设备上任何 App 都可达，所以 token 用两个 UUID
//! （244 bit 随机）且短 TTL。

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::header;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::provider::netease::PC_UA;

const TOKEN_TTL: Duration = Duration::from_secs(2 * 60 * 60);

struct Ticket {
    url: Option<String>,
    local_path: Option<PathBuf>,
    referer: Option<String>,
    expires: Instant,
}

pub struct ProxyHandle {
    pub port: u16,
    tickets: Arc<Mutex<HashMap<String, Ticket>>>,
}

impl ProxyHandle {
    /// 注册一条上游流，返回一次性 token
    pub fn add_stream(&self, url: String, referer: Option<String>) -> String {
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());

        if let Ok(mut map) = self.tickets.lock() {
            map.retain(|_, ticket| ticket.expires > Instant::now());
            map.insert(
                token.clone(),
                Ticket {
                    url: Some(url),
                    local_path: None,
                    referer,
                    expires: Instant::now() + TOKEN_TTL,
                },
            );
        }

        token
    }

    /// 注册本地文件，走 127.0.0.1 直出，避开 Android WebView 的 asset 限制。
    pub fn add_file(&self, path: PathBuf) -> String {
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());

        if let Ok(mut map) = self.tickets.lock() {
            map.retain(|_, ticket| ticket.expires > Instant::now());
            map.insert(
                token.clone(),
                Ticket {
                    url: None,
                    local_path: Some(path),
                    referer: None,
                    expires: Instant::now() + TOKEN_TTL,
                },
            );
        }

        token
    }

    pub fn stream_url(&self, token: &str) -> String {
        format!("http://127.0.0.1:{}/s/{}", self.port, token)
    }

    pub fn file_url(&self, token: &str) -> String {
        format!("http://127.0.0.1:{}/l/{}", self.port, token)
    }
}

/// 绑定 127.0.0.1:0（随机端口），后台线程 accept
pub fn spawn() -> AppResult<ProxyHandle> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|err| AppError::Network(format!("本地代理端口绑定失败: {err}")))?;
    let port = listener.local_addr()?.port();

    let tickets: Arc<Mutex<HashMap<String, Ticket>>> = Arc::new(Mutex::new(HashMap::new()));
    let shared = tickets.clone();

    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(stream) = incoming else { break };
            let tickets = shared.clone();
            std::thread::spawn(move || {
                let _ = serve(stream, tickets);
            });
        }
    });

    Ok(ProxyHandle { port, tickets })
}

fn serve(mut stream: TcpStream, tickets: Arc<Mutex<HashMap<String, Ticket>>>) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;

    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    // 只关心 Range；其余头忽略
    let mut range: Option<String> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("range") {
                range = Some(value.trim().to_string());
            }
        }
    }

    let (kind, token) = if let Some(token) = path.strip_prefix("/s/") {
        ("stream", token)
    } else if let Some(token) = path.strip_prefix("/l/") {
        ("local", token)
    } else {
        return write_status(&mut stream, 404, "not found");
    };

    let ticket = {
        let mut map = tickets.lock().map_err(|_| io::Error::other("ticket lock"))?;
        map.retain(|_, ticket| ticket.expires > Instant::now());
        map.get(token).map(|ticket| {
            (
                ticket.url.clone(),
                ticket.local_path.clone(),
                ticket.referer.clone(),
            )
        })
    };

    let Some((url, local_path, referer)) = ticket else {
        return write_status(&mut stream, 410, "stream ticket expired");
    };

    if kind == "local" {
        let Some(path) = local_path else {
            return write_status(&mut stream, 404, "local ticket missing");
        };
        return serve_local_file(&mut stream, &path, &method, range.as_deref());
    }

    let Some(url) = url else {
        return write_status(&mut stream, 404, "remote ticket missing");
    };

    let client = reqwest::blocking::Client::builder()
        .user_agent(PC_UA)
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(io::Error::other)?;

    let request_method = if method == "HEAD" {
        reqwest::Method::HEAD
    } else {
        reqwest::Method::GET
    };

    let mut request = client
        .request(request_method, &url)
        .header(
            header::REFERER,
            referer.unwrap_or_else(|| "https://music.163.com/".to_string()),
        );

    if let Some(range) = range {
        request = request.header(header::RANGE, range);
    }

    let response = request.send().map_err(io::Error::other)?;
    let status = response.status();

    let mut head = String::new();
    head.push_str(&format!(
        "HTTP/1.1 {} {}\r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or("OK")
    ));

    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
    ] {
        if let Some(value) = response.headers().get(&name).and_then(|v| v.to_str().ok()) {
            head.push_str(&format!("{}: {}\r\n", name.as_str(), value));
        }
    }

    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;

    if method != "HEAD" {
        let mut body = response;
        io::copy(&mut body, &mut stream)?;
    }

    Ok(())
}

fn serve_local_file(
    stream: &mut TcpStream,
    path: &Path,
    method: &str,
    range: Option<&str>,
) -> io::Result<()> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let (start, end, partial) = parse_range(range, len);

    let mime = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" => "audio/mp4",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "aac" => "audio/aac",
        _ => "application/octet-stream",
    };

    let length = end.saturating_sub(start);
    let mut head = String::new();
    head.push_str(if partial {
        "HTTP/1.1 206 Partial Content\r\n"
    } else {
        "HTTP/1.1 200 OK\r\n"
    });
    head.push_str(&format!("Content-Type: {mime}\r\n"));
    head.push_str(&format!("Content-Length: {length}\r\n"));
    head.push_str("Accept-Ranges: bytes\r\n");
    if partial {
        head.push_str(&format!(
            "Content-Range: bytes {start}-{}/{len}\r\n",
            end.saturating_sub(1)
        ));
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;

    if method != "HEAD" {
        file.seek(SeekFrom::Start(start))?;
        io::copy(&mut file.take(length), stream)?;
    }
    Ok(())
}

fn parse_range(range: Option<&str>, len: u64) -> (u64, u64, bool) {
    let Some(raw) = range.and_then(|value| value.trim().strip_prefix("bytes=")) else {
        return (0, len, false);
    };
    let Some((start, end)) = raw.split_once('-') else {
        return (0, len, false);
    };
    let Ok(start) = start.trim().parse::<u64>() else {
        return (0, len, false);
    };
    if start >= len {
        return (0, len, false);
    }
    let end = end
        .trim()
        .parse::<u64>()
        .ok()
        .map(|value| (value + 1).min(len))
        .unwrap_or(len);
    (start, end.max(start), true)
}

fn write_status(stream: &mut TcpStream, code: u16, message: &str) -> io::Result<()> {
    let body = message.as_bytes();
    let head = format!(
        "HTTP/1.1 {code} {message}\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)
}

#[cfg(test)]
mod tests {
    use super::spawn;

    fn get(url: &str, range: Option<&str>) -> reqwest::blocking::Response {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("client");

        let mut request = client.get(url);
        if let Some(range) = range {
            request = request.header(reqwest::header::RANGE, range);
        }

        request.send().expect("proxy request")
    }

    #[test]
    fn unknown_token_is_410() {
        let proxy = spawn().expect("spawn");
        let response = get(&proxy.stream_url("deadbeef"), None);
        assert_eq!(response.status().as_u16(), 410);
    }

    #[test]
    #[ignore]
    fn forwards_real_audio_with_range() {
        let proxy = spawn().expect("spawn");
        let token = proxy.add_stream(
            "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-1.mp3".to_string(),
            Some("https://www.soundhelix.com/".to_string()),
        );

        let response = get(&proxy.stream_url(&token), Some("bytes=0-15"));
        assert!(
            response.status().is_success() || response.status().as_u16() == 206,
            "status={}",
            response.status()
        );

        let head = response.bytes().expect("body");
        assert_ne!(
            crate::pipeline::magic::sniff_bytes(&head),
            crate::pipeline::magic::Container::Unknown,
            "head={head:?}"
        );
    }
}
