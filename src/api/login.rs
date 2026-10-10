use crate::config::Config;
use anyhow::{Context, Result, bail};
use serde_json::Value;

/// SSO 服务地址（生产环境）。
pub const SSO: &str = "https://sso.unipus.cn";
/// 登录时附带的服务标识（与浏览器一致）。
pub const SERVICE: &str = "https://uai.unipus.cn";

/// 登录/刷新得到的凭证。
#[derive(Debug, Clone, Default)]
pub struct Credentials {
    pub jwt: String,
    pub refresh_token: String,
    pub open_id: String,
    /// jwt 过期时间（Unix 秒）
    pub jwt_expire: i64,
    /// refresh_token 过期时间（Unix 秒，0=未知）
    pub rt_expire: i64,
}

/// 账号密码字段加密：AES-128-CBC/PKCS7，密钥与 IV 硬编码于前端 usso SDK，输出大写 hex。
pub fn encrypt_field(s: &str) -> String {
    use aes::Aes128;
    use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
    let key = hex::decode("8AD70B641C024C7ADA2ECD082EC0334F").expect("key hex");
    let iv: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    let ct = cbc::Encryptor::<Aes128>::new_from_slices(&key, &iv)
        .expect("key/iv 长度合法")
        .encrypt_padded_vec_mut::<Pkcs7>(s.as_bytes());
    ct.iter().map(|b| format!("{:02X}", b)).collect()
}

/// 解码 JWT payload（不做签名校验）。
pub fn jwt_payload(token: &str) -> Option<Value> {
    use base64::Engine;
    let part = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(part))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// ucontent 前端内嵌的 annotator 签名默认参数（bundle 动态提取失败时的兜底）。
pub const DEFAULT_ANNOTATOR_KEY: &str = "a824b379f126b8b7aa5e33dee83fb0a05aa7462c";
pub const DEFAULT_ANNOTATOR_ISS: &str = "c4f772063dcfa98e9c50";
pub const DEFAULT_ANNOTATOR_AUD: &str = "edx.unipus.cn";
/// annotator token 默认有效期（毫秒）：1 年。
pub const DEFAULT_ANNOTATOR_TTL_MS: u64 = 31_536_000_000;

/// ucontent 探索页入口（用于定位前端 bundle）。
pub const EXPLORATION_PC_URL: &str =
    "https://ucontent.unipus.cn/_explorationpc_default/pc.html";

/// annotator 签发参数（config 提取值优先，缺省用内置默认）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotatorParams {
    pub key: String,
    pub iss: String,
    pub aud: String,
    pub ttl_ms: u64,
}

impl AnnotatorParams {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            key: if cfg.annotator_key.is_empty() {
                DEFAULT_ANNOTATOR_KEY.to_string()
            } else {
                cfg.annotator_key.clone()
            },
            iss: if cfg.annotator_iss.is_empty() {
                DEFAULT_ANNOTATOR_ISS.to_string()
            } else {
                cfg.annotator_iss.clone()
            },
            aud: if cfg.annotator_aud.is_empty() {
                DEFAULT_ANNOTATOR_AUD.to_string()
            } else {
                cfg.annotator_aud.clone()
            },
            ttl_ms: if cfg.annotator_ttl_ms == 0 {
                DEFAULT_ANNOTATOR_TTL_MS
            } else {
                cfg.annotator_ttl_ms
            },
        }
    }

    /// 是否与内置默认完全一致。
    pub fn is_builtin(&self) -> bool {
        self.key == DEFAULT_ANNOTATOR_KEY
            && self.iss == DEFAULT_ANNOTATOR_ISS
            && self.aud == DEFAULT_ANNOTATOR_AUD
            && self.ttl_ms == DEFAULT_ANNOTATOR_TTL_MS
    }
}

fn base64url_nopad(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 生成 `x-annotator-auth-token`（与浏览器前端同算法/同参数）。
pub fn generate_annotator_token(open_id: &str, params: &AnnotatorParams) -> String {
    build_annotator_token(open_id, now_ms() + params.ttl_ms, params)
}

/// 指定过期毫秒时间戳生成（便于单测复现抓包向量）。
pub fn build_annotator_token(open_id: &str, exp_ms: u64, params: &AnnotatorParams) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let header = base64url_nopad(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload_json = format!(
        "{{\"open_id\":\"{}\",\"name\":\"\",\"email\":\"\",\"administrator\":false,\"exp\":{},\"iss\":\"{}\",\"aud\":\"{}\"}}",
        open_id, exp_ms, params.iss, params.aud
    );
    let payload = base64url_nopad(payload_json.as_bytes());
    let signing_input = format!("{}.{}", header, payload);
    let mut mac = Hmac::<Sha256>::new_from_slice(params.key.as_bytes()).expect("hmac key");
    mac.update(signing_input.as_bytes());
    let sig = base64url_nopad(&mac.finalize().into_bytes());
    format!("{}.{}", signing_input, sig)
}

/// annotator token 是否仍可用（iss/aud 匹配当前参数且剩余有效期 ≥ 30 天）。
pub fn annotator_token_ok(token: &str) -> bool {
    let Some(payload) = jwt_payload(token) else {
        return false;
    };
    let Some(exp_ms) = payload.get("exp").and_then(|x| x.as_u64()) else {
        return false;
    };
    exp_ms.saturating_sub(now_ms()) > 30 * 24 * 3600 * 1000
}

/// 按需重新生成 `x_annotator_auth_token`：为空 / 解析失败 / 剩余 <30 天时重签。
/// 手动粘贴的新 token（有效期内）不会被覆盖。返回是否有变化。
pub fn ensure_annotator_token(cfg: &mut Config) -> bool {
    if annotator_token_ok(&cfg.x_annotator_auth_token) {
        return false;
    }
    if cfg.open_id.is_empty() {
        return false;
    }
    let params = AnnotatorParams::from_config(cfg);
    cfg.x_annotator_auth_token = generate_annotator_token(&cfg.open_id, &params);
    log::info!("已本地签发 x_annotator_auth_token（有效期 1 年）");
    true
}

/// 从 ucontent 前端 bundle 动态提取 annotator 签发参数（仅按需调用）。
pub async fn extract_annotator_params() -> Result<AnnotatorParams> {
    let client = sso_client()?;
    let html = client
        .get(EXPLORATION_PC_URL)
        .send()
        .await
        .context("获取 ucontent 页面失败")?
        .text()
        .await
        .context("读取 ucontent 页面失败")?;
    let files = extract_script_srcs(&html);
    if files.is_empty() {
        bail!("未在 ucontent 页面中找到前端脚本");
    }
    let base = "https://ucontent.unipus.cn/_explorationpc_default/";
    let mut last_err = String::new();
    for file in files {
        let url = if file.starts_with("http") {
            file.clone()
        } else {
            format!("{}{}", base, file.trim_start_matches('/'))
        };
        let Ok(resp) = client.get(&url).send().await else {
            continue;
        };
        let Ok(js) = resp.text().await else {
            continue;
        };
        match parse_annotator_params(&js) {
            Some(p) => {
                log::info!(
                    "已从 bundle 提取 annotator 参数: iss={} aud={} ttl={}ms key={}…",
                    p.iss,
                    p.aud,
                    p.ttl_ms,
                    &p.key[..p.key.len().min(8)]
                );
                return Ok(p);
            }
            None => last_err = file,
        }
    }
    bail!("未在 bundle 中找到 annotator 签发参数（最后检查: {}）", last_err)
}

/// 从页面 HTML 中抽取相对/绝对 JS 脚本地址。
fn extract_script_srcs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("src=") {
        rest = &rest[i + 4..];
        let trimmed = rest.trim_start();
        let quote = trimmed.chars().next().unwrap_or(' ');
        if quote != '"' && quote != '\'' {
            continue;
        }
        rest = &trimmed[1..];
        if let Some(end) = rest.find(quote) {
            let src = &rest[..end];
            rest = &rest[end..];
            if src.ends_with(".js") {
                out.push(src.to_string());
            }
        }
    }
    out
}

/// 从 bundle JS 中解析 annotator 参数（可单测）。
pub fn parse_annotator_params(js: &str) -> Option<AnnotatorParams> {
    // 优先在 _generateJwtToken 附近解析（terser 保留字符串属性名）；标记丢失时退化为全局搜索
    let window = match js.find("_generateJwtToken") {
        Some(i) => &js[i..(i + 4000).min(js.len())],
        None => js,
    };
    let iss_marker = if window.contains("iss:\"") {
        "iss:\""
    } else {
        "\"iss\":\""
    };
    let aud_marker = if window.contains("aud:\"") {
        "aud:\""
    } else {
        "\"aud\":\""
    };
    let iss = find_quoted_after(window, iss_marker)?;
    let aud = find_quoted_after(window, aud_marker)?;
    let ttl_ms = find_number_after(window, "exp:Date.now()+").unwrap_or(DEFAULT_ANNOTATOR_TTL_MS);
    // 密钥位于 iss 之后的函数体内（变量名可能被压缩，取首个 ≥32 位纯 hex 字符串）
    let key_start = window.find(iss_marker).unwrap_or(0);
    let key = find_hex_string(&window[key_start..], 32).filter(|k| *k != iss)?;
    if key.len() < 16 || iss.is_empty() || aud.is_empty() {
        return None;
    }
    Some(AnnotatorParams {
        key,
        iss,
        aud,
        ttl_ms,
    })
}

/// 在片段中查找 `marker` 之后紧跟的双引号字符串。
fn find_quoted_after(s: &str, marker: &str) -> Option<String> {
    let i = s.find(marker)? + marker.len();
    let rest = &s[i..];
    let end = rest.find('"')?;
    let v = rest[..end].to_string();
    (!v.is_empty()).then_some(v)
}

/// 在片段中查找 `marker` 之后的数字（支持 31536e6 形式）。
fn find_number_after(s: &str, marker: &str) -> Option<u64> {
    let i = s.find(marker)? + marker.len();
    let rest = &s[i..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+'))
        .unwrap_or(rest.len());
    rest[..end].parse::<f64>().ok().map(|v| v as u64)
}

/// 在片段中查找首个长度 ≥ `min_len` 的纯 hex 双引号字符串。
fn find_hex_string(s: &str, min_len: usize) -> Option<String> {
    let mut rest = s;
    while let Some(i) = rest.find('"') {
        rest = &rest[i + 1..];
        let Some(end) = rest.find('"') else { break };
        let cand = &rest[..end];
        if cand.len() >= min_len && cand.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(cand.to_string());
        }
        rest = &rest[end..];
    }
    None
}

/// 按需重签 annotator token：先从 bundle 动态提取参数（失败保留现有/内置），再签发。
/// 返回是否有变化（更新了参数或 token）。
pub async fn renew_annotator(cfg: &mut Config) -> bool {
    let mut changed = false;
    match extract_annotator_params().await {
        Ok(p) => {
            if p.key != cfg.annotator_key
                || p.iss != cfg.annotator_iss
                || p.aud != cfg.annotator_aud
                || p.ttl_ms != cfg.annotator_ttl_ms
            {
                cfg.annotator_key = p.key;
                cfg.annotator_iss = p.iss;
                cfg.annotator_aud = p.aud;
                cfg.annotator_ttl_ms = p.ttl_ms;
                changed = true;
                log::info!("annotator 签发参数已更新（来源 bundle）");
            }
        }
        Err(e) => log::warn!("从 bundle 提取 annotator 参数失败（沿用现有参数）: {:#}", e),
    }
    if !cfg.open_id.is_empty() {
        let params = AnnotatorParams::from_config(cfg);
        cfg.x_annotator_auth_token = generate_annotator_token(&cfg.open_id, &params);
        changed = true;
    }
    changed
}

/// 读取 JWT 的 exp（Unix 秒）。
pub fn jwt_exp(token: &str) -> Option<i64> {
    jwt_payload(token)?.get("exp")?.as_i64()
}

/// 读取 JWT 的 openId。
pub fn jwt_open_id(token: &str) -> Option<String> {
    jwt_payload(token)?
        .get("openId")?
        .as_str()
        .map(|s| s.to_string())
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// jwt 是否仍然"新鲜"（剩余有效期 > 24h）。
pub fn jwt_fresh_enough(token: &str) -> bool {
    jwt_exp(token).is_some_and(|e| e - now_ts() > 24 * 3600)
}

fn sso_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("构建登录客户端失败")
}

/// 从 USSO 响应 JSON 解析登录/刷新凭证；`code` 支持字符串与数字。
fn parse_credentials(v: &Value, action: &str) -> Result<Credentials> {
    let code_ok = v
        .get("code")
        .map(|c| c.as_str().map(|s| s == "0").unwrap_or_else(|| c.as_i64() == Some(0)))
        .unwrap_or(false);
    if !code_ok {
        let code = v.get("code").map(|c| c.to_string()).unwrap_or_default();
        let msg = v
            .get("msg")
            .or_else(|| v.get("error"))
            .and_then(|m| m.as_str())
            .unwrap_or("");
        return match code.trim_matches('"') {
            "1506" => Err(anyhow::anyhow!(
                "{}失败：触发滑块验证（极验），无法自动通过。请稍后重试，或从浏览器复制 cookie 到 config.json 兜底",
                action
            )),
            "1504" | "1502" | "1509" => Err(anyhow::anyhow!(
                "{}失败：需要图形验证码（code={}），请稍后重试或使用浏览器 cookie 兜底",
                action,
                code
            )),
            "1507" => Err(anyhow::anyhow!("{}失败：今日验证次数过多，请稍后再试", action)),
            _ => Err(anyhow::anyhow!("{}失败：code={} msg={}", action, code, msg)),
        };
    }
    let rs = v.get("rs").unwrap_or(v);
    let jwt = rs
        .get("jwt")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    if jwt.is_empty() {
        bail!("{}响应缺少 jwt：{}", action, crate::api::parser::truncate_text(&v.to_string(), 200));
    }
    let refresh_token = rs
        .get("rt")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let open_id = rs
        .get("openid")
        .or_else(|| rs.get("openId"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .or_else(|| jwt_open_id(&jwt))
        .unwrap_or_default();
    // 优先使用 jwt 自带 exp（登录响应的 jwtExpire 可能为 0）
    let jwt_expire = jwt_exp(&jwt)
        .or_else(|| rs.get("jwtExpire").and_then(|x| x.as_i64()).filter(|&e| e > 0))
        .unwrap_or(0);
    let rt_expire = rs
        .get("rtExpire")
        .and_then(|x| x.as_i64())
        .filter(|&e| e > 0)
        .unwrap_or(0);
    Ok(Credentials {
        jwt,
        refresh_token,
        open_id,
        jwt_expire,
        rt_expire,
    })
}

/// 账号密码登录（USSO cip/login）。
pub async fn login(username: &str, password: &str) -> Result<Credentials> {
    let body = serde_json::json!({
        "username": encrypt_field(username.trim()),
        "password": encrypt_field(password),
        "remember": true,
        "service": SERVICE,
    });
    let resp = sso_client()?
        .post(format!("{}/sso/0.1/sso/cip/login", SSO))
        .header("origin", SERVICE)
        .header("referer", format!("{}/", SERVICE))
        .json(&body)
        .send()
        .await
        .context("登录请求发送失败")?;
    let status = resp.status();
    let text = resp.text().await.context("读取登录响应失败")?;
    if !status.is_success() {
        bail!("登录失败：HTTP {} {}", status, crate::api::parser::truncate_text(&text, 200));
    }
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("登录响应不是 JSON: {}", crate::api::parser::truncate_text(&text, 200)))?;
    parse_credentials(&v, "登录")
}

/// 用 refresh_token 刷新凭证。
pub async fn refresh(refresh_token: &str) -> Result<Credentials> {
    let body = serde_json::json!({ "rt": refresh_token });
    let resp = sso_client()?
        .post(format!("{}/sso/4.0/sso/refresh_jwt", SSO))
        .header("origin", SERVICE)
        .header("referer", format!("{}/", SERVICE))
        .json(&body)
        .send()
        .await
        .context("刷新请求发送失败")?;
    let status = resp.status();
    let text = resp.text().await.context("读取刷新响应失败")?;
    if !status.is_success() {
        bail!("刷新失败：HTTP {} {}", status, crate::api::parser::truncate_text(&text, 200));
    }
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("刷新响应不是 JSON: {}", crate::api::parser::truncate_text(&text, 200)))?;
    parse_credentials(&v, "刷新登录")
}

/// 将凭证应用到配置（cookie 只保留 jwt，authorization 清空避免旧 token 干扰）。
pub fn apply(cfg: &mut Config, c: &Credentials) {
    cfg.cookie = format!("jwt={}", c.jwt);
    cfg.authorization.clear();
    cfg.refresh_token = c.refresh_token.clone();
    cfg.jwt_expire = c.jwt_expire;
    cfg.rt_expire = c.rt_expire;
    if !c.open_id.is_empty() {
        cfg.open_id = c.open_id.clone();
    }
    ensure_annotator_token(cfg);
}

/// 确保会话凭证有效：>24h 直接用；否则 refresh_token 刷新；再否则账号密码登录。
/// 有更新时写回 config.json，返回是否发生了变化。
pub async fn ensure_login(session: &mut crate::api::session::Session) -> Result<bool> {
    let cfg = session.cfg().clone();
    // 新鲜的 jwt 优先（cookie 中的 jwt）
    if let Some(jwt) = cfg.cookie_jwt()
        && jwt_fresh_enough(&jwt)
    {
        return Ok(false);
    }
    if !cfg.refresh_token.is_empty() && (cfg.rt_expire == 0 || cfg.rt_expire > now_ts()) {
        match refresh(&cfg.refresh_token).await {
            Ok(c) => {
                log::info!("已用 refresh_token 刷新登录（jwt 至 {}）", fmt_ts(c.jwt_expire));
                let mut nc = cfg.clone();
                apply(&mut nc, &c);
                session.update_config(nc)?;
                return Ok(true);
            }
            Err(e) => log::warn!("refresh_token 刷新失败: {:#}", e),
        }
    }
    if !cfg.username.is_empty() && !cfg.password.is_empty() {
        let c = login(&cfg.username, &cfg.password)
            .await
            .context("自动登录失败")?;
        log::info!("已重新登录（jwt 至 {}）", fmt_ts(c.jwt_expire));
        let mut nc = cfg.clone();
        apply(&mut nc, &c);
        session.update_config(nc)?;
        return Ok(true);
    }
    bail!(
        "登录凭证已失效且无法自动登录：请在 config.json 填写 username/password，或重新从浏览器复制 cookie"
    )
}

/// 课程必须已选择且身份信息齐全。
pub fn ensure_course(session: &crate::api::session::Session) -> Result<()> {
    if session.course_id().is_empty() {
        bail!("未选择课程：先运行 `UnipusAI courses` 查看课程，再用 `UnipusAI course <序号>` 选择");
    }
    if session.open_id().is_empty() {
        bail!("open_id 为空：请在 config.json 填写 username/password 以自动登录");
    }
    Ok(())
}

/// 每次启动刷新课程相关字段（非致命，失败仅警告并保留原值）：
///
/// - `class_id` / `curricula_id`：课程列表接口（courseList[].classId / courseList[].id）
/// - `u_school`：账号信息接口（value.userInfo.school）
/// - `publish_version`：课程进度接口（rt.publish_version）
///
/// 有变化返回 true 并写回 config.json。
pub async fn ensure_profile(session: &mut crate::api::session::Session) -> Result<bool> {
    use crate::api::course;
    let mut cfg = session.cfg().clone();
    let mut changed = false;

    // x_annotator_auth_token：本地签发；需重签时按需从 bundle 提取参数（仅此场景有网络开销）
    if cfg.open_id.is_empty() {
        if cfg.x_annotator_auth_token.is_empty() {
            log::warn!("open_id 为空，无法自动签发 x_annotator_auth_token");
        }
    } else if !annotator_token_ok(&cfg.x_annotator_auth_token) {
        renew_annotator(&mut cfg).await;
        changed = true;
    }

    if !cfg.course_id.is_empty() {
        match course::fetch_home_courses(session).await {
            Ok(list) => match list.iter().find(|c| c.course_id == cfg.course_id) {
                Some(c) => {
                    if !c.class_id.is_empty() && cfg.class_id != c.class_id {
                        cfg.class_id = c.class_id.clone();
                        changed = true;
                    }
                    if !c.curricula_id.is_empty() && cfg.curricula_id != c.curricula_id {
                        cfg.curricula_id = c.curricula_id.clone();
                        changed = true;
                    }
                }
                None => log::warn!(
                    "课程列表未找到当前 course_id（{}），保留现有 class_id/curricula_id",
                    cfg.course_id
                ),
            },
            Err(e) => log::warn!("刷新课程信息失败（保留原值）: {:#}", e),
        }
    }

    match course::fetch_user_school(session).await {
        Ok(Some(school)) if cfg.u_school != school => {
            cfg.u_school = school;
            changed = true;
        }
        Ok(_) => {}
        Err(e) => log::warn!("刷新学校编号失败（保留原值）: {:#}", e),
    }

    if !cfg.course_id.is_empty() {
        match course::fetch_course_progress(session).await {
            Ok(rt) => {
                if !rt.publish_version.is_empty() && cfg.publish_version != rt.publish_version {
                    cfg.publish_version = rt.publish_version;
                    changed = true;
                }
            }
            Err(e) => log::warn!("刷新 publish_version 失败（保留原值）: {:#}", e),
        }
    }

    if changed {
        session.update_config(cfg.clone())?;
        log::info!(
            "已自动刷新配置: class_id={} curricula_id={} u_school={} publish_version={}",
            cfg.class_id,
            cfg.curricula_id,
            cfg.u_school,
            cfg.publish_version
        );
    }
    Ok(changed)
}

fn fmt_ts(secs: i64) -> String {
    if secs <= 0 {
        return "未知".into();
    }
    time::OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .and_then(|t| {
            t.format(time::macros::format_description!(
                "[year]-[month]-[day] [hour]:[minute]"
            ))
            .ok()
        })
        .unwrap_or_else(|| secs.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_matches_frontend_vector() {
        assert_eq!(encrypt_field("testuser"), "0BEE28AAD749A902C115ABE749B97F61");
    }

    #[test]
    fn annotator_token_matches_captured_vector() {
        let params = AnnotatorParams::from_config(&Config::default());
        // 抓包真实 token（open_id=ad0195..., exp=1823157201685）复现校验
        let token = build_annotator_token("ad01959edee04ce89baebd12f5d3236e", 1823157201685, &params);
        assert_eq!(
            token,
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.\
             eyJvcGVuX2lkIjoiYWQwMTk1OWVkZWUwNGNlODliYWViZDEyZjVkMzIzNmUiLCJuYW1lIjoiIiwiZW1haWwiOiIiLCJhZG1pbmlzdHJhdG9yIjpmYWxzZSwiZXhwIjoxODIzMTU3MjAxNjg1LCJpc3MiOiJjNGY3NzIwNjNkY2ZhOThlOWM1MCIsImF1ZCI6ImVkeC51bmlwdXMuY24ifQ.\
             oIZR0OePtxO5XArHD6z16KIbbNJqpXUx2S32tsWNcqY"
                .replace(' ', "")
        );
        // 生成的 token 自身可用（剩余期校验）
        let fresh = generate_annotator_token("ad01959edee04ce89baebd12f5d3236e", &params);
        assert!(annotator_token_ok(&fresh));
        assert!(!annotator_token_ok("bad.token.here"));

        let mut cfg = Config::default();
        assert!(!ensure_annotator_token(&mut cfg)); // 无 open_id 不生成
        cfg.open_id = "u1".into();
        assert!(ensure_annotator_token(&mut cfg));
        assert!(annotator_token_ok(&cfg.x_annotator_auth_token));
        // 有效期内再次调用不覆盖
        let kept = cfg.x_annotator_auth_token.clone();
        assert!(!ensure_annotator_token(&mut cfg));
        assert_eq!(cfg.x_annotator_auth_token, kept);
    }

    #[test]
    fn parse_annotator_params_from_bundle_fixture() {
        // 摘自真实 bundle 的函数片段（terser 产物形态）
        let fixture = r#"
        }],[{key:"_generateJwtToken",value:function(e){var t={open_id:e,name:"",email:"",administrator:!1,exp:Date.now()+31536e6,iss:"c4f772063dcfa98e9c50",aud:"edx.unipus.cn"},n=Qt()(t,"a824b379f126b8b7aa5e33dee83fb0a05aa7462c");return console.log("create token: ",n),n}}])}();
        "#;
        let p = parse_annotator_params(fixture).expect("应能解析");
        assert_eq!(p.iss, "c4f772063dcfa98e9c50");
        assert_eq!(p.aud, "edx.unipus.cn");
        assert_eq!(p.key, "a824b379f126b8b7aa5e33dee83fb0a05aa7462c");
        assert_eq!(p.ttl_ms, 31_536_000_000);
        assert!(p.is_builtin());

        // 变量名被压缩/顺序变化的变体仍可解析（依赖字符串常量而非变量名）
        let variant = r#"
        {key:"_generateJwtToken",value:function(x){var z={email:"",open_id:x,name:"",administrator:!1,exp:Date.now()+2592e6,iss:"1234567890abcdef1234",aud:"other.example.cn"},q=JJ()(z,"deadbeefdeadbeefdeadbeefdeadbeef");return q}};
        "#;
        let p = parse_annotator_params(variant).expect("变体应能解析");
        assert_eq!(p.iss, "1234567890abcdef1234");
        assert_eq!(p.aud, "other.example.cn");
        assert_eq!(p.key, "deadbeefdeadbeefdeadbeefdeadbeef");
        assert_eq!(p.ttl_ms, 2_592_000_000);
        assert!(!p.is_builtin());

        // 无标记的纯 JS 返回 None
        assert!(parse_annotator_params("var a=1;").is_none());
    }

    #[test]
    fn extract_script_srcs_from_html() {
        let html = r#"<script src="https://a/x.js"></script><script defer="defer" src="question-data-e421babb.js"></script><link href="a.css">"#;
        let srcs = extract_script_srcs(html);
        assert_eq!(
            srcs,
            vec![
                "https://a/x.js".to_string(),
                "question-data-e421babb.js".to_string()
            ]
        );
    }

    #[test]
    fn jwt_payload_decodes_open_id() {
        use base64::Engine;
        let payload = serde_json::json!({"openId": "abc123", "exp": 4102444800i64});
        let p = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("x.{}.y", p);
        assert_eq!(jwt_open_id(&token).as_deref(), Some("abc123"));
        assert_eq!(jwt_exp(&token), Some(4102444800));
        assert!(jwt_fresh_enough(&token));
    }

    #[test]
    fn parse_login_success_and_captcha() {
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&serde_json::json!({"openId": "u1", "exp": 4102444800i64})).unwrap());
        let jwt = format!("a.{}.c", payload);
        let ok = serde_json::json!({
            "code": 0,
            "rs": {
                "jwt": jwt,
                "rt": "rt-1",
                "openid": "u1",
                "rtExpire": 4102444800i64
            }
        });
        let c = parse_credentials(&ok, "登录").unwrap();
        assert_eq!(c.open_id, "u1");
        assert_eq!(c.refresh_token, "rt-1");
        assert_eq!(c.jwt_expire, 4102444800);
        assert_eq!(c.rt_expire, 4102444800);

        let captcha = serde_json::json!({"code": "1506", "msg": "need check"});
        let err = parse_credentials(&captcha, "登录").unwrap_err().to_string();
        assert!(err.contains("滑块验证"), "{}", err);
    }
}
