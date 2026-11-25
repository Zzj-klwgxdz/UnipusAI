//! 学习通账密登录。
//!
//! 前端 `login.js` 的流程：
//! - `#t=true` 时，手机号与密码都做 **AES-128-CBC / PKCS7** 加密（key=iv=`u2oh6Vu^HWe4_AES`），
//!   输出 Base64；
//! - POST `https://passport2.chaoxing.com/fanyalogin`，返回 `{status,url,name,msg2,...}`；
//! - 成功后由 Set-Cookie 维持登录态。

use crate::platform::chaoxing::session::Session;
use anyhow::{Context, Result, bail};
use serde_json::Value;

/// 登录接口。
pub const FANYALOGIN: &str = "https://passport2.chaoxing.com/fanyalogin";
/// 登录时附带的服务标识。
pub const REFER: &str = "http://i.chaoxing.com/base?ws=1";

/// 登录结果。
#[derive(Debug, Clone, Default)]
pub struct LoginOutcome {
    /// 服务端返回的跳转地址（已解码）。
    pub redirect: String,
    /// 账号展示名。
    pub name: String,
}

/// 前端 usso 的字段加密：AES-128-CBC/PKCS7，key=iv，Base64 输出。
pub fn encrypt_field(s: &str) -> Result<String> {
    use aes::Aes128;
    use base64::Engine;
    use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};

    const KEY: &[u8; 16] = b"u2oh6Vu^HWe4_AES";
    let ct = cbc::Encryptor::<Aes128>::new_from_slices(KEY, KEY)
        .context("AES key/iv 长度非法")?
        .encrypt_padded_vec_mut::<Pkcs7>(s.as_bytes());
    Ok(base64::engine::general_purpose::STANDARD.encode(ct))
}

/// 简单 percent-decode（`%XX`），用于服务端返回的跳转地址。
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 用账号密码登录，返回带 Cookie 的会话。
pub async fn login(
    username: &str,
    password: &str,
    timeout_secs: u64,
) -> Result<(Session, LoginOutcome)> {
    if username.trim().is_empty() || password.is_empty() {
        bail!("chaoxing 账号或密码为空");
    }
    let session = Session::new(timeout_secs, "")?;
    let form = vec![
        ("fid".to_string(), "-1".to_string()),
        ("uname".to_string(), encrypt_field(username.trim())?),
        ("password".to_string(), encrypt_field(password)?),
        ("refer".to_string(), REFER.to_string()),
        ("t".to_string(), "true".to_string()),
        ("forbidotherlogin".to_string(), String::new()),
        ("validate".to_string(), String::new()),
        ("doubleFactorLogin".to_string(), String::new()),
        ("independentId".to_string(), String::new()),
        ("independentNameId".to_string(), String::new()),
    ];
    let text = session.post_form_text(FANYALOGIN, form).await?;
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("登录响应不是 JSON：{}", truncate(&text, 200)))?;
    let ok = v.get("status").and_then(|x| x.as_bool()).unwrap_or(false);
    if !ok {
        let msg = v
            .get("msg2")
            .or_else(|| v.get("msg"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if v.get("containTwoFactorLogin").and_then(|x| x.as_bool()) == Some(true) {
            bail!("登录需要二次验证（{}），请先在浏览器完成登录后改用 cookie", msg);
        }
        bail!("学习通登录失败：{}", if msg.is_empty() { "账号或密码错误" } else { msg });
    }
    let redirect = v
        .get("url")
        .and_then(|x| x.as_str())
        .map(percent_decode)
        .unwrap_or_default();
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    if session.cookie_string().is_empty() {
        bail!("登录成功但未收到 Cookie，请改用浏览器 Cookie");
    }
    Ok((
        session,
        LoginOutcome {
            redirect,
            name,
        },
    ))
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_is_base64_and_changes() {
        let a = encrypt_field("13800138000").unwrap();
        let b = encrypt_field("13800138000").unwrap();
        assert_eq!(a, b, "同输入应得到相同密文（固定 iv）");
        assert!(a.len() > 16);
        assert!(base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &a).is_ok());
    }

    #[test]
    fn percent_decode_works() {
        assert_eq!(
            percent_decode("http%3A%2F%2Fi.chaoxing.com%2Fbase%3Fws%3D1"),
            "http://i.chaoxing.com/base?ws=1"
        );
    }
}
