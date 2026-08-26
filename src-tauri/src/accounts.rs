//! mirasim 订阅账号切换：读写 `~/.mirasim/setting.json` 的 `auth` 块。
//!
//! 磁盘布局与配套的 cli/ 命令行工具完全一致
//! （`<home>/_account_switcher/{profiles,backups}`），两边可混用。
//! token/refreshToken 是绑定本机 secret.key 的 mrs1: 密文，这里当不透明
//! 字符串搬运，**永不**下发给前端——invoke 只回元数据。

use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const BACKUP_KEEP: usize = 20;

/// 切换/保存串行化（双击、连点保护）。
pub struct AccountsLock(pub Mutex<()>);

impl Default for AccountsLock {
    fn default() -> Self {
        Self(Mutex::new(()))
    }
}

#[derive(Serialize, Clone)]
pub struct ProfileMeta {
    pub name: String,
    #[serde(rename = "userId")]
    pub user_id: String,
    #[serde(rename = "accountName")]
    pub account_name: String,
    /// 从令牌 JWT 解出的邮箱（本机 secret.key 可解时）；否则 None，前端回退账号名。
    pub email: Option<String>,
    #[serde(rename = "savedAt")]
    pub saved_at: i64,
    pub current: bool,
}

#[derive(Serialize, Clone)]
pub struct AccountsView {
    /// 当前 setting.json 里登录的账号；未登录为 None。
    pub current: Option<CurrentMeta>,
    pub profiles: Vec<ProfileMeta>,
}

#[derive(Serialize, Clone)]
pub struct CurrentMeta {
    #[serde(rename = "userId")]
    pub user_id: String,
    pub name: String,
    /// 从令牌 JWT 解出的邮箱；解不出为 None，前端回退到 name。
    pub email: Option<String>,
    /// 从令牌 JWT 解出的套餐类型（如 "plus"）；解不出为 None，前端回退 config.planLabel。
    pub plan: Option<String>,
    /// 套餐到期，Unix 秒（JWT plan_exp）；解不出为 None，前端回退 config.validUntil。
    #[serde(rename = "planExp")]
    pub plan_exp: Option<i64>,
    /// 访问令牌过期时间（Unix 秒）；无则 0。刷新由 mirasim 自己做，仅展示用。
    pub exp: i64,
    /// 对应快照名（同 userId）；没有则 None。
    pub profile: Option<String>,
}

fn mirasim_home() -> PathBuf {
    if let Ok(h) = std::env::var("MIRASIM_HOME") {
        let h = h.trim();
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    let base = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(".mirasim")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn read_json(path: &Path) -> Result<Value, String> {
    let s = fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    serde_json::from_str(s.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("{} 解析失败:{e}", path.display()))
}

/// 临时文件 + 改名原子写（Windows 上 rename 覆盖同卷已有文件）。
fn write_json_atomic(path: &Path, v: &Value) -> Result<(), String> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, serde_json::to_string_pretty(v).unwrap())
        .map_err(|e| format!("写入 {} 失败:{e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("替换 {} 失败:{e}", path.display()))
}

fn has_login(auth: &Value) -> bool {
    let s = |k: &str| auth.get(k).and_then(Value::as_str).unwrap_or("");
    !s("token").is_empty() && !s("userId").is_empty()
}

/// 与 CLI 相同的名字清洗：字母/数字（含 CJK）/_-. 之外换 '-'，掐头去尾，限 40 字符。
fn sanitize_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches(|c| c == '-' || c == '.').to_string();
    out.chars().take(40).collect()
}

fn short_id(user_id: &str) -> String {
    user_id.trim_start_matches("usr_").chars().take(8).collect()
}

/// 自动命名去重：base 若被【别的账号】占用，先用邮箱域名首段消歧
/// （sam@outlook.com 与 sam@gmail.com → sam / sam-outlook），
/// 再不行附短 userId 兜底，保证唯一。本账号自己占用的名字视为可用（就是它）。
fn unique_auto_name(
    base: &str,
    uid: &str,
    email: Option<&str>,
    profiles: &[(String, Value, std::path::PathBuf)],
) -> String {
    let taken = |n: &str| {
        profiles
            .iter()
            .any(|(pn, v, _)| pn == n && v.get("userId").and_then(Value::as_str) != Some(uid))
    };
    if !taken(base) {
        return base.to_string();
    }
    // 邮箱域名首段：sam@outlook.com → outlook
    if let Some(dom) = email
        .and_then(|e| e.split('@').nth(1))
        .and_then(|d| d.split('.').next())
    {
        let dom = sanitize_name(dom);
        if !dom.is_empty() {
            let cand = format!("{base}-{dom}");
            if !taken(&cand) {
                return cand;
            }
        }
    }
    // 兜底：附短 userId；万一还撞（不同账号同短 id）就补字符
    let mut cand = format!("{base}-{}", short_id(uid));
    while taken(&cand) {
        cand.push('x');
    }
    cand
}

/// setting-YYYYMMDD-HHMMSS（UTC）。手算 civil date，省掉时间库依赖。
fn backup_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Howard Hinnant civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        y,
        m,
        d,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

struct Store {
    home: PathBuf,
}

impl Store {
    fn new(home: PathBuf) -> Self {
        Self { home }
    }

    /// 从一个 auth 块的 token 解出账号可展示字段（邮箱/套餐/到期）。
    fn account_of(&self, auth: &Value) -> Option<crate::token::Account> {
        let token = auth.get("token").and_then(Value::as_str)?;
        crate::token::account_of(&self.home, token)
    }

    /// 从一个 auth 块的 token 解出邮箱（本机 secret.key 可解时）。
    fn email_of(&self, auth: &Value) -> Option<String> {
        self.account_of(auth)?.email
    }

    fn setting_path(&self) -> PathBuf {
        self.home.join("setting.json")
    }
    fn profiles_dir(&self) -> PathBuf {
        self.home.join("_account_switcher").join("profiles")
    }
    fn backups_dir(&self) -> PathBuf {
        self.home.join("_account_switcher").join("backups")
    }

    fn load_setting(&self) -> Result<Value, String> {
        let p = self.setting_path();
        if !p.exists() {
            return Err(format!("找不到 {}（mirasim 数据目录不对？）", p.display()));
        }
        read_json(&p)
    }

    /// 所有可用快照（损坏的静默跳过），zh 排序近似：按名字码点排。
    fn profiles(&self) -> Vec<(String, Value, PathBuf)> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(self.profiles_dir()) else {
            return out;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Ok(v) = read_json(&path) {
                if v.get("auth").map(has_login).unwrap_or(false) {
                    let name = v.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                    out.push((name, v, path));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn profile_path(&self, name: &str) -> PathBuf {
        self.profiles_dir().join(format!("{name}.json"))
    }

    fn write_profile(&self, name: &str, auth: &Value) -> Result<(), String> {
        fs::create_dir_all(self.profiles_dir()).map_err(|e| format!("建目录失败:{e}"))?;
        // email 落进元数据，供 CLI 复用；显示时若缺失会从 token 现解
        let rec = json!({
            "name": name,
            "userId": auth.get("userId").cloned().unwrap_or(Value::Null),
            "accountName": auth.get("name").cloned().unwrap_or(json!("")),
            "email": self.email_of(auth),
            "savedAt": now_ms(),
            "auth": auth,
        });
        write_json_atomic(&self.profile_path(name), &rec)
    }

    /// 当前登录态回存到同 userId 的快照；没有则以账号名自动建一份。
    fn snapshot_current(&self, setting: &Value) -> Result<Option<String>, String> {
        let auth = setting.get("auth").cloned().unwrap_or(Value::Null);
        if !has_login(&auth) {
            return Ok(None);
        }
        let uid = auth.get("userId").and_then(Value::as_str).unwrap_or("");
        let profiles = self.profiles();
        if let Some((name, ..)) = profiles
            .iter()
            .find(|(_, v, _)| v.get("userId").and_then(Value::as_str) == Some(uid))
        {
            let name = name.clone();
            self.write_profile(&name, &auth)?;
            return Ok(Some(name));
        }
        // 默认名优先取邮箱本地部分（yi.liu@… → yi.liu），回退账号名，再回退 userId
        let email = self.email_of(&auth);
        let base = email
            .as_deref()
            .map(|e| sanitize_name(crate::token::local_part(e)))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| sanitize_name(auth.get("name").and_then(Value::as_str).unwrap_or("")));
        let base = if base.is_empty() {
            format!("usr-{}", short_id(uid))
        } else {
            base
        };
        // 撞到别的账号（同本地部分不同域名等）时自动消歧，不报错
        let name = unique_auto_name(&base, uid, email.as_deref(), &profiles);
        self.write_profile(&name, &auth)?;
        Ok(Some(name))
    }

    fn backup_setting(&self) -> Result<(), String> {
        let dir = self.backups_dir();
        fs::create_dir_all(&dir).map_err(|e| format!("建备份目录失败:{e}"))?;
        let dest = dir.join(format!("setting-{}.json", backup_stamp()));
        fs::copy(self.setting_path(), &dest).map_err(|e| format!("备份失败:{e}"))?;
        // 只留最近 BACKUP_KEEP 份
        let mut baks: Vec<_> = fs::read_dir(&dir)
            .map(|it| {
                it.flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with("setting-"))
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default();
        baks.sort();
        let excess = baks.len().saturating_sub(BACKUP_KEEP);
        for p in baks.into_iter().take(excess) {
            let _ = fs::remove_file(p);
        }
        Ok(())
    }

    fn view(&self) -> Result<AccountsView, String> {
        let setting = self.load_setting()?;
        let auth = setting.get("auth").cloned().unwrap_or(Value::Null);
        let cur_uid = auth.get("userId").and_then(Value::as_str).unwrap_or("").to_string();
        let logged_in = has_login(&auth);

        let profiles: Vec<ProfileMeta> = self
            .profiles()
            .into_iter()
            .map(|(name, v, _)| {
                let uid = v.get("userId").and_then(Value::as_str).unwrap_or("").to_string();
                // 优先用存下的 email；老快照/CLI 存的没有就从 token 现解
                let email = v
                    .get("email")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .or_else(|| v.get("auth").and_then(|a| self.email_of(a)));
                ProfileMeta {
                    current: logged_in && uid == cur_uid,
                    name,
                    account_name: v
                        .get("accountName")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    email,
                    saved_at: v.get("savedAt").and_then(Value::as_i64).unwrap_or(0),
                    user_id: uid,
                }
            })
            .collect();

        // 当前账号一次性解出邮箱/套餐/到期（同一 token，避免多次解密）
        let acct = self.account_of(&auth);
        let current = logged_in.then(|| CurrentMeta {
            user_id: cur_uid.clone(),
            name: auth.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            email: acct.as_ref().and_then(|a| a.email.clone()),
            plan: acct.as_ref().and_then(|a| a.plan.clone()),
            plan_exp: acct.as_ref().and_then(|a| a.plan_exp),
            exp: auth.get("exp").and_then(Value::as_i64).unwrap_or(0),
            profile: profiles
                .iter()
                .find(|p| p.user_id == cur_uid)
                .map(|p| p.name.clone()),
        });

        Ok(AccountsView { current, profiles })
    }

    fn save(&self, want: Option<&str>) -> Result<String, String> {
        let setting = self.load_setting()?;
        let auth = setting.get("auth").cloned().unwrap_or(Value::Null);
        if !has_login(&auth) {
            return Err("当前没有登录，没什么可保存的".into());
        }
        let uid = auth.get("userId").and_then(Value::as_str).unwrap_or("").to_string();
        let profiles = self.profiles();
        let same_user = profiles
            .iter()
            .find(|(_, v, _)| v.get("userId").and_then(Value::as_str) == Some(uid.as_str()));

        let email = self.email_of(&auth);
        let explicit = sanitize_name(want.unwrap_or(""));
        let name = if !explicit.is_empty() {
            // 用户显式命名：撞到【别的账号】才报错，请其换名
            if profiles
                .iter()
                .any(|(n, v, _)| n == &explicit && v.get("userId").and_then(Value::as_str) != Some(uid.as_str()))
            {
                return Err(format!("名字「{explicit}」已被另一个账号占用，换个名字"));
            }
            explicit
        } else if let Some((n, ..)) = same_user {
            // 本账号已有快照：沿用原名（刷新登录态）
            n.clone()
        } else {
            // 自动命名：邮箱本地部分 / 账号名 / userId，撞到别的账号自动消歧（不报错）
            let base = email
                .as_deref()
                .map(|e| sanitize_name(crate::token::local_part(e)))
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    let n = sanitize_name(auth.get("name").and_then(Value::as_str).unwrap_or(""));
                    (!n.is_empty()).then_some(n)
                })
                .unwrap_or_else(|| format!("usr-{}", short_id(&uid)));
            unique_auto_name(&base, &uid, email.as_deref(), &profiles)
        };
        // 同账号改名：移除旧文件，保持一账号一快照
        if let Some((old, _, path)) = same_user {
            if old != &name {
                let _ = fs::remove_file(path);
            }
        }
        self.write_profile(&name, &auth)?;
        Ok(name)
    }

    fn switch(&self, name: &str) -> Result<(), String> {
        let target = self
            .profiles()
            .into_iter()
            .find(|(n, ..)| n == name)
            .ok_or_else(|| format!("找不到快照「{name}」"))?;
        let target_auth = target.1.get("auth").cloned().unwrap_or(Value::Null);
        let target_uid = target_auth.get("userId").and_then(Value::as_str).unwrap_or("");

        let setting = self.load_setting()?;
        let cur = setting.get("auth").cloned().unwrap_or(Value::Null);
        if has_login(&cur) && cur.get("userId").and_then(Value::as_str) == Some(target_uid) {
            // 已在该账号：只刷新快照
            self.write_profile(name, &cur)?;
            return Ok(());
        }

        self.snapshot_current(&setting)?;
        self.backup_setting()?;
        // 写前重读，缩小与 mirasim 自身写入的竞争窗口
        let mut fresh = self.load_setting()?;
        fresh["auth"] = target_auth;
        write_json_atomic(&self.setting_path(), &fresh)
    }

    fn remove(&self, name: &str) -> Result<(), String> {
        let path = self.profile_path(name);
        if !path.exists() {
            return Err(format!("找不到快照「{name}」"));
        }
        fs::remove_file(&path).map_err(|e| format!("删除失败:{e}"))
    }
}

/* ---------- tauri 命令 ---------- */

#[tauri::command]
pub fn accounts_list() -> Result<AccountsView, String> {
    Store::new(mirasim_home()).view()
}

#[tauri::command]
pub fn accounts_save(
    lock: tauri::State<'_, AccountsLock>,
    name: Option<String>,
) -> Result<AccountsView, String> {
    let _g = lock.0.lock().unwrap();
    let store = Store::new(mirasim_home());
    store.save(name.as_deref())?;
    store.view()
}

#[tauri::command]
pub fn accounts_switch(
    lock: tauri::State<'_, AccountsLock>,
    name: String,
) -> Result<AccountsView, String> {
    let _g = lock.0.lock().unwrap();
    let store = Store::new(mirasim_home());
    store.switch(&name)?;
    store.view()
}

#[tauri::command]
pub fn accounts_remove(
    lock: tauri::State<'_, AccountsLock>,
    name: String,
) -> Result<AccountsView, String> {
    let _g = lock.0.lock().unwrap();
    let store = Store::new(mirasim_home());
    store.remove(&name)?;
    store.view()
}

/* ---------- 单测 ---------- */

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("gg-acct-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Store::new(dir)
    }

    fn seed(store: &Store, uid: &str, name: &str) {
        let setting = json!({
            "auth": {"token": format!("mrs1:tok-{uid}"), "refreshToken": format!("mrs1:ref-{uid}"),
                      "userId": uid, "exp": 4_000_000_000i64, "name": name},
            "failover": {"enabled": true, "threshold": 0.95},
            "workspaces": [{"path": "F:/x"}],
        });
        fs::write(store.setting_path(), serde_json::to_string_pretty(&setting).unwrap()).unwrap();
    }

    #[test]
    fn save_then_view_marks_current() {
        let s = temp_store("save");
        seed(&s, "usr_a", "账号A");
        assert_eq!(s.save(Some("主号")).unwrap(), "主号");
        let v = s.view().unwrap();
        assert_eq!(v.profiles.len(), 1);
        assert!(v.profiles[0].current);
        assert_eq!(v.current.unwrap().profile.as_deref(), Some("主号"));
    }

    #[test]
    fn save_rename_same_user_keeps_single_profile() {
        let s = temp_store("rename");
        seed(&s, "usr_a", "账号A");
        s.save(Some("旧名")).unwrap();
        s.save(Some("新名")).unwrap();
        let v = s.view().unwrap();
        assert_eq!(v.profiles.len(), 1);
        assert_eq!(v.profiles[0].name, "新名");
    }

    #[test]
    fn save_name_clash_with_other_user_rejected() {
        // 显式命名撞到别的账号仍然报错（让用户改名）
        let s = temp_store("clash");
        seed(&s, "usr_a", "A");
        s.save(Some("同名")).unwrap();
        seed(&s, "usr_b", "B");
        assert!(s.save(Some("同名")).unwrap_err().contains("占用"));
    }

    #[test]
    fn auto_save_disambiguates_instead_of_erroring() {
        // 自动命名（点「保存当前登录为快照」，name=None）撞到别的账号时不报错，自动消歧。
        // 复现：两账号邮箱本地部分相同、后缀不同 → 派生同一 base。测试环境无 secret.key，
        // email 解不出，base 回退到账号名；用同名账号名即可复现同一冲突路径。
        let s = temp_store("autodup");
        seed(&s, "usr_aaaa1111", "Same Name");
        let n1 = s.save(None).unwrap();
        seed(&s, "usr_bbbb2222", "Same Name");
        let n2 = s.save(None).unwrap(); // 不再报「占用」
        assert_ne!(n1, n2, "两个不同账号的自动命名必须不同");
        let v = s.view().unwrap();
        assert_eq!(v.profiles.len(), 2, "两个账号都应保存下来");
    }

    /// 全链路复现用户 bug：两个账号邮箱本地部分相同、域名不同。
    /// 沙箱里现造 DPAPI 保护的 secret.key + 真 mrs1 令牌（不碰任何真实凭证），
    /// 走 email 解密 → 本地部分派生 → 域名消歧的完整路径。
    #[cfg(windows)]
    #[test]
    fn auto_save_same_localpart_different_domain_end_to_end() {
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes256Gcm, Nonce};
        use base64::Engine;

        fn dpapi_protect(data: &[u8]) -> Vec<u8> {
            use windows::Win32::Foundation::{HLOCAL, LocalFree};
            use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};
            let mut input = CRYPT_INTEGER_BLOB {
                cbData: data.len() as u32,
                pbData: data.as_ptr() as *mut u8,
            };
            let mut out = CRYPT_INTEGER_BLOB::default();
            unsafe {
                CryptProtectData(&mut input, windows::core::PCWSTR::null(), None, None, None, 0, &mut out)
                    .expect("CryptProtectData");
                let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
                let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
                v
            }
        }

        // secret.key = hex(DPAPI(UTF-16LE("64位十六进制主密钥")))，与 load_master_key 互逆
        let key = [33u8; 32];
        let key_hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        let key_utf16: Vec<u8> = key_hex.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let blob = dpapi_protect(&key_utf16);
        let blob_hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();

        let make_token = |jwt: &str| {
            let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
            let iv = [7u8; 12];
            let out = cipher.encrypt(Nonce::from_slice(&iv), jwt.as_bytes()).unwrap();
            let (ct, tag) = out.split_at(out.len() - 16);
            let mut buf = Vec::new();
            buf.extend_from_slice(&iv);
            buf.extend_from_slice(tag);
            buf.extend_from_slice(ct);
            format!("mrs1:{}", base64::engine::general_purpose::STANDARD.encode(buf))
        };
        let jwt_for = |uid: &str, email: &str| {
            let b64 = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
            format!(
                "{}.{}.sig",
                b64(b"{\"alg\":\"HS256\"}"),
                b64(format!("{{\"sub\":\"{uid}\",\"exp\":123,\"email\":\"{email}\"}}").as_bytes())
            )
        };
        let seed_real = |s: &Store, uid: &str, email: &str| {
            let setting = json!({
                "auth": {"token": make_token(&jwt_for(uid, email)), "refreshToken": "mrs1:r",
                          "userId": uid, "exp": 4_000_000_000i64, "name": "N"},
            });
            fs::write(s.setting_path(), serde_json::to_string_pretty(&setting).unwrap()).unwrap();
        };

        let s = temp_store("e2edom");
        fs::write(s.home.join("secret.key"), &blob_hex).unwrap();

        seed_real(&s, "usr_g1", "sam123@gmail.com");
        assert_eq!(s.save(None).unwrap(), "sam123");
        seed_real(&s, "usr_o2", "sam123@outlook.com");
        assert_eq!(s.save(None).unwrap(), "sam123-outlook", "撞名自动附域名，而不是报「占用」");

        let v = s.view().unwrap();
        assert_eq!(v.profiles.len(), 2);
        let emails: Vec<_> = v.profiles.iter().filter_map(|p| p.email.clone()).collect();
        assert!(emails.contains(&"sam123@gmail.com".to_string()));
        assert!(emails.contains(&"sam123@outlook.com".to_string()));
    }

    #[test]
    fn unique_auto_name_domain_then_shortid() {
        let profiles = vec![(
            "sam123".to_string(),
            json!({"userId": "usr_aaaa"}),
            std::path::PathBuf::from("x"),
        )];
        // 名字空闲 → 原样
        assert_eq!(unique_auto_name("fresh", "usr_bbbb", None, &profiles), "fresh");
        // 被本账号自己占用 → 视为可用（就是它，刷新）
        assert_eq!(
            unique_auto_name("sam123", "usr_aaaa", Some("sam123@gmail.com"), &profiles),
            "sam123"
        );
        // 被别的账号占用、本地部分相同后缀不同 → 用域名首段消歧
        assert_eq!(
            unique_auto_name("sam123", "usr_bbbb", Some("sam123@outlook.com"), &profiles),
            "sam123-outlook"
        );
        // 被别的账号占用、拿不到邮箱 → 附短 userId 兜底
        assert_eq!(
            unique_auto_name("sam123", "usr_bbbb2222", None, &profiles),
            "sam123-bbbb2222"
        );
    }

    #[test]
    fn switch_swaps_auth_snapshots_and_backs_up() {
        let s = temp_store("switch");
        seed(&s, "usr_a", "A");
        s.save(Some("甲")).unwrap();
        seed(&s, "usr_b", "B");
        s.switch("甲").unwrap();

        let setting = read_json(&s.setting_path()).unwrap();
        assert_eq!(setting["auth"]["userId"], "usr_a");
        assert_eq!(setting["auth"]["token"], "mrs1:tok-usr_a");
        // B 被自动快照（名字来自账号名）
        let v = s.view().unwrap();
        assert!(v.profiles.iter().any(|p| p.user_id == "usr_b"));
        // 其它配置字段原样
        assert_eq!(setting["failover"]["threshold"], 0.95);
        // 生成了备份，内容是切换前的 B
        let baks: Vec<_> = fs::read_dir(s.backups_dir()).unwrap().flatten().collect();
        assert_eq!(baks.len(), 1);
        let bak = read_json(&baks[0].path()).unwrap();
        assert_eq!(bak["auth"]["userId"], "usr_b");
    }

    #[test]
    fn switch_to_current_only_refreshes() {
        let s = temp_store("same");
        seed(&s, "usr_a", "A");
        s.save(Some("甲")).unwrap();
        s.switch("甲").unwrap();
        assert!(!s.backups_dir().exists(), "同账号切换不应产生备份");
    }

    #[test]
    fn switch_works_when_logged_out() {
        let s = temp_store("loggedout");
        seed(&s, "usr_a", "A");
        s.save(Some("甲")).unwrap();
        let mut setting = s.load_setting().unwrap();
        setting["auth"] = json!({});
        write_json_atomic(&s.setting_path(), &setting).unwrap();
        assert!(s.view().unwrap().current.is_none());
        s.switch("甲").unwrap();
        assert_eq!(s.load_setting().unwrap()["auth"]["userId"], "usr_a");
    }

    #[test]
    fn corrupt_profile_skipped() {
        let s = temp_store("corrupt");
        seed(&s, "usr_a", "A");
        s.save(Some("好的")).unwrap();
        fs::write(s.profile_path("坏的"), "not json").unwrap();
        assert_eq!(s.view().unwrap().profiles.len(), 1);
    }

    #[test]
    fn remove_deletes_profile() {
        let s = temp_store("rm");
        seed(&s, "usr_a", "A");
        s.save(Some("甲")).unwrap();
        s.remove("甲").unwrap();
        assert!(s.view().unwrap().profiles.is_empty());
        assert!(s.remove("甲").is_err());
    }

    #[test]
    fn sanitize_names() {
        assert_eq!(sanitize_name("Yi Liu"), "Yi-Liu");
        assert_eq!(sanitize_name("  主号!!"), "主号");
        assert_eq!(sanitize_name("a/b\\c"), "a-b-c");
        assert_eq!(sanitize_name("---"), "");
    }

    #[test]
    fn backup_stamp_shape() {
        let st = backup_stamp();
        assert_eq!(st.len(), 15, "{st}");
        assert_eq!(&st[8..9], "-");
    }
}
