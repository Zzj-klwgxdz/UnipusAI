use crate::api::session::{content_url, Session};
use aes::cipher::{BlockDecrypt, KeyInit};
use aes::Aes128;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct ContentWrapper {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub k: Option<String>,
    #[serde(default)]
    pub publish_version: Option<i64>,
    #[serde(default)]
    pub version: Option<i64>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub configs: Option<Value>,
}

pub struct FetchedContent {
    pub content: String,
    pub k: String,
    pub publish_version: i64,
}

pub async fn fetch_content(session: &Session, group_id: &str) -> Result<FetchedContent> {
    let url = content_url(session.course_id(), group_id);
    let wrap: ContentWrapper = session.get_json(&url).await?;
    let content = wrap.content.unwrap_or_default();
    let k = wrap.k.unwrap_or_default();
    let publish_version = wrap.publish_version.unwrap_or(0);
    Ok(FetchedContent {
        content,
        k,
        publish_version,
    })
}

/// 解密 v3 content。
/// 密文格式: "unipus.<hex>" 或 "<hex>"；key = "1a2b3c4d" + k 截取前 16 字节；
/// AES-128-ECB + ZeroPadding。
pub fn decrypt_content(content: &str, k: &str) -> Result<String> {
    let hex = content
        .strip_prefix("unipus.")
        .unwrap_or(content)
        .trim();
    if hex.is_empty() {
        return Ok(String::new());
    }
    let cipher_bytes = hex::decode(hex).context("content 不是合法 hex")?;

    let key_string = format!("1a2b3c4d{}", k);
    let mut key_arr = [0u8; 16];
    for (i, b) in key_string.as_bytes().iter().take(16).enumerate() {
        key_arr[i] = *b;
    }
    let cipher = Aes128::new(&key_arr.into());
    let mut out = Vec::with_capacity(cipher_bytes.len());
    for chunk in cipher_bytes.chunks(16) {
        if chunk.len() != 16 {
            break;
        }
        let mut block: aes::Block = Default::default();
        block.copy_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        out.extend_from_slice(&block);
    }
    while out.last() == Some(&0u8) {
        out.pop();
    }
    String::from_utf8(out).context("解密内容不是合法 UTF-8")
}

/// 从解密后的 JSON 解析题目模块。
pub fn parse_decrypted(json: &str) -> Result<Value> {
    serde_json::from_str(json).context("解密后的 content 不是合法 JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::{BlockEncrypt, KeyInit};

    fn encrypt_ecb(plain: &[u8], k: &str) -> Vec<u8> {
        let mut key_arr = [0u8; 16];
        let key_string = format!("1a2b3c4d{}", k);
        for (i, b) in key_string.as_bytes().iter().take(16).enumerate() {
            key_arr[i] = *b;
        }
        let cipher = Aes128::new(&key_arr.into());
        let padded = plain.to_vec();
        let mut out = Vec::new();
        for chunk in padded.chunks(16) {
            let mut block: aes::Block = Default::default();
            block.copy_from_slice(chunk);
            cipher.encrypt_block(&mut block);
            out.extend_from_slice(&block);
        }
        out
    }

    #[test]
    fn decrypt_real_zero_pad() {
        let mut plain = (0..40u8).collect::<Vec<u8>>();
        plain.extend_from_slice(&[0u8; 8]);
        let enc = encrypt_ecb(&plain, "20260808");
        let hex = enc.iter().map(|b| format!("{:02x}", b)).collect::<String>();
        let dec = decrypt_content(&format!("unipus.{}", hex), "20260808").unwrap();
        assert_eq!(dec.as_bytes(), &(0..40u8).collect::<Vec<u8>>()[..]);
    }

    #[test]
    fn decrypt_escapes_empty() {
        assert_eq!(decrypt_content("", "x").unwrap(), "");
    }
}