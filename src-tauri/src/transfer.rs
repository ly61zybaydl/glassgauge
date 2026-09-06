//! 凭证导出 / 导入 / 刷新（v0.15.0）。
//!
//! - **导出**：明文 JSON，内容就是两枚 JWT（access + refresh）加邮箱/套餐等元数据。这是用户
//!   明确选择的形态——拿到文件的人即可登录该账号，文件请自行保管。
//! - **导入**：读 JSON（兼容本工具导出格式、`/auth/verify` 原始响应、setting.json auth 块三种
//!   键名）→ 校验两枚 JWT → 联网刷新一次（验活 + 换到新 pair；网络不通则原样入库并提示）→
//!   用本机 secret.key 封成 mrs1 → 落成快照（同 userId 覆盖，一账号一快照）。不自动切换登录。
//! - **刷新**：`POST https://auth.mirasim.ai/auth/refresh {refresh_token}`（与 mirasim 自身完全
//!   一致：无签名、无设备头）→ 新 pair 写回。目标是当前登录时改 setting.json 的 `auth`
//!   三个字段；目标是快照时重写快照记录。实测（2026-09-07）服务端对同一枚 refresh 幂等返回
//!   同一子代 pair，旧 refresh / 旧 access 轮换后仍有效，所以刷新对别的机器没有副作用。
//!
//! 令牌只在本进程内流转；invoke 只回路径、邮箱、快照名等元数据。文件对话框（rfd）也放在
//! Rust 侧，前端连文件路径都不需要先拿到。

use crate::accounts::{self, AccountsLock, AccountsView, Store};
use crate::token;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::State;

pub const AUTH_BASE: &str = "https://auth.mirasim.ai";
pub const EXPORT_FORMAT: &str = "mirasim-account-export";
/// 用户在文件对话框点了取消：前端据此静默，不当错误显示。
pub const CANCELLED: &str = "cancelled";

#[derive(Serialize)]
pub struct ExportResult {
    pub path: String,
    pub email: Option<String>,
    /// "当前登录" 或 "快照「x」"
    pub target: String,
}

#[derive(Serialize)]
pub struct ImportResult {
    pub name: String,
    pub email: Option<String>,
    /// 是否已联网刷新验活
    pub refreshed: bool,
    /// 未能联网验证等提示
    pub note: Option<String>,
    pub view: AccountsView,
}

#[derive(Serialize)]
pub struct RefreshResult {
    pub target: String,
    #[serde(rename = "accessExp")]
    pub access_exp: i64,
    pub view: AccountsView,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 2026-09-07T12:34:56Z（复用 backup_stamp 的手算 civil date）
fn iso_now() -> String {
    let s = accounts::backup_stamp(); // YYYYMMDD-HHMMSS
    format!("{}-{}-{}T{}:{}:{}Z", &s[0..4], &s[4..6], &s[6..8], &s[9..11], &s[11..13], &s[13..15])
}

/// 目标：None = 当前登录（setting.json 的 auth），Some(name) = 某个快照。返回 (auth 块, 展示名)。
fn target_auth(store: &Store, name: Option<&str>) -> Result<(Value, String), String> {
    match name {
        None => {
            let setting = store.load_setting()?;
            let auth = setting.get("auth").cloned().unwrap_or(Value::Null);
            if !accounts::has_login(&auth) {
                return Err("当前没有登录".into());
            }
            Ok((auth, "当前登录".into()))
        }
        Some(n) => {
            let (_, rec, _) = store
                .profiles()
                .into_iter()
                .find(|(pn, ..)| pn == n)
                .ok_or_else(|| format!("找不到快照「{n}」"))?;
            Ok((rec.get("auth").cloned().unwrap_or(Value::Null), format!("快照「{n}」")))
        }
    }
}

/// 解出 auth 块里的两枚 JWT 明文。
fn plain_pair(home: &Path, auth: &Value) -> Result<(String, String), String> {
    let get = |k: &str| auth.get(k).and_then(Value::as_str).unwrap_or("");
    let access = token::access_jwt(home, get("token"))
        .ok_or("解不出 access 令牌（secret.key 不可读，或密文来自别的机器）")?;
    let refresh = token::access_jwt(home, get("refreshToken")).ok_or("解不出 refresh 令牌")?;
    Ok((access, refresh))
}

fn claim<'a>(c: &'a Value, k: &str) -> Option<&'a str> {
    c.get(k).and_then(Value::as_str)
}

/* ---------- 导出格式 ---------- */

/// 导出文件内容（明文）。
pub fn export_json(access: &str, refresh: &str, exported_at: &str) -> Value {
    let a = token::jwt_claims(access).unwrap_or(Value::Null);
    let r = token::jwt_claims(refresh).unwrap_or(Value::Null);
    let pick = |v: &Value, k: &str| v.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "format": EXPORT_FORMAT,
        "version": 1,
        "exportedAt": exported_at,
        "userId": pick(&a, "sub"),
        "email": pick(&a, "email"),
        "plan": pick(&a, "plan"),
        "planExp": pick(&a, "plan_exp"),
        "accessToken": access,
        "accessExp": pick(&a, "exp"),
        "refreshToken": refresh,
        "refreshExp": pick(&r, "exp"),
        "note": "明文凭证：拿到此文件即可登录该账号，请自行保管。导入：glassgauge 账号面板「导入」。refresh 约 30 天有效；导入方与导出方各自刷新互不影响。",
    })
}

/// 从导入 JSON 取两枚 JWT 明文。兼容三种键名：本工具导出（accessToken/refreshToken）、
/// `/auth/verify` 原始响应（access_token/refresh_token）、setting.json auth 块（token/refreshToken）。
/// 值若是 `mrs1:` 密文，尝试用本机密钥解（同机快照文件）；解不开就是别的机器的密文，报错。
pub fn parse_import(v: &Value, home: &Path) -> Result<(String, String), String> {
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| v.get(*k).and_then(Value::as_str))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let access = pick(&["accessToken", "access_token", "token"]).ok_or("文件里没有 access 令牌字段")?;
    let refresh = pick(&["refreshToken", "refresh_token"]).ok_or("文件里没有 refresh 令牌字段")?;
    let unwrap = |s: String, what: &str| -> Result<String, String> {
        if s.starts_with("mrs1:") {
            token::access_jwt(home, &s).ok_or_else(|| {
                format!("{what} 是 mrs1 密文且本机解不开——它绑定别的机器，请在源机用「导出」生成明文文件")
            })
        } else {
            Ok(s)
        }
    };
    let access = unwrap(access, "access")?;
    let refresh = unwrap(refresh, "refresh")?;
    let ac = token::jwt_claims(&access).ok_or("access 不是合法 JWT")?;
    let rc = token::jwt_claims(&refresh).ok_or("refresh 不是合法 JWT")?;
    if claim(&rc, "token_type").is_some_and(|t| t != "refresh") {
        return Err("refreshToken 字段里放的不是 refresh 令牌".into());
    }
    if claim(&ac, "sub").is_none() {
        return Err("access 令牌缺少 sub（账号 ID）".into());
    }
    if ac.get("sub") != rc.get("sub") {
        return Err("两枚令牌不属于同一账号".into());
    }
    if rc.get("exp").and_then(Value::as_i64).is_some_and(|e| e < now_secs()) {
        return Err("refresh 令牌已过期（30 天），请在源机重新登录后再导出".into());
    }
    Ok((access, refresh))
}

/// 由两枚明文 JWT 组装 setting.json 风格的 auth 块（密文）。返回 (auth, access exp)。
fn sealed_auth(home: &Path, access: &str, refresh: &str, name: &str) -> Result<(Value, i64), String> {
    let ac = token::jwt_claims(access).ok_or("access 不是合法 JWT")?;
    let sub = claim(&ac, "sub").ok_or("access 缺少 sub")?;
    let exp = ac.get("exp").and_then(Value::as_i64).unwrap_or(0);
    let token = token::seal(home, access).ok_or("本机 secret.key 不可用，无法加密入库")?;
    let refresh_token = token::seal(home, refresh).ok_or("本机 secret.key 不可用，无法加密入库")?;
    Ok((
        json!({ "token": token, "userId": sub, "exp": exp, "refreshToken": refresh_token, "name": name }),
        exp,
    ))
}

/* ---------- /auth/refresh ---------- */

#[derive(Debug, PartialEq)]
pub enum RefreshErr {
    /// 非 200：状态码 + 正文片段
    Rejected(u16, String),
    Network(String),
    BadResponse,
}

impl RefreshErr {
    pub fn message(&self) -> String {
        match self {
            RefreshErr::Rejected(400 | 401 | 403, _) => "服务端拒绝：refresh 令牌已失效或被吊销，需要重新登录".into(),
            RefreshErr::Rejected(code, body) => format!("服务端 HTTP {code}：{body}"),
            RefreshErr::Network(e) => format!("网络不可达：{e}"),
            RefreshErr::BadResponse => "服务端响应缺少 access_token".into(),
        }
    }
}

/// 公网地址，沿用系统代理；15s 超时。
fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("reqwest client")
}

/// 用 refresh 换一对新令牌。服务端不下发新 refresh 时沿用旧的（与 mirasim 客户端一致）。
pub async fn refresh_pair(client: &reqwest::Client, base: &str, refresh: &str) -> Result<(String, String), RefreshErr> {
    let resp = client
        .post(format!("{}/auth/refresh", base.trim_end_matches('/')))
        .json(&json!({ "refresh_token": refresh }))
        .send()
        .await
        .map_err(|e| RefreshErr::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(|e| RefreshErr::Network(e.to_string()))?;
    if status != 200 {
        return Err(RefreshErr::Rejected(status, text.chars().take(120).collect()));
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| RefreshErr::BadResponse)?;
    let access = claim(&v, "access_token").filter(|s| !s.is_empty()).ok_or(RefreshErr::BadResponse)?;
    let new_refresh = claim(&v, "refresh_token").filter(|s| !s.is_empty()).unwrap_or(refresh);
    Ok((access.to_string(), new_refresh.to_string()))
}

/* ---------- 落库 ---------- */

/// 新 pair 写回目标。当前登录：重读 setting.json 只改 auth 的 token/refreshToken/exp（保留 name 等），
/// 且校验 userId 未变（用户刷新期间切了号就放弃）；快照：整条记录重写。返回 access exp。
fn write_back(store: &Store, name: Option<&str>, access: &str, refresh: &str) -> Result<i64, String> {
    match name {
        None => {
            let mut setting = store.load_setting()?;
            let cur_uid = setting["auth"].get("userId").and_then(Value::as_str).unwrap_or("").to_string();
            let cur_name = setting["auth"].get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let (sealed, exp) = sealed_auth(store.home(), access, refresh, &cur_name)?;
            if !cur_uid.is_empty() && sealed["userId"].as_str() != Some(cur_uid.as_str()) {
                return Err("当前登录账号已变，放弃写回".into());
            }
            for k in ["token", "refreshToken", "exp"] {
                setting["auth"][k] = sealed[k].clone();
            }
            accounts::write_json_atomic(&store.setting_path(), &setting)?;
            Ok(exp)
        }
        Some(n) => {
            let (_, rec, _) = store
                .profiles()
                .into_iter()
                .find(|(pn, ..)| pn == n)
                .ok_or_else(|| format!("找不到快照「{n}」"))?;
            let keep_name = rec["auth"].get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let (sealed, exp) = sealed_auth(store.home(), access, refresh, &keep_name)?;
            store.write_profile(n, &sealed)?;
            Ok(exp)
        }
    }
}

/// 导入落库：同 userId 覆盖原快照名，否则按邮箱本地部分自动命名（撞名自动消歧）。
pub fn store_import(store: &Store, access: &str, refresh: &str) -> Result<(String, Option<String>), String> {
    let ac = token::jwt_claims(access).ok_or("access 不是合法 JWT")?;
    let sub = claim(&ac, "sub").ok_or("access 缺少 sub")?.to_string();
    let email = claim(&ac, "email").map(str::to_string);
    let profiles = store.profiles();
    let name = match profiles
        .iter()
        .find(|(_, rec, _)| rec.get("userId").and_then(Value::as_str) == Some(sub.as_str()))
    {
        Some((n, ..)) => n.clone(),
        None => {
            let base = email
                .as_deref()
                .map(|e| accounts::sanitize_name(token::local_part(e)))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("usr-{}", accounts::short_id(&sub)));
            accounts::unique_auto_name(&base, &sub, email.as_deref(), &profiles)
        }
    };
    let display = email.as_deref().map(token::local_part).unwrap_or("").to_string();
    let (sealed, _) = sealed_auth(store.home(), access, refresh, &display)?;
    store.write_profile(&name, &sealed)?;
    Ok((name, email))
}

/* ---------- 可测的核心流程（命令只负责弹对话框 + 组 Store） ---------- */

/// 导出到指定路径。
pub fn export_to(store: &Store, name: Option<&str>, path: &Path) -> Result<ExportResult, String> {
    let (auth, target) = target_auth(store, name)?;
    let (access, refresh) = plain_pair(store.home(), &auth)?;
    let email = token::jwt_claims(&access).and_then(|c| claim(&c, "email").map(str::to_string));
    let doc = export_json(&access, &refresh, &iso_now());
    std::fs::write(path, serde_json::to_string_pretty(&doc).unwrap()).map_err(|e| format!("写入失败：{e}"))?;
    Ok(ExportResult {
        path: path.display().to_string(),
        email,
        target,
    })
}

/// 导出文件的默认文件名：mirasim-<邮箱本地部分>-<YYYYMMDD>.json
pub fn export_filename(store: &Store, name: Option<&str>) -> String {
    let local = target_auth(store, name)
        .ok()
        .and_then(|(auth, _)| store.email_of(&auth))
        .map(|e| accounts::sanitize_name(token::local_part(&e)))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "account".into());
    format!("mirasim-{local}-{}.json", &accounts::backup_stamp()[..8])
}

/// 从文件导入。`online` 给出登录服务器地址时先联网刷新一次验活；None 表示离线入库。
pub async fn import_file(
    store: &Store,
    lock: &AccountsLock,
    path: &Path,
    online: Option<&str>,
) -> Result<ImportResult, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("读取失败：{e}"))?;
    let v: Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).map_err(|e| format!("不是合法 JSON：{e}"))?;
    let (mut access, mut refresh) = parse_import(&v, store.home())?;
    let mut refreshed = false;
    let mut note = None;
    if let Some(base) = online {
        match refresh_pair(&http(), base, &refresh).await {
            Ok((a, r)) => {
                access = a;
                refresh = r;
                refreshed = true;
            }
            Err(e @ RefreshErr::Rejected(..)) => return Err(format!("凭证无效：{}", e.message())),
            Err(e) => note = Some(format!("未能联网验证（{}），已按原样入库", e.message())),
        }
    }
    // 落库阶段串行化；此后没有 await，锁不跨越异步边界
    let _g = lock.0.lock().unwrap();
    let (name, email) = store_import(store, &access, &refresh)?;
    let view = store.view()?;
    Ok(ImportResult {
        name,
        email,
        refreshed,
        note,
        view,
    })
}

/// 刷新目标（None = 当前登录）的令牌并写回。
pub async fn refresh_target(
    store: &Store,
    lock: &AccountsLock,
    name: Option<&str>,
    base: &str,
) -> Result<RefreshResult, String> {
    let (auth, target) = target_auth(store, name)?;
    let (_, refresh) = plain_pair(store.home(), &auth)?;
    let (access, new_refresh) = refresh_pair(&http(), base, &refresh).await.map_err(|e| e.message())?;
    let _g = lock.0.lock().unwrap();
    let access_exp = write_back(store, name, &access, &new_refresh)?;
    Ok(RefreshResult {
        target,
        access_exp,
        view: store.view()?,
    })
}

/* ---------- tauri 命令 ---------- */

#[tauri::command]
pub async fn accounts_export(name: Option<String>) -> Result<ExportResult, String> {
    let store = Store::new(accounts::mirasim_home());
    // 先确认目标可解，再弹对话框——别让用户选完路径才发现解不出
    let (auth, _) = target_auth(&store, name.as_deref())?;
    plain_pair(store.home(), &auth)?;
    let fname = export_filename(&store, name.as_deref());
    let picked = tauri::async_runtime::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_title("导出 mirasim 账号凭证（明文 JSON）")
            .set_file_name(&fname)
            .add_filter("JSON", &["json"])
            .save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked else {
        return Err(CANCELLED.into());
    };
    export_to(&store, name.as_deref(), &path)
}

#[tauri::command]
pub async fn accounts_import(lock: State<'_, AccountsLock>) -> Result<ImportResult, String> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("导入 mirasim 账号凭证")
            .add_filter("JSON", &["json"])
            .pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked else {
        return Err(CANCELLED.into());
    };
    let store = Store::new(accounts::mirasim_home());
    import_file(&store, &lock, &path, Some(AUTH_BASE)).await
}

#[tauri::command]
pub async fn accounts_refresh(lock: State<'_, AccountsLock>, name: Option<String>) -> Result<RefreshResult, String> {
    let store = Store::new(accounts::mirasim_home());
    refresh_target(&store, &lock, name.as_deref(), AUTH_BASE).await
}

/* ---------- 单测 ---------- */

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::token::testkit::{install_sandbox_key, jwt};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const KEY: [u8; 32] = [77u8; 32];
    const FAR: i64 = 4_000_000_000; // 2096 年，永不过期

    fn sandbox(tag: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("gg-xfer-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        install_sandbox_key(&dir, KEY);
        Store::new(dir)
    }

    fn login(store: &Store, sub: &str, email: &str) {
        let auth = json!({
            "token": token::encrypt_token(&jwt(sub, email, "access", FAR), &KEY).unwrap(),
            "refreshToken": token::encrypt_token(&jwt(sub, email, "refresh", FAR + 100), &KEY).unwrap(),
            "userId": sub, "exp": FAR, "name": "Some One",
        });
        std::fs::write(
            store.setting_path(),
            serde_json::to_string_pretty(&json!({ "auth": auth, "failover": { "enabled": true } })).unwrap(),
        )
        .unwrap();
    }

    /// 一次性假登录服务：记录请求，按给定状态/正文应答。
    fn fake_auth(status: &'static str, body: String) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = vec![0u8; 16384];
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
        (format!("http://127.0.0.1:{port}"), rx)
    }

    #[test]
    fn export_then_parse_roundtrip() {
        let s = sandbox("export");
        login(&s, "usr_a1", "ann.lee@example.com");
        let out = s.home().join("out.json");
        let r = export_to(&s, None, &out).unwrap();
        assert_eq!(r.email.as_deref(), Some("ann.lee@example.com"));
        assert_eq!(r.target, "当前登录");
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(doc["format"], EXPORT_FORMAT);
        assert_eq!(doc["userId"], "usr_a1");
        assert_eq!(doc["plan"], "plus");
        assert!(doc["accessToken"].as_str().unwrap().split('.').count() == 3, "明文 JWT");
        assert!(doc["refreshToken"].as_str().unwrap().split('.').count() == 3);
        // 导出的文件能被 parse_import 原样吃回
        let (a, r2) = parse_import(&doc, s.home()).unwrap();
        assert_eq!(a, doc["accessToken"]);
        assert_eq!(r2, doc["refreshToken"]);
        assert!(export_filename(&s, None).starts_with("mirasim-ann.lee-"));
    }

    #[test]
    fn parse_import_accepts_three_key_styles_and_rejects_bad_input() {
        let s = sandbox("parse");
        let a = jwt("usr_x", "x@example.com", "access", FAR);
        let r = jwt("usr_x", "x@example.com", "refresh", FAR);
        assert!(parse_import(&json!({ "accessToken": a, "refreshToken": r }), s.home()).is_ok());
        assert!(parse_import(&json!({ "access_token": a, "refresh_token": r }), s.home()).is_ok());
        // setting.json auth 块：本机 mrs1 密文也能解
        let sealed = json!({
            "token": token::encrypt_token(&a, &KEY).unwrap(),
            "refreshToken": token::encrypt_token(&r, &KEY).unwrap(),
        });
        assert_eq!(parse_import(&sealed, s.home()).unwrap().0, a);
        // 别的机器的密文：解不开 → 明确报错
        let foreign = json!({ "token": token::encrypt_token(&a, &[1u8; 32]).unwrap(), "refreshToken": r });
        assert!(parse_import(&foreign, s.home()).unwrap_err().contains("别的机器"));
        // 缺字段 / 不同账号 / refresh 过期 / 放错令牌
        assert!(parse_import(&json!({ "accessToken": a }), s.home()).is_err());
        let other = jwt("usr_y", "y@example.com", "refresh", FAR);
        assert!(parse_import(&json!({ "accessToken": a, "refreshToken": other }), s.home()).unwrap_err().contains("同一账号"));
        let expired = jwt("usr_x", "x@example.com", "refresh", 1_000);
        assert!(parse_import(&json!({ "accessToken": a, "refreshToken": expired }), s.home()).unwrap_err().contains("过期"));
        assert!(parse_import(&json!({ "accessToken": a, "refreshToken": a }), s.home()).unwrap_err().contains("不是 refresh"));
    }

    #[test]
    fn store_import_seals_and_names_snapshot() {
        let s = sandbox("import");
        login(&s, "usr_live", "live@example.com");
        let a = jwt("usr_new", "sam@example.com", "access", FAR);
        let r = jwt("usr_new", "sam@example.com", "refresh", FAR);
        let (name, email) = store_import(&s, &a, &r).unwrap();
        assert_eq!(name, "sam");
        assert_eq!(email.as_deref(), Some("sam@example.com"));
        let v = s.view().unwrap();
        let p = v.profiles.iter().find(|p| p.name == "sam").unwrap();
        assert_eq!(p.email.as_deref(), Some("sam@example.com"), "快照里的密文本机可解");
        assert!(!p.current, "导入不切换登录");
        // 同 userId 再导入 → 覆盖同名，不长第二份
        let (again, _) = store_import(&s, &a, &r).unwrap();
        assert_eq!(again, "sam");
        assert_eq!(s.view().unwrap().profiles.len(), 1);
        // 磁盘上存的是 mrs1 密文而非明文
        let raw = std::fs::read_to_string(s.profile_path("sam")).unwrap();
        assert!(raw.contains("mrs1:") && !raw.contains(&a));
    }

    #[tokio::test]
    async fn refresh_pair_talks_to_auth_server() {
        let na = jwt("usr_a1", "a@example.com", "access", FAR + 7);
        let nr = jwt("usr_a1", "a@example.com", "refresh", FAR + 8);
        let (base, rx) = fake_auth("200 OK", json!({ "access_token": na, "refresh_token": nr, "token_type": "bearer" }).to_string());
        let (a, r) = refresh_pair(&http(), &base, "old.refresh.jwt").await.unwrap();
        assert_eq!((a, r), (na, nr));
        let req = rx.recv().unwrap();
        assert!(req.starts_with("POST /auth/refresh "), "{req}");
        assert!(req.contains(r#"{"refresh_token":"old.refresh.jwt"}"#), "{req}");
        // 不下发新 refresh → 沿用旧的
        let (base, _) = fake_auth("200 OK", json!({ "access_token": "x.y.z" }).to_string());
        assert_eq!(refresh_pair(&http(), &base, "keep.me.pls").await.unwrap().1, "keep.me.pls");
        // 401 → Rejected，文案指向重新登录
        let (base, _) = fake_auth("401 Unauthorized", r#"{"detail":"invalid"}"#.into());
        let err = refresh_pair(&http(), &base, "dead").await.unwrap_err();
        assert!(matches!(err, RefreshErr::Rejected(401, _)));
        assert!(err.message().contains("重新登录"));
        // 连接被拒 → Network
        assert!(matches!(refresh_pair(&http(), "http://127.0.0.1:1", "x").await.unwrap_err(), RefreshErr::Network(_)));
    }

    #[tokio::test]
    async fn refresh_target_writes_back_current_login_and_snapshot() {
        let s = sandbox("refresh");
        let lock = AccountsLock::default();
        login(&s, "usr_a1", "ann@example.com");
        // 当前登录：只改 token/refreshToken/exp，保留 name 与其它顶层字段
        let na = jwt("usr_a1", "ann@example.com", "access", FAR + 500);
        let nr = jwt("usr_a1", "ann@example.com", "refresh", FAR + 600);
        let (base, _) = fake_auth("200 OK", json!({ "access_token": na, "refresh_token": nr }).to_string());
        let r = refresh_target(&s, &lock, None, &base).await.unwrap();
        assert_eq!(r.target, "当前登录");
        assert_eq!(r.access_exp, FAR + 500);
        let setting = s.load_setting().unwrap();
        assert_eq!(setting["auth"]["exp"], FAR + 500);
        assert_eq!(setting["auth"]["name"], "Some One");
        assert_eq!(setting["failover"]["enabled"], true);
        assert_eq!(token::access_jwt(s.home(), setting["auth"]["token"].as_str().unwrap()).unwrap(), na);
        assert_eq!(token::access_jwt(s.home(), setting["auth"]["refreshToken"].as_str().unwrap()).unwrap(), nr);
        // 快照：整条记录重写，密文换新
        let a2 = jwt("usr_b2", "bob@example.com", "access", FAR);
        let r2 = jwt("usr_b2", "bob@example.com", "refresh", FAR);
        store_import(&s, &a2, &r2).unwrap();
        let nb = jwt("usr_b2", "bob@example.com", "access", FAR + 900);
        let (base, _) = fake_auth("200 OK", json!({ "access_token": nb }).to_string());
        let r = refresh_target(&s, &lock, Some("bob"), &base).await.unwrap();
        assert_eq!(r.target, "快照「bob」");
        assert_eq!(r.access_exp, FAR + 900);
        let rec: Value = serde_json::from_str(&std::fs::read_to_string(s.profile_path("bob")).unwrap()).unwrap();
        assert_eq!(token::access_jwt(s.home(), rec["auth"]["token"].as_str().unwrap()).unwrap(), nb);
        assert_eq!(token::access_jwt(s.home(), rec["auth"]["refreshToken"].as_str().unwrap()).unwrap(), r2, "未下发新 refresh 时沿用旧的");
        // 服务端拒绝 → 报错且不写回
        let (base, _) = fake_auth("401 Unauthorized", "{}".into());
        assert!(refresh_target(&s, &lock, None, &base).await.err().unwrap().contains("重新登录"));
        assert_eq!(s.load_setting().unwrap()["auth"]["exp"], FAR + 500);
    }

    #[tokio::test]
    async fn import_file_offline_and_online() {
        let s = sandbox("importfile");
        let lock = AccountsLock::default();
        login(&s, "usr_live", "live@example.com");
        let a = jwt("usr_imp", "imp@example.com", "access", FAR);
        let r = jwt("usr_imp", "imp@example.com", "refresh", FAR);
        let f = s.home().join("in.json");
        std::fs::write(&f, export_json(&a, &r, "2026-09-07T00:00:00Z").to_string()).unwrap();
        // 离线：原样入库
        let res = import_file(&s, &lock, &f, None).await.unwrap();
        assert_eq!(res.name, "imp");
        assert!(!res.refreshed && res.note.is_none());
        // 在线：换到服务端下发的新 pair
        let na = jwt("usr_imp", "imp@example.com", "access", FAR + 1);
        let (base, _) = fake_auth("200 OK", json!({ "access_token": na }).to_string());
        let res = import_file(&s, &lock, &f, Some(&base)).await.unwrap();
        assert!(res.refreshed);
        let rec: Value = serde_json::from_str(&std::fs::read_to_string(s.profile_path("imp")).unwrap()).unwrap();
        assert_eq!(token::access_jwt(s.home(), rec["auth"]["token"].as_str().unwrap()).unwrap(), na);
        // 服务端 401 → 凭证无效，拒绝入库（快照保持上一版）
        let (base, _) = fake_auth("401 Unauthorized", "{}".into());
        assert!(import_file(&s, &lock, &f, Some(&base)).await.err().unwrap().contains("凭证无效"));
        // 网络不通 → 带提示原样入库
        let res = import_file(&s, &lock, &f, Some("http://127.0.0.1:1")).await.unwrap();
        assert!(!res.refreshed && res.note.as_deref().is_some_and(|n| n.contains("未能联网验证")));
        // 坏文件
        std::fs::write(&f, "not json").unwrap();
        assert!(import_file(&s, &lock, &f, None).await.err().unwrap().contains("JSON"));
    }
}
