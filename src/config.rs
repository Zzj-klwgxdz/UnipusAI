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
    /// 是否启用本地语音(视频/音频)转写，默认关闭。
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
    1200
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
        if self.authorization.is_empty() {
            anyhow::bail!("config：authorization(ucontent JWT) 为空");
        }
        if self.course_id.is_empty() {
            anyhow::bail!("config：course_id 为空，例如 course-v2:...");
        }
        if self.open_id.is_empty() {
            anyhow::bail!("config：open_id 为空");
        }
        Ok(())
    }
}
