//! 从 mirasim 的 `mrs1:` 令牌里取出邮箱（逆向自 server.cjs）。
//!
//! 链路：secret.key 是 Windows DPAPI blob（PowerShell `ConvertFrom-SecureString`
//! 的十六进制输出）→ `CryptUnprotectData` 还原出 UTF-16LE 的 64 位十六进制主密钥
//! → 令牌是 base64(`[IV 12][TAG 16][密文]`) 的 AES-256-GCM，明文是 JWT，
//! `email` 在 payload claim 里。全程本机、不联网；任何一步失败都回 None，
//! 显示层自会回退到账号名。密文本身永不出这个进程。

use base64::Engine;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const PREFIX: &str = "mrs1:";

/// 账号在令牌 JWT 里的可展示字段（邮箱、套餐、套餐到期）。
#[derive(Default)]
pub struct Account {
    pub email: Option<String>,
    pub plan: Option<String>,
    /// 套餐到期，Unix 秒（JWT `plan_exp` claim）。
    pub plan_exp: Option<i64>,
}

/// 解某令牌，取出账号可展示字段。key 从 home 下的 secret.key 派生（单条缓存，见下）。
pub fn account_of(home: &Path, token: &str) -> Option<Account> {
    let key = master_key(home)?;
    let jwt = decrypt_token(token, &key)?;
    let claims = jwt_claims(&jwt)?;
    Some(Account {
        email: str_claim(&claims, "email"),
        plan: str_claim(&claims, "plan"),
        plan_exp: claims.get("plan_exp").and_then(|v| v.as_i64()),
    })
}

/// 取某令牌对应账号的邮箱（account_of 的薄封装）。
pub fn email_of(home: &Path, token: &str) -> Option<String> {
    account_of(home, token)?.email
}

/// 主密钥单条缓存：DPAPI 解一次即可（secret.key 不在会话内轮换）。按 home 路径
/// 记忆，换目录（测试/沙箱）自动重派生。所有令牌都用同一把机器密钥加密。
fn master_key(home: &Path) -> Option<[u8; 32]> {
    static CACHE: Mutex<Option<(PathBuf, [u8; 32])>> = Mutex::new(None);
    let mut guard = CACHE.lock().unwrap();
    if let Some((p, k)) = guard.as_ref() {
        if p == home {
            return Some(*k);
        }
    }
    let k = load_master_key(home)?;
    *guard = Some((home.to_path_buf(), k));
    Some(k)
}

fn load_master_key(home: &Path) -> Option<[u8; 32]> {
    let raw = std::fs::read_to_string(home.join("secret.key")).ok()?;
    let blob = hex_decode(raw.trim())?;
    let unprotected = dpapi_unprotect(&blob)?; // UTF-16LE 的十六进制密钥
    hex_to_key(&utf16le_to_string(&unprotected))
}

/// 解一个 mrs1 令牌，返回内部 JWT 字符串。
fn decrypt_token(token: &str, key: &[u8; 32]) -> Option<String> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};

    let body = token.strip_prefix(PREFIX)?;
    let raw = b64_standard(body)?;
    if raw.len() <= 12 + 16 {
        return None;
    }
    let (iv, rest) = raw.split_at(12);
    let (tag, ct) = rest.split_at(16);
    // aes-gcm 约定密文尾随 tag，这里布局是 [IV][TAG][CT]，需重排成 CT||TAG
    let mut ct_tag = Vec::with_capacity(ct.len() + tag.len());
    ct_tag.extend_from_slice(ct);
    ct_tag.extend_from_slice(tag);

    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let plain = cipher.decrypt(Nonce::from_slice(iv), ct_tag.as_ref()).ok()?;
    String::from_utf8(plain).ok()
}

/// 解 JWT 的 payload 段为 claims 对象。
fn jwt_claims(jwt: &str) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = b64_url(payload)?;
    serde_json::from_slice(&bytes).ok()
}

/// 取一个非空字符串 claim。
fn str_claim(claims: &serde_json::Value, key: &str) -> Option<String> {
    let s = claims.get(key)?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// 从 JWT 里取 `email` claim（供测试用）。
fn email_from_jwt(jwt: &str) -> Option<String> {
    str_claim(&jwt_claims(jwt)?, "email")
}

/// 邮箱本地部分（@ 前），用于默认快照名。非邮箱串原样返回。
pub fn local_part(email: &str) -> &str {
    email.split('@').next().unwrap_or(email)
}

/* ---------- 编解码小工具 ---------- */

fn b64_standard(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .ok()
        .or_else(|| base64::engine::general_purpose::URL_SAFE.decode(s.trim()).ok())
}

fn b64_url(s: &str) -> Option<Vec<u8>> {
    let s = s.trim().trim_end_matches('=');
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).ok()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

fn hex_to_key(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn utf16le_to_string(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/* ---------- DPAPI（仅 Windows；其它平台恒 None，显示层回退账号名） ---------- */

#[cfg(windows)]
fn dpapi_unprotect(blob: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let mut input = CRYPT_INTEGER_BLOB {
        cbData: blob.len() as u32,
        pbData: blob.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&mut input, None, None, None, None, 0, &mut out).ok()?;
        if out.pbData.is_null() {
            return None;
        }
        let data = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
        Some(data)
    }
}

#[cfg(not(windows))]
fn dpapi_unprotect(_blob: &[u8]) -> Option<Vec<u8>> {
    None
}

/* ---------- 单测（不碰 DPAPI：直接给已知密钥造令牌验证解密+JWT 链路） ---------- */

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};

    /// 用已知密钥把一段明文封成 mrs1 令牌（复刻服务端布局 [IV][TAG][CT]）。
    fn make_token(key: &[u8; 32], plain: &str) -> String {
        let cipher = Aes256Gcm::new_from_slice(key).unwrap();
        let iv = [7u8; 12];
        // aes-gcm 输出 CT||TAG，拆出来按 [IV][TAG][CT] 重排
        let out = cipher.encrypt(Nonce::from_slice(&iv), plain.as_bytes()).unwrap();
        let (ct, tag) = out.split_at(out.len() - 16);
        let mut buf = Vec::new();
        buf.extend_from_slice(&iv);
        buf.extend_from_slice(tag);
        buf.extend_from_slice(ct);
        format!("{PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(buf))
    }

    fn jwt_with_email(email: &str) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{\"alg\":\"HS256\"}");
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!("{{\"sub\":\"usr_x\",\"exp\":123,\"email\":\"{email}\"}}"));
        format!("{header}.{payload}.sigsigsig")
    }

    #[test]
    fn decrypt_then_extract_email() {
        let key = [42u8; 32];
        let jwt = jwt_with_email("alice@example.com");
        let token = make_token(&key, &jwt);
        let got = decrypt_token(&token, &key).unwrap();
        assert_eq!(got, jwt);
        assert_eq!(email_from_jwt(&got).as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn wrong_key_fails_gracefully() {
        let token = make_token(&[1u8; 32], &jwt_with_email("a@b.com"));
        assert!(decrypt_token(&token, &[2u8; 32]).is_none());
    }

    #[test]
    fn decrypt_then_extract_plan_and_exp() {
        let key = [9u8; 32];
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{\"alg\":\"HS256\"}");
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            b"{\"email\":\"alice@example.com\",\"plan\":\"plus\",\"plan_exp\":1789029052}",
        );
        let jwt = format!("{header}.{payload}.sig");
        let got = decrypt_token(&make_token(&key, &jwt), &key).unwrap();
        let claims = jwt_claims(&got).unwrap();
        assert_eq!(str_claim(&claims, "plan").as_deref(), Some("plus"));
        assert_eq!(claims.get("plan_exp").and_then(|v| v.as_i64()), Some(1789029052));
        assert_eq!(str_claim(&claims, "email").as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn non_mrs1_or_garbage_is_none() {
        assert!(decrypt_token("plain-token", &[0u8; 32]).is_none());
        assert!(decrypt_token("mrs1:@@@notbase64@@@", &[0u8; 32]).is_none());
    }

    #[test]
    fn jwt_without_email_is_none() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{}");
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{\"sub\":\"x\"}");
        assert!(email_from_jwt(&format!("{header}.{payload}.s")).is_none());
    }

    #[test]
    fn local_part_of_email() {
        assert_eq!(local_part("alice@example.com"), "alice");
        assert_eq!(local_part("noatsign"), "noatsign");
    }
}
