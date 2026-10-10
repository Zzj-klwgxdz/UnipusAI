use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub timeout: u64,
    #[serde(default)]
    pub cookie: String,
    #[serde(default)]
    pub authorization: String,
    #[serde(default)]
    pub x_annotator_auth_token: String,
    #[serde(default)]
    pub u_school: String,
    #[serde(default)]
    pub course_id: String,
    /// 班级 id（页面 URL 的 cid），讨论题 BBS 接口使用。
    #[serde(default)]
    pub class_id: String,
    /// AI 版课程 id（页面 URL 的 cloudCurriculaId），讨论题 BBS 接口使用。
    #[serde(default)]
    pub curricula_id: String,
    #[serde(default)]
    pub open_id: String,
    #[serde(default)]
    pub publish_version: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub learning_strategy: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_fallback_on_llm_failure")]
    pub fallback_on_llm_failure: bool,
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u64,
    /// 是否启用本地语音(视频/音频)转写。
    #[serde(default)]
    pub whisper_enabled: bool,
    /// 转写用的 whisper 模型，如 tiny/base/small。
    #[serde(default = "default_whisper_model")]
    pub whisper_model: String,
    /// 转写语言，auto/空 表示自动检测，也可指定如 en / zh。
    #[serde(default = "default_whisper_language")]
    pub whisper_language: String,
    /// U校园通行证账号（手机号/邮箱），配置后自动登录。
    #[serde(default)]
    pub username: String,
    /// U校园通行证密码（本地明文保存）。
    #[serde(default)]
    pub password: String,
    /// 自动登录缓存的 refresh token。
    #[serde(default)]
    pub refresh_token: String,
    /// jwt 过期时间（Unix 秒，0=未知）。
    #[serde(default)]
    pub jwt_expire: i64,
    /// refresh token 过期时间（Unix 秒，0=未知）。
    #[serde(default)]
    pub rt_expire: i64,
    /// annotator token 签名密钥（从 ucontent bundle 动态提取；空=内置默认）。
    #[serde(default)]
    pub annotator_key: String,
    /// annotator token 的 iss 字段（空=内置默认）。
    #[serde(default)]
    pub annotator_iss: String,
    /// annotator token 的 aud 字段（空=内置默认）。
    #[serde(default)]
    pub annotator_aud: String,
    /// annotator token 有效期毫秒数（0=内置默认）。
    #[serde(default)]
    pub annotator_ttl_ms: u64,
}

fn default_whisper_model() -> String {
    "base".to_string()
}

fn default_whisper_language() -> String {
    "auto".to_string()
}

fn default_fallback_on_llm_failure() -> bool {
    true
}

fn default_max_tokens() -> u32 {
    2000
}

fn default_temperature() -> f32 {
    0.3
}

fn default_interval_ms() -> u64 {
    3000
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: 10,
            cookie: String::new(),
            authorization: String::new(),
            x_annotator_auth_token: String::new(),
            u_school: String::new(),
            course_id: String::new(),
            class_id: String::new(),
            curricula_id: String::new(),
            open_id: String::new(),
            publish_version: String::new(),
            api_key: String::new(),
            base_url: "https://api.moonshot.cn/v1".to_string(),
            model: "kimi-k2-turbo-preview".to_string(),
            learning_strategy: "learn_all_compulsory_course".to_string(),
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            fallback_on_llm_failure: true,
            interval_ms: default_interval_ms(),
            whisper_enabled: false,
            whisper_model: default_whisper_model(),
            whisper_language: default_whisper_language(),
            username: String::new(),
            password: String::new(),
            refresh_token: String::new(),
            jwt_expire: 0,
            rt_expire: 0,
            annotator_key: String::new(),
            annotator_iss: String::new(),
            annotator_aud: String::new(),
            annotator_ttl_ms: 0,
        }
    }
}

impl Config {
    pub fn compulsory_only(&self) -> bool {
        let s = self.learning_strategy.trim();
        s == "learn_all_compulsory_course" || s == "learn_all_compusory_course"
    }

    /// 只要有 api_key 就启用 LLM 答题。
    pub fn use_llm(&self) -> bool {
        !self.api_key.is_empty()
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            anyhow::bail!("配置不存在: {}", path.display());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("读取配置失败: {}", path.display()))?;
        let cfg: Config =
            serde_json::from_str(&text).with_context(|| "解析配置失败，请检查 config.json 格式")?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        let auto = !self.username.is_empty() && !self.password.is_empty();
        let cookie_ok =
            !self.cookie.is_empty() && (self.cookie_jwt().is_some() || !self.authorization.is_empty());
        let rt_ok = !self.refresh_token.is_empty();
        if !auto && !cookie_ok && !rt_ok {
            anyhow::bail!(
                "config：请填写 username/password（推荐，自动登录），或重新从浏览器复制 cookie（含 jwt=）"
            );
        }
        // 自动登录补全 course_id/open_id，具体命令在使用处（ensure_course）给出引导
        Ok(())
    }

    /// 从 cookie 中提取 `jwt=` 值（与浏览器 Authorization 头一致）。
    pub fn cookie_jwt(&self) -> Option<String> {
        cookie_jwt_from(&self.cookie)
    }
}

/// 从 cookie 字符串中提取 `jwt=` 的值。
pub fn cookie_jwt_from(cookie: &str) -> Option<String> {
    cookie.split(';').find_map(|part| {
        let part = part.trim();
        let value = part.strip_prefix("jwt=")?;
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> Config {
        Config {
            cookie: "a=1; jwt=tok123; b=2".into(),
            course_id: "course-v2:x".into(),
            open_id: "open".into(),
            ..Config::default()
        }
    }

    #[test]
    fn cookie_jwt_extract() {
        assert_eq!(
            cookie_jwt_from("a=1; jwt=tok123; b=2").as_deref(),
            Some("tok123")
        );
        assert_eq!(cookie_jwt_from("jwt=only").as_deref(), Some("only"));
        assert_eq!(cookie_jwt_from("jwt=; a=1"), None);
        assert_eq!(cookie_jwt_from("a=1"), None);
        assert_eq!(cookie_jwt_from(""), None);
    }

    #[test]
    fn validate_allows_empty_authorization_with_cookie_jwt() {
        let cfg = base_config();
        assert!(cfg.authorization.is_empty());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_requires_some_jwt() {
        let mut cfg = base_config();
        cfg.cookie = "a=1".into();
        assert!(cfg.validate().is_err());
        // 没有 cookie jwt 时保留 authorization 也可通过
        cfg.authorization = "tok".into();
        assert!(cfg.validate().is_ok());
    }
}
