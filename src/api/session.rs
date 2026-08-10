use crate::config::Config;
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

pub const UCONTENT: &str = "https://ucontent.unipus.cn";

#[derive(Clone)]
pub struct Session {
    client: reqwest::Client,
    cfg: Config,
    config_path: PathBuf,
}

impl Session {
    pub fn new(cfg: Config, config_path: PathBuf) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout.max(3)))
            .user_agent(default_ua())
            .default_headers(build_base_headers(&cfg))
            .build()
            .context("构建 HTTP 客户端失败")?;
        Ok(Self {
            client,
            cfg,
            config_path,
        })
    }

    pub fn cfg(&self) -> &Config {
        &self.cfg
    }

    pub fn set_publish_version(&mut self, version: &str) -> Result<()> {
        if version.is_empty() || version == self.cfg.publish_version {
            return Ok(());
        }
        self.cfg.publish_version = version.to_string();
        let fresh = self.cfg.clone();
        fresh.save(&self.config_path)
    }

    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let resp = self
            .client
            .get(url)
            .header("accept", "application/json, text/plain, */*")
            .send()
            .await
            .context("GET 请求失败")?;
        let status = resp.status();
        let body = resp.text().await.context("读取响应失败")?;
        if !status.is_success() {
            anyhow::bail!("GET {} HTTP {}", url, status);
        }
        let v: Value = serde_json::from_str(&body)
            .with_context(|| format!("解析JSON失败: {}", truncate(&body, 300)))?;
        check_code(&v)?;
        serde_json::from_value(v).context("反序列化失败")
    }

    /// 下载原始字节（媒体文件 / 字幕文件）。
    pub async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .context("媒体下载请求失败")?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("下载 {} HTTP {}", url, status);
        }
        let bytes = resp.bytes().await.context("读取媒体失败")?;
        Ok(bytes.to_vec())
    }

    pub async fn post_json<I: serde::Serialize, T: DeserializeOwned>(&self,url: &str,payload: &I,) -> Result<T> {
        let resp = self
            .client
            .post(url)
            .header("accept", "application/json, text/plain, */*")
            .header("content-type", "application/json; charset=UTF-8")
            .json(payload)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("POST {} HTTP {}", url, status);
        }
        let v: Value = serde_json::from_str(&body)?;
        check_code(&v)?;
        serde_json::from_value(v).context("反序列化响应失败")
    }

    pub async fn post_raw(&self,url: &str,body: &str,) -> Result<(reqwest::StatusCode, String)> {
        let resp = self
            .client
            .post(url)
            .header("accept", "application/json, text/plain, */*")
            .header("content-type", "application/json; charset=UTF-8")
            .body(body.to_string())
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        Ok((status, text))
    }

    pub fn course_id(&self) -> &str {
        &self.cfg.course_id
    }

    pub fn open_id(&self) -> &str {
        &self.cfg.open_id
    }

    pub fn publish_version(&self) -> &str {
        &self.cfg.publish_version
    }
}

fn check_code(v: &Value) -> Result<()> {
    let code = v.get("code").and_then(|c| c.as_i64());
    if code.is_some_and(|c| c != 0) {
        let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
        anyhow::bail!("接口返回错误 code={} msg={}", code.unwrap(), msg);
    }
    Ok(())
}

fn build_base_headers(cfg: &Config) -> reqwest::header::HeaderMap {
    let mut map = reqwest::header::HeaderMap::new();
    if !cfg.cookie.is_empty() {
        map.insert(
            "cookie",
            reqwest::header::HeaderValue::from_str(cfg.cookie.as_str()).unwrap(),
        );
    }
    if !cfg.authorization.is_empty() {
        map.insert(
            "authorization",
            reqwest::header::HeaderValue::from_str(cfg.authorization.as_str()).unwrap(),
        );
    }
    if !cfg.x_annotator_auth_token.is_empty() {
        map.insert(
            "x-annotator-auth-token",
            reqwest::header::HeaderValue::from_str(cfg.x_annotator_auth_token.as_str()).unwrap(),
        );
    }
    if !cfg.open_id.is_empty() {
        map.insert(
            "u-openid",
            reqwest::header::HeaderValue::from_str(cfg.open_id.as_str()).unwrap(),
        );
        map.insert(
            "x-csrftoken",
            reqwest::header::HeaderValue::from_str(cfg.open_id.as_str()).unwrap(),
        );
    }
    map.insert(
        "u-app-id",
        reqwest::header::HeaderValue::from_static("39"),
    );
    map.insert(
        "u-platform",
        reqwest::header::HeaderValue::from_static("2"),
    );
    if !cfg.u_school.is_empty() {
        map.insert(
            "u-school",
            reqwest::header::HeaderValue::from_str(cfg.u_school.as_str()).unwrap(),
        );
    }
    map.insert(
        "appid",
        reqwest::header::HeaderValue::from_static("undefined"),
    );
    map.insert(
        "origin",
        reqwest::header::HeaderValue::from_static("https://ucontent.unipus.cn"),
    );
    map.insert(
        "referer",
        reqwest::header::HeaderValue::from_static(
            "https://ucontent.unipus.cn/_explorationpc_default/pc.html",
        ),
    );
    map
}

fn default_ua() -> String {
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36 Edg/151.0.0.0".to_string()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut r: String = s.chars().take(n).collect();
        r.push('…');
        r
    }
}

pub fn progress_url(course: &str, open_id: &str) -> String {
    format!(
        "{}/course/api/v2/course_progress/{}/{}/default",
        UCONTENT, course, open_id
    )
}

pub fn unit_progress_url(course: &str, unit_id: &str, open_id: &str) -> String {
    format!(
        "{}/course/api/v2/course_progress/{}/{}/{}/default/",
        UCONTENT, course, unit_id, open_id
    )
}

pub fn content_url(course: &str, group_id: &str) -> String {
    format!(
        "{}/course/api/v3/content/{}/{}/default",
        UCONTENT, course, group_id
    )
}

pub fn submit_url() -> &'static str {
    "https://ucontent.unipus.cn/course/api/v3/newExploration/submit"
}

pub fn user_module_url(course: &str, group_id: &str, ts: i64) -> String {
    format!(
        "{}/api/mobile/user_module/{}/{}-{}",
        UCONTENT, course, group_id, ts
    )
}