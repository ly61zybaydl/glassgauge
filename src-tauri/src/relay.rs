//! 取数命令（spec §4.2）：语义不解析，把 /v1/limits 原文交给前端。
//!
//! 两条数据源，按序尝试：
//! 1. **上游直连** `https://relay.mirasim.ai/v1/limits`，Bearer = 本机 setting.json 里
//!    当前账号令牌解出的访问 JWT（与 mirasim 自己向上游取额度的方式一致，纯 Bearer，
//!    不需要设备签名）。mirasim 0.0.284 起本机 relay 改成「每个 agent 会话一个私密
//!    路径 + 会话令牌」，凭证不落盘，第三方无法再匿名访问本机 `/v1/limits`，因此上游
//!    直连成为主路径；顺带 `subject` 恒等于当前账号，切号后不再有 relay 追赶期。
//! 2. **本机 relay 发现**（旧版 mirasim 仍开放匿名 `/v1/limits`）：缓存端口 → 全量扫描。
//!
//! 全部失败时返回 Err 让前端进入降级态：`token-expired`（令牌已过期/被拒，本机也找不到
//! relay——需要打开 mirasim 让它刷新令牌）或 `relay-not-found`。

use crate::discovery;
use serde::Serialize;
use serde_json::Value;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::State;

pub const UPSTREAM: &str = "https://relay.mirasim.ai";

pub struct RelayState(pub Mutex<Option<u16>>);

impl Default for RelayState {
    fn default() -> Self {
        Self(Mutex::new(discovery::load_cached()))
    }
}

#[derive(Serialize)]
pub struct LimitsResult {
    pub json: String,
    pub endpoint: String,
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum UpstreamErr {
    /// 未登录 / 令牌解不出 → 上游这条路根本没走
    NoToken,
    /// 401/403 或本地判定 exp 已过 → 令牌失效
    Unauthorized,
    /// 网络 / 非 2xx / 响应形状不对
    Other,
}

/// 当前登录账号的访问 JWT（本机 DPAPI→AES 解出）；未登录或解不出为 None。
fn current_access_jwt() -> Option<String> {
    let home = crate::accounts::mirasim_home();
    let raw = std::fs::read_to_string(home.join("setting.json")).ok()?;
    let v: Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;
    let tok = v.get("auth")?.get("token")?.as_str()?;
    if tok.is_empty() {
        return None;
    }
    crate::token::access_jwt(&home, tok)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 上游客户端：公网地址，沿用系统代理设置；10s 超时。
fn upstream_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client")
}

/// 上游响应认领：来源可信，只要求有 `windows` 数组（可为空——如 unmetered 账号）。
fn looks_like_upstream(v: &Value) -> bool {
    v.get("windows").is_some_and(Value::is_array)
}

async fn upstream_limits(client: &reqwest::Client, base: &str, jwt: &str) -> Result<String, UpstreamErr> {
    let resp = client
        .get(format!("{}/v1/limits", base.trim_end_matches('/')))
        .bearer_auth(jwt)
        .send()
        .await
        .map_err(|_| UpstreamErr::Other)?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(UpstreamErr::Unauthorized);
    }
    if !status.is_success() {
        return Err(UpstreamErr::Other);
    }
    let text = resp.text().await.map_err(|_| UpstreamErr::Other)?;
    let v: Value = serde_json::from_str(&text).map_err(|_| UpstreamErr::Other)?;
    looks_like_upstream(&v).then_some(text).ok_or(UpstreamErr::Other)
}

/// 上游和本机都失败时给前端的错误码。
fn relay_error(upstream: UpstreamErr) -> &'static str {
    match upstream {
        UpstreamErr::Unauthorized => "token-expired",
        UpstreamErr::NoToken | UpstreamErr::Other => "relay-not-found",
    }
}

/// 取一次 limits：上游直连 → 缓存本机端口 → 全量重扫认领。
#[tauri::command]
pub async fn fetch_limits(state: State<'_, RelayState>) -> Result<LimitsResult, String> {
    // 1) 上游直连（当前账号令牌）
    let mut upstream = UpstreamErr::NoToken;
    let jwt = tauri::async_runtime::spawn_blocking(current_access_jwt)
        .await
        .ok()
        .flatten();
    if let Some(jwt) = jwt {
        // exp 已过（留 30s 时钟余量）就别白跑一趟网络
        if crate::token::jwt_exp(&jwt).is_some_and(|exp| exp + 30 < now_secs()) {
            upstream = UpstreamErr::Unauthorized;
        } else {
            match upstream_limits(&upstream_client(), UPSTREAM, &jwt).await {
                Ok(json) => {
                    return Ok(LimitsResult {
                        json,
                        endpoint: UPSTREAM.to_string(),
                    })
                }
                Err(e) => upstream = e,
            }
        }
    }

    // 2) 本机 relay：先试缓存端口，失败全量重扫认领（spec §4.1 的两级策略）
    let client = discovery::probe_client();
    let cached = *state.0.lock().unwrap();
    if let Some(port) = cached {
        if let Some(json) = discovery::probe(&client, port).await {
            return Ok(LimitsResult {
                json,
                endpoint: format!("http://127.0.0.1:{port}"),
            });
        }
    }
    match discovery::scan(&client).await {
        Some((port, json)) => {
            *state.0.lock().unwrap() = Some(port);
            discovery::save_cached(port);
            Ok(LimitsResult {
                json,
                endpoint: format!("http://127.0.0.1:{port}"),
            })
        }
        None => {
            *state.0.lock().unwrap() = None;
            Err(relay_error(upstream).into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const FIXTURE: &str = include_str!("../../ui/tests/fixtures/limits.json");

    /// 一次性假 HTTP 服务：记录请求原文，按给定状态/正文应答。
    fn fake_upstream(status: &'static str, body: &'static str) -> (u16, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
                let resp = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });
        (port, rx)
    }

    #[tokio::test]
    async fn upstream_sends_bearer_and_accepts_relay_shape() {
        let (port, rx) = fake_upstream("200 OK", FIXTURE);
        let got = upstream_limits(&upstream_client(), &format!("http://127.0.0.1:{port}"), "jwt.abc.def")
            .await
            .unwrap();
        assert_eq!(got, FIXTURE);
        let req = rx.recv().unwrap();
        assert!(req.starts_with("GET /v1/limits "), "{req}");
        assert!(req.to_ascii_lowercase().contains("authorization: bearer jwt.abc.def"), "{req}");
    }

    #[tokio::test]
    async fn upstream_401_is_unauthorized() {
        let (port, _rx) = fake_upstream("401 Unauthorized", r#"{"error":"expired"}"#);
        let err = upstream_limits(&upstream_client(), &format!("http://127.0.0.1:{port}"), "x")
            .await
            .unwrap_err();
        assert_eq!(err, UpstreamErr::Unauthorized);
    }

    #[tokio::test]
    async fn upstream_garbage_or_5xx_or_refused_is_other() {
        let (port, _rx) = fake_upstream("200 OK", r#"{"hello":"world"}"#);
        let err = upstream_limits(&upstream_client(), &format!("http://127.0.0.1:{port}"), "x")
            .await
            .unwrap_err();
        assert_eq!(err, UpstreamErr::Other);
        let (port, _rx) = fake_upstream("502 Bad Gateway", "");
        let err = upstream_limits(&upstream_client(), &format!("http://127.0.0.1:{port}"), "x")
            .await
            .unwrap_err();
        assert_eq!(err, UpstreamErr::Other);
        // 连接被拒（无监听）也是 Other，不是"令牌失效"
        let err = upstream_limits(&upstream_client(), "http://127.0.0.1:1", "x").await.unwrap_err();
        assert_eq!(err, UpstreamErr::Other);
    }

    #[test]
    fn upstream_shape_allows_empty_windows() {
        assert!(looks_like_upstream(&serde_json::json!({"subject":"u","windows":[]})));
        assert!(!looks_like_upstream(&serde_json::json!({"subject":"u"})));
        assert!(!looks_like_upstream(&serde_json::json!({"windows":"nope"})));
    }

    #[test]
    fn error_code_mapping() {
        assert_eq!(relay_error(UpstreamErr::Unauthorized), "token-expired");
        assert_eq!(relay_error(UpstreamErr::NoToken), "relay-not-found");
        assert_eq!(relay_error(UpstreamErr::Other), "relay-not-found");
    }
}
