//! 统一认证层（M8）。
//!
//! 第一个实现是 B 站扫码：
//!   generate → 前端渲染二维码 → poll → 成功后抓 Set-Cookie / 跨域 URL 里的
//!   `SESSDATA` 等凭据，交给 ipc 落 settings 表。
//!
//! 后续 QQ 音乐（ptlogin2.qq.com）会继续实现同一个 [`AuthProvider`] trait，
//! 上层命令与 SourcesView 的面板不需要区分平台细节。

use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header;
use serde::Serialize;
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::pipeline::download::BROWSER_UA;

pub const COOKIE_KEY: &str = "bilibili_cookie";

const REFERER: &str = "https://www.bilibili.com/";
const QR_GENERATE: &str = "https://passport.bilibili.com/x/passport-login/web/qrcode/generate";
const QR_POLL: &str = "https://passport.bilibili.com/x/passport-login/web/qrcode/poll";
const NAV_API: &str = "https://api.bilibili.com/x/web-interface/nav";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);

/// 只保留登录/鉴权需要的 Cookie，避免把埋点 Cookie 也塞回请求头
const COOKIE_NAMES: &[&str] = &[
    "SESSDATA",
    "bili_jct",
    "DedeUserID",
    "DedeUserID__ckMd5",
    "sid",
    "buvid3",
    "buvid4",
    "b_nut",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QrLoginStart {
    /// 轮询用的 key
    pub key: String,
    /// 二维码内容（前端用 qrcode 库转成 data URL）
    pub url: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QrLoginState {
    /// B 站：0 成功 / 86101 待扫码 / 86090 已扫码待确认 / 86038 已过期
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BilibiliProfile {
    pub mid: i64,
    pub nickname: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BilibiliStatus {
    pub logged_in: bool,
    pub profile: Option<BilibiliProfile>,
}

/// 统一认证接口：命令层只依赖这个 trait，扫码面板也只看 QrLoginStart/QrLoginState。
#[async_trait]
pub trait AuthProvider: Send + Sync {
    async fn start_qr(&self) -> AppResult<QrLoginStart>;
    async fn poll_qr(&self, key: &str) -> AppResult<QrLoginState>;
    async fn logout(&self) -> AppResult<()>;
}

pub struct BilibiliAuth {
    client: reqwest::Client,
    cookie: RwLock<Option<String>>,
    /// generate 阶段拿到的 buvid 等 Cookie，poll 时带上，部分网络下可避免风控直断
    qr_cookie: RwLock<Option<String>>,
}

impl BilibiliAuth {
    pub fn new(client: reqwest::Client, cookie: Option<String>) -> Self {
        // 扫码链路单独建客户端：HTTP/1.1 + 不复用连接。
        // 实机反馈 passport 轮询会随机 error sending request，短连接 + 重试最稳。
        let auth_client = reqwest::Client::builder()
            .user_agent(BROWSER_UA)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(3))
            .pool_max_idle_per_host(0)
            .http1_only()
            .tcp_nodelay(true)
            .build()
            .unwrap_or(client);

        Self {
            client: auth_client,
            cookie: RwLock::new(cookie.filter(|value| !value.trim().is_empty())),
            qr_cookie: RwLock::new(None),
        }
    }

    pub fn cookie(&self) -> Option<String> {
        self.cookie.read().ok().and_then(|guard| guard.clone())
    }

    pub fn set_cookie(&self, value: Option<String>) {
        if let Ok(mut guard) = self.cookie.write() {
            *guard = value;
        }
    }

    /// GET JSON，最多 3 次；每次都是新连接（pool_max_idle_per_host=0）。
    /// 返回 body 与本次响应的 Set-Cookie。
    async fn get_json_with_retry(
        &self,
        url: &str,
        query: Option<(&str, &str)>,
        cookie: Option<&str>,
        label: &str,
    ) -> AppResult<(Value, BTreeMap<String, String>)> {
        let mut last_error: Option<AppError> = None;

        for attempt in 0..3 {
            let mut request = self
                .client
                .get(url)
                .header(header::REFERER, REFERER)
                .header(header::ORIGIN, "https://www.bilibili.com")
                .header(header::USER_AGENT, BROWSER_UA)
                .header(header::ACCEPT, "application/json, text/plain, */*")
                .header(header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
                .header(header::CONNECTION, "close")
                .timeout(REQUEST_TIMEOUT);

            if let Some((name, value)) = query {
                request = request.query(&[(name, value)]);
            }
            if let Some(cookie) = cookie {
                request = request.header(header::COOKIE, cookie);
            }

            match request.send().await {
                Ok(response) => {
                    let cookies = cookies_from_headers(&response);
                    match read_json(response, label).await {
                        Ok(body) => return Ok((body, cookies)),
                        Err(err) if attempt < 2 => {
                            last_error = Some(err);
                        }
                        Err(err) => return Err(err),
                    }
                }
                Err(err) => {
                    last_error = Some(AppError::Network(format!(
                        "{label}请求失败（第 {} 次）: {}",
                        attempt + 1,
                        error_chain(&err)
                    )));
                }
            }

            tokio::time::sleep(Duration::from_millis(350 * (attempt + 1))).await;
        }

        Err(last_error.unwrap_or_else(|| {
            AppError::Network(format!("{label}连续 3 次请求失败"))
        }))
    }

    /// 查询登录态 + 头像昵称。Cookie 失效时返回 loggedIn=false，不抛错。
    pub async fn status(&self) -> AppResult<BilibiliStatus> {
        let Some(cookie) = self.cookie() else {
            return Ok(BilibiliStatus {
                logged_in: false,
                profile: None,
            });
        };

        let (body, _) = self
            .get_json_with_retry(NAV_API, None, Some(&cookie), "B站账号状态")
            .await?;

        let code = body["code"].as_i64().unwrap_or(-1);
        if code != 0 || !body["data"]["isLogin"].as_bool().unwrap_or(false) {
            return Ok(BilibiliStatus {
                logged_in: false,
                profile: None,
            });
        }

        let data = &body["data"];
        Ok(BilibiliStatus {
            logged_in: true,
            profile: Some(BilibiliProfile {
                mid: data["mid"].as_i64().unwrap_or(0),
                nickname: data["uname"].as_str().unwrap_or("B站用户").to_string(),
                avatar_url: upgrade_https(data["face"].as_str().unwrap_or("")),
            }),
        })
    }
}

#[async_trait]
impl AuthProvider for BilibiliAuth {
    async fn start_qr(&self) -> AppResult<QrLoginStart> {
        let (body, cookies) = self
            .get_json_with_retry(QR_GENERATE, None, None, "B站二维码")
            .await?;

        // generate 可能下发 buvid3 / b_nut，poll 时带上更接近真实浏览器
        if !cookies.is_empty() {
            if let Ok(mut guard) = self.qr_cookie.write() {
                *guard = Some(cookie_header(&cookies));
            }
        }

        if body["code"].as_i64().unwrap_or(-1) != 0 {
            return Err(AppError::Network(format!(
                "B站二维码生成失败: {}",
                body["message"].as_str().unwrap_or("未知错误")
            )));
        }

        let key = body["data"]["qrcode_key"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::Network("B站未返回 qrcode_key".to_string()))?
            .to_string();

        let url = body["data"]["url"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::Network("B站未返回二维码内容".to_string()))?
            .to_string();

        Ok(QrLoginStart {
            key,
            url,
            expires_in: 180,
        })
    }

    async fn poll_qr(&self, key: &str) -> AppResult<QrLoginState> {
        let key = key.trim();
        if key.is_empty() {
            return Err(AppError::BadInput("缺少二维码 key".to_string()));
        }

        let qr_cookie = self.qr_cookie.read().ok().and_then(|guard| guard.clone());
        let (body, mut cookies) = self
            .get_json_with_retry(
                QR_POLL,
                Some(("qrcode_key", key)),
                qr_cookie.as_deref(),
                "B站扫码轮询",
            )
            .await?;

        if body["code"].as_i64().unwrap_or(-1) != 0 {
            return Err(AppError::Network(format!(
                "B站扫码轮询返回异常: {}",
                body["message"].as_str().unwrap_or("未知错误")
            )));
        }

        let data = &body["data"];
        let code = data["code"].as_i64().unwrap_or(-1);

        let message = if code == 0 {
            "登录成功"
        } else if code == 86101 {
            "等待扫码…"
        } else if code == 86090 {
            "已扫码，请在手机上确认"
        } else if code == 86038 {
            "二维码已过期，请重新获取"
        } else {
            data["message"].as_str().unwrap_or("扫码状态未知")
        }
        .to_string();

        if code == 0 {
            // 跨域登录 URL 的 query 里也带了一份 Cookie，作为 Set-Cookie 的补充
            if let Some(cross) = data["url"].as_str() {
                merge_cross_domain_cookies(&mut cookies, cross);
            }

            let cookie = cookie_header(&cookies);
            if !cookie.contains("SESSDATA=") {
                return Err(AppError::Network(
                    "B站登录成功但未拿到 SESSDATA，请重试".to_string(),
                ));
            }
            self.set_cookie(Some(cookie));
        }

        Ok(QrLoginState { code, message })
    }

    async fn logout(&self) -> AppResult<()> {
        self.set_cookie(None);
        Ok(())
    }
}

fn cookies_from_headers(response: &reqwest::Response) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();

    for value in response.headers().get_all(header::SET_COOKIE) {
        let Ok(raw) = value.to_str() else { continue };
        let Some(pair) = raw.split(';').next() else {
            continue;
        };
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };

        let name = name.trim();
        if COOKIE_NAMES.contains(&name) {
            out.insert(name.to_string(), value.trim().to_string());
        }
    }

    out
}

fn merge_cross_domain_cookies(out: &mut BTreeMap<String, String>, url: &str) {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return;
    };

    for (name, value) in parsed.query_pairs() {
        if COOKIE_NAMES.contains(&name.as_ref()) {
            out.insert(name.into_owned(), value.into_owned());
        }
    }
}

fn cookie_header(cookies: &BTreeMap<String, String>) -> String {
    cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn upgrade_https(url: &str) -> String {
    match url.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    }
}

async fn read_json(response: reqwest::Response, label: &str) -> AppResult<Value> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| AppError::Network(format!("{label}读取响应失败: {err}")))?;

    if !status.is_success() {
        return Err(AppError::Network(format!(
            "{label} HTTP {status}: {}",
            snippet(&text)
        )));
    }

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(AppError::Network(format!("{label}返回空响应")));
    }

    serde_json::from_str(trimmed).map_err(|err| {
        AppError::Network(format!(
            "{label} JSON 解析失败（{err}）: {}",
            snippet(trimmed)
        ))
    })
}

fn snippet(text: &str) -> String {
    text.chars()
        .take(200)
        .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
        .collect()
}

fn error_chain(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        out.push_str(" ← ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 需要外网，手动执行：
    /// `cargo test --lib provider::auth -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "需要外网：验证 B 站二维码生成 + 轮询"]
    async fn live_qr_start() {
        let client = reqwest::Client::builder()
            .user_agent(BROWSER_UA)
            .build()
            .expect("client");
        let auth = BilibiliAuth::new(client, None);
        let qr = auth.start_qr().await.expect("start_qr");

        assert!(!qr.key.is_empty());
        assert!(qr.url.contains("qrcode_key="), "url = {}", qr.url);

        // 未扫码时应返回 86101（等待扫码），证明 poll 链路可达
        let state = auth.poll_qr(&qr.key).await.expect("poll_qr");
        assert!(
            state.code == 86101 || state.code == 86038,
            "unexpected poll code {}",
            state.code
        );
    }
}
