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
        if let Some((name, ..)) = self
            .profiles()
            .into_iter()
            .find(|(_, v, _)| v.get("userId").and_then(Value::as_str) == Some(uid))
        {
            self.write_profile(&name, &auth)?;
            return Ok(Some(name));
        }
        // 默认名优先取邮箱本地部分（alice@… → alice），回退账号名，再回退 userId
        let mut name = self
            .email_of(&auth)
            .map(|e| sanitize_name(crate::token::local_part(&e)))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| sanitize_name(auth.get("name").and_then(Value::as_str).unwrap_or("")));
        if name.is_empty() {
            name = format!("usr-{}", short_id(uid));
        }
        while self.profile_path(&name).exists() {
            name.push_str(&format!("-{}", &short_id(uid)[..4.min(short_id(uid).len())]));
        }
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

        let mut name = sanitize_name(want.unwrap_or(""));
        if name.is_empty() {
            name = same_user
                .map(|(n, ..)| n.clone())
                .or_else(|| {
                    self.email_of(&auth)
                        .map(|e| sanitize_name(crate::token::local_part(&e)))
                        .filter(|s| !s.is_empty())
                })
                .or_else(|| Some(sanitize_name(auth.get("name").and_then(Value::as_str)?)))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("usr-{}", short_id(&uid)));
        }
        if profiles
            .iter()
            .any(|(n, v, _)| n == &name && v.get("userId").and_then(Value::as_str) != Some(uid.as_str()))
        {
            return Err(format!("名字「{name}」已被另一个账号占用"));
        }
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
            "workspaces": [{"path": "/x"}],
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
        let s = temp_store("clash");
        seed(&s, "usr_a", "A");
        s.save(Some("同名")).unwrap();
        seed(&s, "usr_b", "B");
        assert!(s.save(Some("同名")).unwrap_err().contains("占用"));
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
        assert_eq!(sanitize_name("Ada Lovelace"), "Ada-Lovelace");
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
