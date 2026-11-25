use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::Path;

/// 站点（网课平台）标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Site {
    /// U 校园 AI 版（原 UnipusAI）
    #[default]
    Unipusai,
    /// 学习通 / 超星
    Chaoxing,
    /// 知到 / 智慧树
    Zhihuishu,
}

impl Site {
    pub fn as_str(self) -> &'static str {
        match self {
            Site::Unipusai => "unipusai",
            Site::Chaoxing => "chaoxing",
            Site::Zhihuishu => "zhihuishu",
        }
    }
}

impl std::fmt::Display for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Site {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "unipusai" | "unipus" | "u" => Ok(Site::Unipusai),
            "chaoxing" | "cx" | "xuexitong" => Ok(Site::Chaoxing),
            "zhihuishu" | "zhs" | "zhidao" => Ok(Site::Zhihuishu),
            other => anyhow::bail!(
                "未知平台: {}（可选: unipusai / chaoxing / zhihuishu）",
                other
            ),
        }
    }
}

/// 顶层配置：
/// - 公共配置（模型、LLM、转写、间隔等）直接位于顶层；
/// - 各平台自己的配置放在同名子对象里（unipusai / chaoxing）。
///
/// 为了兼容旧版本扁平 config.json，`load` 会把旧字段自动迁移进 `unipusai`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 当前使用的平台（默认 unipusai，与旧配置兼容）。
    #[serde(default)]
    pub site: Site,

    // ===== 公共配置 =====
    #[serde(default)]
    pub timeout: u64,
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
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// 同一道题向大模型提问的次数（多数投票；1 表示不投票）。
    #[serde(default = "default_llm_votes")]
    pub llm_votes: u32,
    #[serde(default = "default_fallback_on_llm_failure")]
    pub fallback_on_llm_failure: bool,

    // ===== 平台配置 =====
    #[serde(default)]
    pub unipusai: UnipusConfig,
    #[serde(default)]
    pub chaoxing: ChaoxingConfig,
    /// 知到 / 智慧树
    #[serde(default)]
    pub zhihuishu: ZhihuishuConfig,
}

/// U 校园 AI 版（原 UnipusAI）平台配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnipusConfig {
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
    pub learning_strategy: String,
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

impl Default for UnipusConfig {
    fn default() -> Self {
        Self {
            cookie: String::new(),
            authorization: String::new(),
            x_annotator_auth_token: String::new(),
            u_school: String::new(),
            course_id: String::new(),
            class_id: String::new(),
            curricula_id: String::new(),
            open_id: String::new(),
            publish_version: String::new(),
            learning_strategy: "learn_all_compulsory_course".to_string(),
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

/// 学习通 / 超星平台配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChaoxingConfig {
    /// 手机号 / 超星号。
    #[serde(default)]
    pub username: String,
    /// 密码（本地明文保存；登录时 AES-CBC 加密后提交）。
    #[serde(default)]
    pub password: String,
    /// 浏览器 Cookie 兜底（登录失败时可直接粘贴）。
    #[serde(default)]
    pub cookie: String,
    /// 课程 id（courseid）。
    #[serde(default)]
    pub course_id: String,
    /// 班级 id（clazzid）。
    #[serde(default)]
    pub clazz_id: String,
    /// cpi（课程页内容接口参数，可从课程页 URL 获取）。
    #[serde(default)]
    pub cpi: String,
    /// 讨论区 bbsid（可自动获取）。
    #[serde(default)]
    pub bbsid: String,
    /// 学生 userid（页面隐藏域 userId）。
    #[serde(default)]
    pub user_id: String,
    /// 学生端课程 enc。
    #[serde(default)]
    pub enc: String,
    /// studentcourse 的 stuenc。
    #[serde(default)]
    pub stuenc: String,
    /// 学生端全局 utEnc（studentstudy 页内联变量，测验链接需要）。
    #[serde(default)]
    pub utenc: String,
    /// 学校 fid / 课程 cfid。
    #[serde(default)]
    pub fid: String,
    #[serde(default)]
    pub cfid: String,
    /// 课程名（展示用，可自动填充）。
    #[serde(default)]
    pub course_name: String,
    /// 是否已完成登录（缓存标记，仅供展示）。
    #[serde(default)]
    pub logged_in: bool,
}

/// 知到（智慧树）配置。
///
/// 登录需要滑块验证，暂不支持纯 HTTP 账密登录；从浏览器复制 Cookie 使用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZhihuishuConfig {
    /// 手机号 / 用户名（首次登录用；登录态仍以 Cookie 维护）。
    #[serde(default)]
    pub username: String,
    /// 密码（仅用于自动填登录表单；验证码需人工完成）。
    #[serde(default)]
    pub password: String,
    /// 浏览器路径（留空自动找 Edge/Chrome）。
    #[serde(default)]
    pub browser_path: String,
    /// 浏览器 Cookie（需含 CASLOGC / zhs-jt-cas 等会话字段）。
    #[serde(default)]
    pub cookie: String,
    /// 课程 recruitId。
    #[serde(default)]
    pub recruit_id: i64,
    /// 课程 courseId。
    #[serde(default)]
    pub course_id: i64,
    /// 学习页 secret（`stuStudy?recruitAndCourseId=...` 的值）。
    #[serde(default)]
    pub recruit_and_course_id: String,
    /// 作业路由里的 schoolId（一般为 365）。
    #[serde(default = "default_zhs_school")]
    pub school_id: i64,
    /// 学习上报用的 uuid（抓包或 Cookie 里的 uuid）。
    #[serde(default)]
    pub uuid: String,
    /// 视频上报步长（秒），默认 60。
    #[serde(default = "default_zhs_step")]
    pub video_step: u64,
}

fn default_zhs_school() -> i64 {
    365
}

fn default_zhs_step() -> u64 {
    60
}

impl Default for ZhihuishuConfig {
    fn default() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            browser_path: String::new(),
            cookie: String::new(),
            recruit_id: 0,
            course_id: 0,
            recruit_and_course_id: String::new(),
            school_id: default_zhs_school(),
            uuid: String::new(),
            video_step: default_zhs_step(),
        }
    }
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

fn default_llm_votes() -> u32 {
    1
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
            site: Site::Unipusai,
            timeout: 10,
            interval_ms: default_interval_ms(),
            whisper_enabled: false,
            whisper_model: default_whisper_model(),
            whisper_language: default_whisper_language(),
            api_key: String::new(),
            base_url: "https://api.moonshot.cn/v1".to_string(),
            model: "kimi-k2-turbo-preview".to_string(),
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            llm_votes: default_llm_votes(),
            fallback_on_llm_failure: true,
            unipusai: UnipusConfig::default(),
            chaoxing: ChaoxingConfig::default(),
            zhihuishu: ZhihuishuConfig::default(),
        }
    }
}

/// 兼容旧代码：大量调用点仍以 `cfg.course_id` / `cfg.cookie` 形式访问平台字段。
/// 这些字段现在位于 `cfg.unipusai` 下，通过 Deref 保持原写法可用。
impl std::ops::Deref for Config {
    type Target = UnipusConfig;

    fn deref(&self) -> &Self::Target {
        &self.unipusai
    }
}

impl std::ops::DerefMut for Config {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.unipusai
    }
}

/// 旧版扁平 config.json 中属于 Unipus 平台的字段。
const LEGACY_UNIPUS_KEYS: &[&str] = &[
    "cookie",
    "authorization",
    "x_annotator_auth_token",
    "u_school",
    "course_id",
    "class_id",
    "curricula_id",
    "open_id",
    "publish_version",
    "learning_strategy",
    "username",
    "password",
    "refresh_token",
    "jwt_expire",
    "rt_expire",
    "annotator_key",
    "annotator_iss",
    "annotator_aud",
    "annotator_ttl_ms",
];

/// 把旧版扁平配置迁移为新结构：平台字段移入 `unipusai`。
fn migrate_legacy(value: &mut Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    if obj.contains_key("unipusai") {
        return;
    }
    let mut unipus = Map::new();
    for key in LEGACY_UNIPUS_KEYS {
        if let Some(v) = obj.remove(*key) {
            unipus.insert((*key).to_string(), v);
        }
    }
    if !unipus.is_empty() {
        obj.insert("unipusai".to_string(), Value::Object(unipus));
    }
}

impl Config {
    /// 旧字段写法仍可用（经 Deref 指向 unipusai）。
    pub fn compulsory_only(&self) -> bool {
        let s = self.unipusai.learning_strategy.trim();
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
        let mut value: Value =
            serde_json::from_str(&text).with_context(|| "解析配置失败，请检查 config.json 格式")?;
        migrate_legacy(&mut value);
        let mut cfg: Config =
            serde_json::from_value(value).with_context(|| "配置结构不正确（请参考 config.example.json）")?;
        // 环境变量覆盖（便于临时测试，不落盘）：
        // CHAOXING_USERNAME / CHAOXING_PASSWORD / CHAOXING_COOKIE
        if let Ok(v) = std::env::var("CHAOXING_USERNAME") {
            cfg.chaoxing.username = v;
        }
        if let Ok(v) = std::env::var("CHAOXING_PASSWORD") {
            cfg.chaoxing.password = v;
        }
        if let Ok(v) = std::env::var("CHAOXING_COOKIE") {
            cfg.chaoxing.cookie = v;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// 校验当前选中平台的登录方式是否可用。
    pub fn validate(&self) -> Result<()> {
        if self.llm_votes == 0 || self.llm_votes > 9 {
            anyhow::bail!("config.llm_votes 需在 1~9 之间（当前 {}）", self.llm_votes);
        }
        match self.site {
            Site::Unipusai => {
                let u = &self.unipusai;
                let auto = !u.username.is_empty() && !u.password.is_empty();
                let cookie_ok = !u.cookie.is_empty()
                    && (cookie_jwt_from(&u.cookie).is_some() || !u.authorization.is_empty());
                let rt_ok = !u.refresh_token.is_empty();
                if !auto && !cookie_ok && !rt_ok {
                    anyhow::bail!(
                        "config.unipusai：请填写 username/password（推荐，自动登录），或重新从浏览器复制 cookie（含 jwt=）"
                    );
                }
            }
            Site::Chaoxing => {
                let c = &self.chaoxing;
                let auto = !c.username.is_empty() && !c.password.is_empty();
                if !auto && c.cookie.is_empty() {
                    anyhow::bail!(
                        "config.chaoxing：请填写 username/password（手机号+密码），或粘贴浏览器 Cookie"
                    );
                }
            }
            Site::Zhihuishu => {
                let z = &self.zhihuishu;
                let can_browser_login =
                    !z.username.is_empty() && !z.password.is_empty();
                if z.cookie.is_empty() && !can_browser_login {
                    anyhow::bail!(
                        "config.zhihuishu：请填写 username/password（首次登录会打开浏览器让你过验证码），或粘贴已登录的 Cookie"
                    );
                }
                if z.recruit_id == 0 || z.course_id == 0 {
                    anyhow::bail!("config.zhihuishu：请填写 recruit_id / course_id");
                }
            }
        }
        Ok(())
    }

    /// 从 cookie 中提取 `jwt=` 值（与浏览器 Authorization 头一致）。
    pub fn cookie_jwt(&self) -> Option<String> {
        cookie_jwt_from(&self.unipusai.cookie)
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
        let mut cfg = Config::default();
        cfg.unipusai.cookie = "a=1; jwt=tok123; b=2".into();
        cfg.unipusai.course_id = "course-v2:x".into();
        cfg.unipusai.open_id = "open".into();
        cfg
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
    fn llm_votes_rejects_out_of_range() {
        let mut cfg = base_config();
        cfg.llm_votes = 0;
        assert!(cfg.validate().is_err());
        cfg.llm_votes = 10;
        assert!(cfg.validate().is_err());
        cfg.llm_votes = 3;
        assert!(cfg.validate().is_ok());
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

    #[test]
    fn legacy_flat_config_migrates_into_unipusai() {
        let mut v: serde_json::Value = serde_json::json!({
            "timeout": 10,
            "api_key": "k",
            "base_url": "https://api.deepseek.com",
            "course_id": "c1",
            "cookie": "jwt=x",
            "open_id": "o1",
            "learning_strategy": "learn_all_compusory_course",
            "username": "u",
            "password": "p"
        });
        migrate_legacy(&mut v);
        let cfg: Config = serde_json::from_value(v).unwrap();
        assert_eq!(cfg.site, Site::Unipusai);
        assert_eq!(cfg.api_key, "k");
        assert_eq!(cfg.unipusai.course_id, "c1");
        assert_eq!(cfg.unipusai.open_id, "o1");
        assert!(cfg.compulsory_only());
        // 顶层不再保留平台字段
        let s = serde_json::to_value(&cfg).unwrap();
        assert!(s.get("course_id").is_none());
        assert!(s.get("unipusai").is_some());
        assert!(s.get("chaoxing").is_some());
    }

    #[test]
    fn chaoxing_validate() {
        let mut cfg = Config {
            site: Site::Chaoxing,
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
        cfg.chaoxing.username = "1".into();
        cfg.chaoxing.password = "2".into();
        assert!(cfg.validate().is_ok());
        cfg.chaoxing.username.clear();
        cfg.chaoxing.cookie = "a=1".into();
        assert!(cfg.validate().is_ok());
    }
}
