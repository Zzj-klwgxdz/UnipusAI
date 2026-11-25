//! 知到（智慧树）HTTP 会话。
//!
//! 登录依赖滑块验证，纯 HTTP 无法自动完成；因此使用浏览器 Cookie。
//! Cookie 需要包含 `CASLOGC` / `zhs-jt-cas` 等会话字段。

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36 Edg/151.0.0.0";

#[derive(Debug, Clone)]
pub struct ZsSession {
    client: Client,
    cookie: String,
}

impl ZsSession {
    pub fn new(cookie: &str, timeout_secs: u64) -> Result<Self> {
        let timeout = Duration::from_secs(timeout_secs.max(5));
        let client = Client::builder()
            .timeout(timeout)
            .user_agent(UA)
            .build()
            .context("构建知到 HTTP 客户端失败")?;
        Ok(Self {
            client,
            cookie: cookie.to_string(),
        })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// 以表单方式 POST，返回 JSON。
    pub async fn post_form(&self, url: &str, form: String, referer: &str) -> Result<Value> {
        let resp = self
            .client
            .post(url)
            .header("Cookie", &self.cookie)
            .header("Referer", referer)
            .header("Origin", origin_of(url))
            .header("Accept", "application/json, text/plain, */*")
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8")
            .body(form)
            .send()
            .await
            .with_context(|| format!("请求失败: {url}"))?;
        let status = resp.status();
        let text = resp.text().await.context("读取响应失败")?;
        if !status.is_success() {
            bail!("HTTP {}: {}", status, text.chars().take(300).collect::<String>());
        }
        if text.trim_start().starts_with('<') {
            bail!("登录已失效（服务端返回登录页/HTML，HTTP {}）", status);
        }
        serde_json::from_str(&text)
            .with_context(|| format!("响应不是 JSON: {}", text.chars().take(200).collect::<String>()))
    }

    /// 以 GET 方式取文本。
    pub async fn get_text(&self, url: &str, referer: &str) -> Result<String> {
        let resp = self
            .client
            .get(url)
            .header("Cookie", &self.cookie)
            .header("Referer", referer)
            .header("Accept", "*/*")
            .send()
            .await
            .with_context(|| format!("请求失败: {url}"))?;
        let status = resp.status();
        let text = resp.text().await.context("读取响应失败")?;
        if !status.is_success() {
            bail!("HTTP {}: {}", status, text.chars().take(300).collect::<String>());
        }
        Ok(text)
    }
}

fn origin_of(url: &str) -> &'static str {
    if url.contains("studyservice-api") {
        "https://studyvideoh5.zhihuishu.com"
    } else {
        "https://onlineexamh5new.zhihuishu.com"
    }
}
