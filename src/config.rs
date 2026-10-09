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
        if self.cookie.is_empty() {
            anyhow::bail!("config：cookie 为空，请从浏览器复制");
        }
        if self.cookie_jwt().is_none() && self.authorization.is_empty() {
            anyhow::bail!(
                "config：cookie 中没有 jwt= 且 authorization 为空，至少需要其一（推荐只填 cookie）"
            );
        }
        if self.course_id.is_empty() {
            anyhow::bail!("config：course_id 为空，例如 course-v2:...");
        }
        if self.open_id.is_empty() {
            anyhow::bail!("config：open_id 为空");
        }
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
