use crate::api::login::{self, Credentials};
use crate::config::Config;
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub const UCONTENT: &str = "https://ucontent.unipus.cn";

/// 会话级认证状态（所有 Session 克隆共享，自动刷新后立即生效）。
#[derive(Debug, Clone, Default)]
pub struct AuthState {
    pub jwt: String,
    pub cookie: String,
    pub open_id: String,
    pub refresh_token: String,
    pub jwt_expire: i64,
    pub rt_expire: i64,
    /// x-annotator-auth-token（本地签发，401 时自动重提取重签）
    pub annotator: String,
}

fn auth_from_cfg(cfg: &Config) -> AuthState {
    let jwt = cfg
        .cookie_jwt()
        .unwrap_or_else(|| cfg.authorization.clone());
    let cookie = if cfg.cookie.is_empty() && !jwt.is_empty() {
        format!("jwt={}", jwt)
    } else {
        cfg.cookie.clone()
    };
    // config 缺 open_id 时从 jwt 声明兜底（手动清理过 config 的场景）
    let open_id = if cfg.open_id.is_empty() {
        login::jwt_open_id(&jwt).unwrap_or_default()
    } else {
        cfg.open_id.clone()
    };
    AuthState {
        jwt,
        cookie,
        open_id,
        refresh_token: cfg.refresh_token.clone(),
        jwt_expire: cfg.jwt_expire,
        rt_expire: cfg.rt_expire,
        annotator: cfg.x_annotator_auth_token.clone(),
    }
}

#[derive(Clone)]
pub struct Session {
    client: reqwest::Client,
    cfg: Config,
    config_path: PathBuf,
    auth: Arc<RwLock<AuthState>>,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    annotator_lock: Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let auth = self.auth.read().ok();
        let (jwt_len, has_rt) = auth
            .as_ref()
            .map(|a| (a.jwt.len(), !a.refresh_token.is_empty()))
            .unwrap_or((0, false));
        f.debug_struct("Session")
            .field("config_path", &self.config_path)
            .field("jwt_len", &jwt_len)
            .field("has_refresh_token", &has_rt)
            .finish()
    }
}

impl Session {
    pub fn new(cfg: Config, config_path: PathBuf) -> Result<Self> {
        let client = build_client(&cfg)?;
        let auth = auth_from_cfg(&cfg);
        Ok(Self {
            client,
            cfg,
            config_path,
            auth: Arc::new(RwLock::new(auth)),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            annotator_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    pub fn cfg(&self) -> &Config {
        &self.cfg
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }

    /// 当前生效的认证信息快照。
    pub fn auth_snapshot(&self) -> AuthState {
        self.auth.read().map(|a| a.clone()).unwrap_or_default()
    }

    /// 把配置中的凭证同步进共享认证状态（配置更新后调用）。
    fn sync_auth(&self) {
        if let Ok(mut a) = self.auth.write() {
            *a = auth_from_cfg(&self.cfg);
        }
    }

    /// 替换配置：写入 config.json 并重建 HTTP 客户端（默认头依赖配置）。
    pub fn update_config(&mut self, cfg: Config) -> Result<()> {
        let client = build_client(&cfg)?;
        cfg.save(&self.config_path)?;
        self.client = client;
        self.cfg = cfg;
        self.sync_auth();
        Ok(())
    }

    /// 运行时覆盖提交间隔（毫秒），不写回 config.json。
    pub fn set_interval_ms(&mut self, ms: u64) {
        self.cfg.interval_ms = ms;
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
            .send_with_retry(|| {
                self.apply_auth(
                    self.client
                        .get(url)
                        .header("accept", "application/json, text/plain, */*"),
                )
            })
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
            .send_with_retry(|| self.apply_auth(self.client.get(url)))
            .await
            .context("媒体下载请求失败")?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("下载 {} HTTP {}", url, status);
        }
        let bytes = resp.bytes().await.context("读取媒体失败")?;
        Ok(bytes.to_vec())
    }

    pub async fn post_json<I: serde::Serialize, T: DeserializeOwned>(
        &self,
        url: &str,
        payload: &I,
    ) -> Result<T> {
        let resp = self
            .send_with_retry(|| {
                self.apply_auth(
                    self.client
                        .post(url)
                        .header("accept", "application/json, text/plain, */*")
                        .header("content-type", "application/json; charset=UTF-8")
                        .json(payload),
                )
            })
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

    pub async fn post_raw(&self, url: &str, body: &str) -> Result<(reqwest::StatusCode, String)> {
        self.post_raw_with_auth(url, body, None).await
    }

    /// 与 post_raw 相同，但可用 authorization 覆盖默认头（讨论区 BBS 令牌回退使用）。
    pub async fn post_raw_with_auth(
        &self,
        url: &str,
        body: &str,
        authorization: Option<&str>,
    ) -> Result<(reqwest::StatusCode, String)> {
        let build = || {
            let mut rb = self
                .client
                .post(url)
                .header("accept", "application/json, text/plain, */*")
                .header("content-type", "application/json; charset=UTF-8")
                .body(body.to_string());
            rb = self.apply_auth_common(rb);
            if let Some(auth) = authorization {
                if let Ok(value) = reqwest::header::HeaderValue::from_str(auth) {
                    rb = rb.header("authorization", value);
                }
            } else {
                rb = self.apply_auth_header_only(rb);
            }
            rb
        };
        // 指定 authorization 覆盖时由调用方（BBS）自行处理 401 重试，避免重复刷新
        let resp = if authorization.is_some() {
            build().send().await?
        } else {
            self.send_with_retry(build).await?
        };
        let status = resp.status();
        let text = resp.text().await?;
        Ok((status, text))
    }

    /// 注入 cookie 与 openid 相关头。
    fn apply_auth_common(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let a = self.auth.read().map(|a| a.clone()).unwrap_or_default();
        let cookie = if a.cookie.is_empty() {
            self.cfg.cookie.clone()
        } else {
            a.cookie
        };
        let open_id = if a.open_id.is_empty() {
            self.cfg.open_id.clone()
        } else {
            a.open_id
        };
        let annotator = if a.annotator.is_empty() {
            self.cfg.x_annotator_auth_token.clone()
        } else {
            a.annotator
        };
        let mut rb = rb;
        if !cookie.is_empty()
            && let Ok(v) = reqwest::header::HeaderValue::from_str(&cookie)
        {
            rb = rb.header("cookie", v);
        }
        if !open_id.is_empty()
            && let Ok(v) = reqwest::header::HeaderValue::from_str(&open_id)
        {
            rb = rb.header("u-openid", v.clone()).header("x-csrftoken", v);
        }
        if !annotator.is_empty()
            && let Ok(v) = reqwest::header::HeaderValue::from_str(&annotator)
        {
            rb = rb.header("x-annotator-auth-token", v);
        }
        rb
    }

    /// 注入 authorization 头（取共享认证状态，回退配置）。
    fn apply_auth_header_only(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let a = self.auth.read().map(|a| a.clone()).unwrap_or_default();
        let jwt = if !a.jwt.is_empty() {
            a.jwt
        } else if let Some(j) = self.cfg.cookie_jwt() {
            j
        } else {
            self.cfg.authorization.clone()
        };
        if jwt.is_empty() {
            return rb;
        }
        match reqwest::header::HeaderValue::from_str(&jwt) {
            Ok(v) => rb.header("authorization", v),
            Err(_) => rb,
        }
    }

    /// cookie + openid + authorization 全套认证头。
    fn apply_auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let rb = self.apply_auth_common(rb);
        self.apply_auth_header_only(rb)
    }

    /// 发送请求；遇 401 时按阶梯自愈：jwt 刷新重试 → annotator 重提取重签重试。
    async fn send_with_retry(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        let mut resp = build().send().await.context("请求发送失败")?;
        if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(resp);
        }
        // 第一级：刷新登录凭证（已新鲜时内部短路返回 false）
        log::warn!("接口返回 401，尝试自动刷新登录…");
        if self.refresh_auth().await.unwrap_or(false) {
            resp = build().send().await.context("刷新后重试请求失败")?;
            if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
                return Ok(resp);
            }
        }
        // 第二级：重新提取参数并重签 annotator token
        log::warn!("仍为 401，尝试从 bundle 重新提取并重签 x-annotator-auth-token…");
        if self.refresh_annotator().await.unwrap_or(false) {
            resp = build().send().await.context("annotator 重签后重试请求失败")?;
        }
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            log::warn!(
                "凭证均有效但仍 401：annotator 签名算法/字段可能已变更；\
                 可从浏览器复制最新 x-annotator-auth-token 到 config.json 兜底（有效值不会被覆盖）"
            );
        }
        Ok(resp)
    }

    /// 单飞重提取并重签 annotator token（仅 401 或需重签时调用，含网络请求）。
    /// 成功写回 config.json 并更新共享状态；返回是否有变化。
    pub async fn refresh_annotator(&self) -> Result<bool> {
        let _guard = self.annotator_lock.lock().await;
        let mut cfg = self.cfg.clone();
        // 合并共享状态中的最新认证信息，避免写回时覆盖刚刷新的 cookie
        let live = self.auth_snapshot();
        if !live.open_id.is_empty() {
            cfg.open_id = live.open_id.clone();
        }
        let changed = login::renew_annotator(&mut cfg).await;
        if !changed {
            return Ok(false);
        }
        if !live.cookie.is_empty() {
            cfg.cookie = live.cookie.clone();
        }
        if !live.refresh_token.is_empty() {
            cfg.refresh_token = live.refresh_token.clone();
        }
        if live.jwt_expire > 0 {
            cfg.jwt_expire = live.jwt_expire;
        }
        if live.rt_expire > 0 {
            cfg.rt_expire = live.rt_expire;
        }
        {
            let mut a = self.auth.write().unwrap_or_else(|e| e.into_inner());
            a.annotator = cfg.x_annotator_auth_token.clone();
            if !cfg.open_id.is_empty() {
                a.open_id = cfg.open_id.clone();
            }
        }
        cfg.save(&self.config_path).context("写回 annotator 配置失败")?;
        Ok(true)
    }

    /// 单飞刷新登录凭证（refresh_token 优先，其次账号密码）；成功写回 config.json。
    /// 返回是否执行了刷新。
    pub async fn refresh_auth(&self) -> Result<bool> {
        let _guard = self.refresh_lock.lock().await;
        let cfg = self.cfg.clone();
        {
            let a = self.auth.read().unwrap_or_else(|e| e.into_inner());
            let jwt = if a.jwt.is_empty() {
                cfg.cookie_jwt().unwrap_or_default()
            } else {
                a.jwt.clone()
            };
            if !jwt.is_empty() && login::jwt_fresh_enough(&jwt) {
                return Ok(false);
            }
        }
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let (rt, rt_expire) = {
            let a = self.auth.read().unwrap_or_else(|e| e.into_inner());
            (
                if a.refresh_token.is_empty() {
                    cfg.refresh_token.clone()
                } else {
                    a.refresh_token.clone()
                },
                if a.rt_expire == 0 {
                    cfg.rt_expire
                } else {
                    a.rt_expire
                },
            )
        };
        let mut creds: Option<Credentials> = None;
        if !rt.is_empty() && (rt_expire == 0 || rt_expire > now) {
            match login::refresh(&rt).await {
                Ok(c) => creds = Some(c),
                Err(e) => log::warn!("refresh_token 刷新失败: {:#}", e),
            }
        }
        if creds.is_none() && !cfg.username.is_empty() && !cfg.password.is_empty() {
            creds = Some(
                login::login(&cfg.username, &cfg.password)
                    .await
                    .context("自动登录失败")?,
            );
        }
        let Some(c) = creds else {
            log::warn!("无可用刷新方式（缺 refresh_token/账号密码），请检查 config.json");
            return Ok(false);
        };
        {
            let mut a = self.auth.write().unwrap_or_else(|e| e.into_inner());
            a.jwt = c.jwt.clone();
            a.cookie = format!("jwt={}", c.jwt);
            if !c.open_id.is_empty() {
                a.open_id = c.open_id.clone();
            }
            a.refresh_token = c.refresh_token.clone();
            a.jwt_expire = c.jwt_expire;
            a.rt_expire = c.rt_expire;
        }
        let mut nc = cfg;
        nc.cookie = format!("jwt={}", c.jwt);
        nc.authorization.clear();
        nc.refresh_token = c.refresh_token;
        nc.jwt_expire = c.jwt_expire;
        nc.rt_expire = c.rt_expire;
        if !c.open_id.is_empty() {
            nc.open_id = c.open_id;
        }
        nc.save(&self.config_path).context("写回登录凭证失败")?;
        log::info!("登录凭证已自动刷新（jwt 过期时间 {}）", c.jwt_expire);
        Ok(true)
    }

    pub fn course_id(&self) -> &str {
        &self.cfg.course_id
    }

    /// 当前 open_id（优先共享认证状态，登录刷新后立即生效）。
    pub fn open_id(&self) -> String {
        let a = self.auth.read().map(|a| a.clone()).unwrap_or_default();
        if a.open_id.is_empty() {
            self.cfg.open_id.clone()
        } else {
            a.open_id
        }
    }

    /// BBS 可用 JWT 候选（authorization + 共享 jwt + cookie jwt），按 exp 从新到旧。
    pub fn jwt_candidates(&self) -> Vec<String> {
        let a = self.auth_snapshot();
        let mut list: Vec<String> = Vec::new();
        for t in [
            self.cfg.authorization.clone(),
            a.jwt,
            self.cfg.cookie_jwt().unwrap_or_default(),
        ] {
            if !t.is_empty() && !list.contains(&t) {
                list.push(t);
            }
        }
        list.sort_by_key(|t| std::cmp::Reverse(login::jwt_exp(t).unwrap_or(0)));
        list
    }

    pub fn publish_version(&self) -> &str {
        &self.cfg.publish_version
    }
}

fn check_code(v: &Value) -> Result<()> {
    match v.get("code").and_then(|c| c.as_i64()) {
        Some(code) if code != 0 => {
            let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
            anyhow::bail!("接口返回错误 code={} msg={}", code, msg)
        }
        _ => Ok(()),
    }
}

/// 构造请求头值；配置项含换行等控制字符（常见于手动粘贴 cookie）时返回 None 并告警。
/// 直接 `HeaderValue::from_str(..).unwrap()` 会让程序在启动时 panic。
/// 注意：http 允许值中出现 obs-text（≥0x80，如中文），只有控制字符才会被拒绝。
fn header_value(field: &str, value: &str) -> Option<reqwest::header::HeaderValue> {
    if value.is_empty() {
        return None;
    }
    match reqwest::header::HeaderValue::from_str(value) {
        Ok(v) => Some(v),
        Err(_) => {
            log::warn!(
                "config.json 的 {} 含换行等控制字符，已忽略对应请求头；请检查该配置项",
                field
            );
            None
        }
    }
}

fn build_client(cfg: &Config) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout.max(3)))
        .user_agent(default_ua())
        .default_headers(build_base_headers(cfg))
        .build()
        .context("构建 HTTP 客户端失败")
}

fn build_base_headers(cfg: &Config) -> reqwest::header::HeaderMap {
    let mut map = reqwest::header::HeaderMap::new();
    if let Some(v) = header_value("cookie", &cfg.cookie) {
        map.insert("cookie", v);
    }
    // authorization 为空时回退到 cookie 中的 jwt（浏览器两者是同一个 token）。
    let authorization = if cfg.authorization.is_empty() {
        cfg.cookie_jwt()
    } else {
        Some(cfg.authorization.clone())
    };
    if let Some(auth) = authorization
        && let Some(v) = header_value("authorization", &auth)
    {
        map.insert("authorization", v);
    }
    if let Some(v) = header_value("x_annotator_auth_token", &cfg.x_annotator_auth_token) {
        map.insert("x-annotator-auth-token", v);
    }
    // open_id 同时用于 u-openid 与 x-csrftoken（值相同）。
    if let Some(v) = header_value("open_id", &cfg.open_id) {
        map.insert("u-openid", v.clone());
        map.insert("x-csrftoken", v);
    }
    map.insert("u-app-id", reqwest::header::HeaderValue::from_static("39"));
    map.insert("u-platform", reqwest::header::HeaderValue::from_static("2"));
    if let Some(v) = header_value("u_school", &cfg.u_school) {
        map.insert("u-school", v);
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
    String::from("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36 Edg/151.0.0.0")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_headers_skip_invalid_values() {
        // 手动粘贴的 cookie 常带换行（控制字符），构造请求头失败时不能 panic。
        let mut cfg = Config::default();
        cfg.unipusai.cookie = "jwt=abc\nX".to_string();
        cfg.unipusai.authorization = "bad\nvalue".to_string();
        cfg.unipusai.open_id = "openid\n123".to_string();
        cfg.unipusai.u_school = "8320".to_string();
        cfg.unipusai.x_annotator_auth_token = "ok-token".to_string();
        let h = build_base_headers(&cfg);
        assert!(h.get("cookie").is_none(), "含换行的 cookie 应被跳过");
        assert!(
            h.get("authorization").is_none(),
            "含换行的 authorization 应被跳过"
        );
        assert!(h.get("u-openid").is_none(), "含换行的 open_id 应被跳过");
        assert!(h.get("x-csrftoken").is_none());
        // 合法值照常注入，固定头始终存在
        assert_eq!(h.get("u-school").unwrap().to_str().unwrap(), "8320");
        assert_eq!(
            h.get("x-annotator-auth-token").unwrap().to_str().unwrap(),
            "ok-token"
        );
        assert_eq!(h.get("u-app-id").unwrap().to_str().unwrap(), "39");
    }

    #[test]
    fn base_headers_keep_non_ascii_values() {
        // obs-text（≥0x80）是合法头值：中文只会在服务端被拒，不应在本地 panic。
        let mut cfg = Config::default();
        cfg.unipusai.open_id = "中文openid".to_string();
        let h = build_base_headers(&cfg);
        assert!(h.get("u-openid").is_some());
        assert!(h.get("x-csrftoken").is_some());
    }

    #[test]
    fn base_headers_accept_normal_values() {
        let mut cfg = Config::default();
        cfg.unipusai.cookie = "jwt=abc.def".to_string();
        cfg.unipusai.open_id = "openid123".to_string();
        let h = build_base_headers(&cfg);
        assert_eq!(h.get("cookie").unwrap().to_str().unwrap(), "jwt=abc.def");
        assert_eq!(h.get("u-openid").unwrap().to_str().unwrap(), "openid123");
        assert_eq!(h.get("x-csrftoken").unwrap().to_str().unwrap(), "openid123");
    }
}
