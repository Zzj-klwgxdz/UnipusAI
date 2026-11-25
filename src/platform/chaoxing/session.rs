//! 学习通 HTTP 会话：手动 Cookie 管理 + 重定向跟随。
//!
//! 为什么不用 reqwest 的 cookie_store：
//! - 需要把 Cookie 持久化回 config.json（cookie_provider 无法导出）；
//! - 需要看到每一跳 Set-Cookie（自动重定向会隐藏中间响应）。

use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, LOCATION, SET_COOKIE};
use reqwest::{Client, Method, StatusCode};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

/// 学习通默认 UA（与浏览器一致，避免被风控）。
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36 Edg/151.0.0.0";

pub struct Session {
    client: Client,
    cookies: Mutex<BTreeMap<String, String>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.cookies.lock().map(|c| c.len()).unwrap_or(0);
        f.debug_struct("ChaoxingSession")
            .field("cookies", &n)
            .finish()
    }
}

impl Session {
    /// 创建会话；`cookie` 为浏览器复制来的 `k=v; k2=v2` 形式（可空）。
    pub fn new(timeout_secs: u64, cookie: &str) -> Result<Self> {
        // 部分接口（如 /ananas/status）会校验 Referer，没有就 403。
        let mut default_headers = HeaderMap::new();
        default_headers.insert(
            reqwest::header::REFERER,
            HeaderValue::from_static("https://mooc1.chaoxing.com/"),
        );
        default_headers.insert(
            "x-requested-with",
            HeaderValue::from_static("XMLHttpRequest"),
        );
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs.max(5)))
            .user_agent(USER_AGENT)
            .default_headers(default_headers)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("构建学习通 HTTP 客户端失败")?;
        let mut map = BTreeMap::new();
        for part in cookie.split(';') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((k, v)) = part.split_once('=') {
                let k = k.trim();
                if !k.is_empty() {
                    map.insert(k.to_string(), v.trim().to_string());
                }
            }
        }
        Ok(Self {
            client,
            cookies: Mutex::new(map),
        })
    }

    /// 当前 Cookie 字符串（用于持久化到 config.json）。
    pub fn cookie_string(&self) -> String {
        self.cookies
            .lock()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default()
    }

    pub fn has_cookies(&self) -> bool {
        self.cookies
            .lock()
            .map(|m| !m.is_empty())
            .unwrap_or(false)
    }

    /// 合并一批 Cookie（登录后写入）。
    pub fn set_cookies(&self, pairs: &[(String, String)]) {
        if let Ok(mut m) = self.cookies.lock() {
            for (k, v) in pairs {
                m.insert(k.clone(), v.clone());
            }
        }
    }

    fn cookie_header(&self) -> Result<HeaderValue> {
        let s = self.cookie_string();
        HeaderValue::from_str(&s).context("Cookie 含非法字符（换行/非 ASCII）")
    }

    /// 记录响应里的 Set-Cookie。
    fn absorb_cookies(&self, headers: &HeaderMap) {
        let mut updates: Vec<(String, Option<String>)> = Vec::new();
        for v in headers.get_all(SET_COOKIE) {
            let Ok(s) = v.to_str() else { continue };
            let first = s.split(';').next().unwrap_or("");
            let Some((k, val)) = first.split_once('=') else {
                continue;
            };
            let k = k.trim().to_string();
            let val = val.trim().to_string();
            // 过期/删除标记
            let expired = s.to_ascii_lowercase().contains("max-age=0")
                || s.to_ascii_lowercase().contains("expires=thu, 01 jan 1970");
            if val.is_empty() || expired {
                updates.push((k, None));
            } else {
                updates.push((k, Some(val)));
            }
        }
        if let Ok(mut m) = self.cookies.lock() {
            for (k, v) in updates {
                match v {
                    Some(v) => {
                        m.insert(k, v);
                    }
                    None => {
                        m.remove(&k);
                    }
                }
            }
        }
    }

    /// 发起请求并手动跟随重定向（最多 10 跳），返回最终响应体。
    async fn fetch(&self, method: Method, url: &str, form: Option<Vec<(String, String)>>) -> Result<(StatusCode, String)> {
        let mut current = url.to_string();
        let mut method = method;
        let mut form = form;
        for _ in 0..10 {
            let mut req = self.client.request(method.clone(), &current);
            let cookie = self.cookie_header()?;
            if !cookie.is_empty() {
                req = req.header(reqwest::header::COOKIE, cookie);
            }
            if let Some(body) = form.take() {
                req = req.form(&body);
            }
            let resp = req
                .send()
                .await
                .with_context(|| format!("请求失败: {}", current))?;
            let status = resp.status();
            self.absorb_cookies(resp.headers());
            if status.is_redirection()
                && let Some(loc) = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok())
            {
                    let next = resolve_url(&current, loc);
                    // 303 一律 GET；301/302 对 POST 也按浏览器习惯转 GET
                    if matches!(status, StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER) {
                        method = Method::GET;
                    }
                    if !next.starts_with("http") {
                        anyhow::bail!("重定向地址不合法: {}", next);
                    }
                    current = next;
                    continue;
                }
            let text = resp
                .text()
                .await
                .with_context(|| format!("读取响应失败: {}", current))?;
            return Ok((status, text));
        }
        anyhow::bail!("重定向次数过多: {}", url)
    }

    pub async fn get_text(&self, url: &str) -> Result<String> {
        let (status, text) = self.fetch(Method::GET, url, None).await?;
        if !status.is_success() {
            anyhow::bail!("GET {} 返回 {}", url, status);
        }
        Ok(text)
    }

    pub async fn get_json(&self, url: &str) -> Result<Value> {
        let text = self.get_text(url).await?;
        serde_json::from_str(&text).with_context(|| format!("响应不是合法 JSON: {}", truncate(&text, 200)))
    }

    pub async fn post_form_text(&self, url: &str, form: Vec<(String, String)>) -> Result<String> {
        let (status, text) = self.fetch(Method::POST, url, Some(form)).await?;
        if !status.is_success() {
            anyhow::bail!("POST {} 返回 {}", url, status);
        }
        Ok(text)
    }

    /// GET，附加查询参数（自动 percent-encode）。
    pub async fn get_text_params(&self, url: &str, params: &[(&str, String)]) -> Result<String> {
        let mut u = reqwest::Url::parse(url).with_context(|| format!("非法 URL: {}", url))?;
        {
            let mut q = u.query_pairs_mut();
            for (k, v) in params {
                q.append_pair(k, v);
            }
        }
        self.get_text(u.as_str()).await
    }

    /// GET 并解析 JSON，附加查询参数。
    pub async fn get_json_params(&self, url: &str, params: &[(&str, String)]) -> Result<Value> {
        let text = self.get_text_params(url, params).await?;
        serde_json::from_str(&text)
            .with_context(|| format!("响应不是合法 JSON: {}", truncate(&text, 200)))
    }

    pub async fn post_form_json(&self, url: &str, form: Vec<(String, String)>) -> Result<Value> {
        let text = self.post_form_text(url, form).await?;
        serde_json::from_str(&text).with_context(|| format!("响应不是合法 JSON: {}", truncate(&text, 200)))
    }
}

/// 解析相对重定向地址。
pub fn resolve_url(base: &str, loc: &str) -> String {
    if loc.starts_with("http://") || loc.starts_with("https://") {
        return loc.to_string();
    }
    if let Some(rest) = loc.strip_prefix("//") {
        let scheme = base.split("://").next().unwrap_or("https");
        return format!("{}://{}", scheme, rest);
    }
    let (scheme, rest) = match base.split_once("://") {
        Some(v) => v,
        None => return loc.to_string(),
    };
    let host_end = rest.find('/').unwrap_or(rest.len());
    let host = &rest[..host_end];
    if loc.starts_with('/') {
        return format!("{}://{}{}", scheme, host, loc);
    }
    // 相对路径：取 base 的目录部分（去掉主机名之后）
    let path = &rest[host_end..];
    let dir = match path.rfind('/') {
        Some(i) => &path[..=i],
        None => "/",
    };
    format!("{}://{}{}{}", scheme, host, dir, loc)
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
    fn resolve_relative_redirects() {
        assert_eq!(
            resolve_url("https://a.com/x/y?z=1", "/p/q"),
            "https://a.com/p/q"
        );
        assert_eq!(
            resolve_url("https://a.com/x/y", "z?w=1"),
            "https://a.com/x/z?w=1"
        );
        assert_eq!(resolve_url("https://a.com/x", "//b.com/p"), "https://b.com/p");
        assert_eq!(
            resolve_url("https://a.com/x", "https://c.com/p"),
            "https://c.com/p"
        );
    }

    #[test]
    fn cookie_parse_and_string() {
        let s = Session::new(10, "a=1; jwt=abc; empty=").unwrap();
        let c = s.cookie_string();
        assert!(c.contains("a=1"));
        assert!(c.contains("jwt=abc"));
    }
}
