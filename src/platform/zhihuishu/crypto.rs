//! 知到（智慧树）请求体加密。
//!
//! 两套 AES-128-CBC/PKCS7，IV 相同：
//! - 学习体系（studyservice-api）：key `azp53h0kft7qi78q`，表单 `secretStr=..&dateFormate=..`
//! - 考试体系（studentexam-api）：key `onbfhdyvz8x7otrp`，表单只有 `secretStr=..`
//!
//! 两者都是标准 base64，可直接与浏览器抓包对照。

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes128;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;

pub const LEARN_KEY: &[u8] = b"azp53h0kft7qi78q";
pub const EXAM_KEY: &[u8] = b"onbfhdyvz8x7otrp";
pub const IV: &[u8] = b"1g3qqdh4jvbskb9x";

fn xor_block(block: &mut [u8], prev: &[u8]) {
    for (b, p) in block.iter_mut().zip(prev) {
        *b ^= *p;
    }
}

pub fn aes_cbc_encrypt(plain: &[u8], key: &[u8], iv: &[u8]) -> Vec<u8> {
    let cipher = <Aes128 as KeyInit>::new_from_slice(key).expect("AES-128 key");
    let mut data = plain.to_vec();
    let pad = 16 - data.len() % 16;
    data.extend(std::iter::repeat_n(pad as u8, pad));
    let mut prev = iv.to_vec();
    for chunk in data.chunks_mut(16) {
        xor_block(chunk, &prev);
        cipher.encrypt_block(chunk.into());
        prev.copy_from_slice(chunk);
    }
    data
}

pub fn aes_cbc_decrypt(cipher_text: &[u8], key: &[u8], iv: &[u8]) -> Option<Vec<u8>> {
    if cipher_text.is_empty() || !cipher_text.len().is_multiple_of(16) {
        return None;
    }
    let cipher = <Aes128 as KeyInit>::new_from_slice(key).ok()?;
    let mut prev = iv.to_vec();
    let mut out = Vec::with_capacity(cipher_text.len());
    for chunk in cipher_text.chunks(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        let saved = block;
        cipher.decrypt_block((&mut block).into());
        xor_block(&mut block, &prev);
        out.extend_from_slice(&block);
        prev.copy_from_slice(&saved);
    }
    let pad = *out.last()? as usize;
    if pad == 0 || pad > 16 || out.len() < pad {
        return None;
    }
    out.truncate(out.len() - pad);
    Some(out)
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 考试体系请求体：只有 `secretStr`。
pub fn exam_form(params: &Value) -> String {
    let raw = serde_json::to_string(params).unwrap_or_else(|_| "{}".into());
    let ct = aes_cbc_encrypt(raw.as_bytes(), EXAM_KEY, IV);
    format!("secretStr={}", percent_encode(&STANDARD.encode(ct)))
}

/// 学习体系请求体：`secretStr` + 毫秒时间戳。
pub fn learn_form(params: &Value, date_ms: u128) -> String {
    let raw = serde_json::to_string(params).unwrap_or_else(|_| "{}".into());
    let ct = aes_cbc_encrypt(raw.as_bytes(), LEARN_KEY, IV);
    format!(
        "secretStr={}&dateFormate={}",
        percent_encode(&STANDARD.encode(ct)),
        date_ms
    )
}

/// 解密抓包得到的 `secretStr`（调试/测试用）。
pub fn decrypt_exam(secret: &str) -> Option<String> {
    let raw = STANDARD.decode(secret).ok()?;
    let pt = aes_cbc_decrypt(&raw, EXAM_KEY, IV)?;
    String::from_utf8(pt).ok()
}

pub fn decrypt_learn(secret: &str) -> Option<String> {
    let raw = STANDARD.decode(secret).ok()?;
    let pt = aes_cbc_decrypt(&raw, LEARN_KEY, IV)?;
    String::from_utf8(pt).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exam_vector_matches_browser() {
        // 浏览器 yxyz({a:1}) 的真实输出
        let form = exam_form(&serde_json::json!({"a": 1}));
        let b64 = form.trim_start_matches("secretStr=");
        let decoded = percent_decode(b64);
        assert_eq!(decoded, "8QrBC/TOnA3F5LwUno9JuQ==");
    }

    fn percent_decode(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn roundtrip() {
        let pt = r#"{"recruitId":"467029","courseId":"1000009484"}"#;
        let ct = aes_cbc_encrypt(pt.as_bytes(), LEARN_KEY, IV);
        let back = aes_cbc_decrypt(&ct, LEARN_KEY, IV).unwrap();
        assert_eq!(String::from_utf8(back).unwrap(), pt);
    }

    #[test]
    fn decrypt_known_save_body() {
        // 抓包得到的 saveStudentAnswer 明文（第三章）
        let secret = "hi7cPP8CbeHM2IP7SkqAXFzpy%2FTIIoiqe1apivRlkWd%2Fo%2BOW2VJ3Ad8W%2BfK%2Fb6UtOMgBqZ30lTPtXfEuIKMXfLSNc6rCuULmvshus3P5tIbaVF%2FClgoCQLoNEWCjVPLOMYqsgHOwAzZ3aULzx98nuwzU0GqyCG9DjDqct2QdOv9lM56ObFUEklW8FEQjRosyu7kpv2bXA4zKPkEN%2Bl5OM2jeqQU8wy9I3uU8FeMIV4KHABmVL%2BEcTQ1a6VMHAHsVttUrr3xa4FcyTI3L8jD4Sc7ZAiUVb0KuPoZo%2FI9tbcODU68BrfFaOqlALEg60TuRHbueTzOi7p%2FiydqYZdUldoo4L9tyZnTa9KVLc6IgiGeN%2FNSnzuEW09aXUKObggsN";
        let decoded = percent_decode(secret);
        let plain = decrypt_exam(&decoded).expect("decrypt");
        assert!(plain.contains("stuExamAnswer"));
        assert!(plain.contains("eid"));
    }
}
