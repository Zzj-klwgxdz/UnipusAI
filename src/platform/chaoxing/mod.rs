//! 学习通 / 超星客户端。

pub mod cli;
pub mod course;
pub mod font;
pub mod login;
pub mod quiz;
pub mod solve;
pub mod session;
pub mod video;

use crate::config::{ChaoxingConfig, Config};
use crate::platform::types::{Chapter, Course, TaskPoint};
use anyhow::Result;
use std::path::PathBuf;

pub use session::Session;

const VISIT_INTERACTION: &str = "https://mooc1-1.chaoxing.com/visit/interaction";

/// 学习通客户端：持有会话、配置，并提供平台操作。
pub struct ChaoxingClient {
    pub session: Session,
    pub config: Config,
    pub config_path: PathBuf,
}

impl ChaoxingClient {
    pub fn new(config: Config, config_path: PathBuf) -> Result<Self> {
        let timeout = config.timeout.max(5);
        let session = Session::new(timeout, &config.chaoxing.cookie)?;
        Ok(Self {
            session,
            config,
            config_path,
        })
    }

    pub fn chaoxing(&self) -> &ChaoxingConfig {
        &self.config.chaoxing
    }

    fn chaoxing_mut(&mut self) -> &mut ChaoxingConfig {
        &mut self.config.chaoxing
    }

    /// 保存配置（把登录后的 cookie 写回 config.json）。
    pub fn save_config(&mut self) -> Result<()> {
        self.config.save(&self.config_path)?;
        Ok(())
    }

    /// 判断当前页面是否落到登录页。
    fn looks_logged_out(html: &str) -> bool {
        html.contains("passport2.chaoxing.com")
            || html.contains("用户登录")
            || html.contains("请先登录")
            || html.contains("login.html")
    }

    /// 确保已登录：先用现有 cookie 探测，失效则用账密登录。
    pub async fn ensure_login(&mut self) -> Result<()> {
        if self.session.has_cookies() {
            let html = self.session.get_text(VISIT_INTERACTION).await?;
            if !Self::looks_logged_out(&html) && html.contains("stucoursemiddle") {
                if !self.chaoxing().logged_in {
                    self.chaoxing_mut().logged_in = true;
                }
                return Ok(());
            }
        }
        let (u, p) = {
            let c = self.chaoxing();
            (c.username.clone(), c.password.clone())
        };
        if u.is_empty() || p.is_empty() {
            anyhow::bail!("学习通未登录且没有配置 username/password，请重新登录或更新 cookie");
        }
        let timeout = self.config.timeout.max(5);
        let (session, outcome) = login::login(&u, &p, timeout).await?;
        self.session = session;
        let cookie = self.session.cookie_string();
        {
            let c = self.chaoxing_mut();
            c.cookie = cookie;
            c.logged_in = true;
        }
        let _ = outcome;
        self.save_config()?;
        Ok(())
    }

    /// 拉取课程列表。
    pub async fn fetch_courses(&self) -> Result<Vec<Course>> {
        course::fetch_courses(&self.session).await
    }

    /// 拉取章节列表。
    pub async fn fetch_chapters(&self, course: &Course) -> Result<Vec<Chapter>> {
        let clazz = course.extra_str("clazz_id");
        let cpi = course.extra_str("cpi");
        course::fetch_chapters(&self.session, &course.course_id, &clazz, &cpi).await
    }

    /// 拉取任务点。
    pub async fn fetch_tasks(&self, course: &Course, chapter: &Chapter) -> Result<Vec<TaskPoint>> {
        let clazz = course.extra_str("clazz_id");
        let cpi = course.extra_str("cpi");
        course::fetch_tasks(
            &self.session,
            &course.course_id,
            &clazz,
            &cpi,
            &chapter.id,
        )
        .await
    }

    /// 记住当前选择的课程（写回 config.json，供后续命令使用）。
    pub fn set_course(&mut self, course: &Course) -> Result<()> {
        {
            let c = self.chaoxing_mut();
            c.course_id = course.course_id.clone();
            c.clazz_id = course.extra_str("clazz_id");
            c.cpi = course.extra_str("cpi");
            c.course_name = course.name.clone();
        }
        self.save_config()
    }

    /// 从课程页读取用户 id（userId），用于测验提交。
    pub async fn ensure_user_id(&mut self) -> Result<String> {
        if !self.chaoxing().user_id.is_empty() {
            return Ok(self.chaoxing().user_id.clone());
        }
        let (course_id, clazz_id, cpi) = {
            let c = self.chaoxing();
            (c.course_id.clone(), c.clazz_id.clone(), c.cpi.clone())
        };
        if course_id.is_empty() || clazz_id.is_empty() {
            anyhow::bail!("尚未选择课程（course_id/clazz_id 为空）");
        }
        let url = course::course_page_url(&course_id, &clazz_id, &cpi);
        let html = self.session.get_text(&url).await?;
        let user_id = extract_hidden_value(&html, "userId")
            .or_else(|| extract_hidden_value(&html, "userId"))
            .unwrap_or_default();
        if user_id.is_empty() {
            anyhow::bail!("课程页未找到 userId，请检查登录态");
        }
        self.chaoxing_mut().user_id = user_id.clone();
        self.save_config()?;
        Ok(user_id)
    }

    /// 拉取某章节的卡片（含 mArg.defaults：cfid/userid/ktoken 等）。
    pub async fn fetch_cards(
        &self,
        course: &Course,
        chapter: &Chapter,
    ) -> Result<Vec<course::CardPage>> {
        course::fetch_cards(
            &self.session,
            &course.course_id,
            &course.extra_str("clazz_id"),
            &course.extra_str("cpi"),
            &chapter.id,
        )
        .await
    }

    /// 尝试拿到 utEnc（studentstudy 页内联的全局变量）。
    ///
    /// 实测 utenc 为空也能打开答题页，因此这里失败只返回空串，不阻断流程。
    pub async fn ensure_utenc(&mut self, course: &Course, chapter: &Chapter) -> Result<String> {
        if !self.chaoxing().utenc.is_empty() {
            return Ok(self.chaoxing().utenc.clone());
        }
        let (course_id, clazz_id, cpi, enc) = {
            let c = self.chaoxing();
            (
                c.course_id.clone(),
                c.clazz_id.clone(),
                c.cpi.clone(),
                c.enc.clone(),
            )
        };
        let cid = if course_id.is_empty() {
            course.course_id.clone()
        } else {
            course_id
        };
        let clz = if clazz_id.is_empty() {
            course.extra_str("clazz_id")
        } else {
            clazz_id
        };
        let url = format!(
            "https://mooc1.chaoxing.com/mycourse/studentstudy?chapterId={}&courseId={}&clazzid={}&cpi={}&enc={}&mooc2=1&hidetype=0",
            chapter.id, cid, clz, cpi, enc
        );
        let html = self.session.get_text(&url).await?;
        let re = regex::Regex::new(r#"var utEnc\s*=\s*"([0-9a-fA-F]+)""#)?;
        let utenc = re
            .captures(&html)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .unwrap_or_default();
        if !utenc.is_empty() {
            self.chaoxing_mut().utenc = utenc.clone();
            let _ = self.save_config();
        }
        Ok(utenc)
    }

    /// 打开某次章节测验的答题页（含字体解密）。
    pub async fn fetch_quiz_page(
        &mut self,
        course: &Course,
        chapter: &Chapter,
        work_id: &str,
    ) -> Result<quiz::QuizPage> {
        let cards = self.fetch_cards(course, chapter).await?;
        let mut enc = String::new();
        let mut jobid = String::new();
        let mut defaults = course::MargDefaults::default();
        for page in &cards {
            if defaults.ktoken.is_empty() {
                defaults = page.defaults.clone();
            }
            for a in &page.attachments {
                if a.id == work_id {
                    enc = a.extra_str("enc");
                    jobid = a.extra_str("jobid");
                }
            }
        }
        if enc.is_empty() {
            anyhow::bail!("未在卡片中找到测验 {}（或缺少 enc）", work_id);
        }
        let utenc = self.ensure_utenc(course, chapter).await.unwrap_or_default();
        let clazz = course.extra_str("clazz_id");
        let cpi = if defaults.cpi.is_empty() {
            course.extra_str("cpi")
        } else {
            defaults.cpi.clone()
        };
        let url = quiz::work_url(
            work_id,
            &jobid,
            &chapter.id,
            &defaults.ktoken,
            &cpi,
            &clazz,
            &enc,
            &utenc,
            &course.course_id,
        );
        quiz::fetch_page(&self.session, &url).await
    }

    /// 用 LLM 作答整份测验（返回 qid -> 提交值）。
    pub async fn solve_quiz(
        &self,
        page: &quiz::QuizPage,
    ) -> Result<std::collections::HashMap<String, String>> {
        solve::solve_with_llm(&self.config, page).await
    }

    /// 提交测验答案。
    pub async fn submit_quiz(
        &self,
        page: &quiz::QuizPage,
        answers: &std::collections::HashMap<String, String>,
    ) -> Result<serde_json::Value> {
        quiz::submit(&self.session, page, answers).await
    }

    /// 提交后取批阅页分数。
    pub async fn quiz_score(&self, response: &serde_json::Value) -> Result<Option<f64>> {
        quiz::fetch_score(&self.session, response).await
    }

}

/// 从 HTML 隐藏域取 `id="<name>" value="..."`。
pub fn extract_hidden_value(html: &str, name: &str) -> Option<String> {
    let pat = format!(r#"id="{}"[^>]*value="([^"]*)""#, name);
    let re = regex::Regex::new(&pat).ok()?;
    re.captures(html)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_hidden_value_works() {
        let html = r#"<input type="hidden" id="userId" value="413421686"><input id="workRelationId" value="57384638">"#;
        assert_eq!(extract_hidden_value(html, "userId").as_deref(), Some("413421686"));
        assert_eq!(
            extract_hidden_value(html, "workRelationId").as_deref(),
            Some("57384638")
        );
        assert!(extract_hidden_value(html, "none").is_none());
    }

    #[test]
    fn create_client_with_empty_cookie() {
        let cfg = Config {
            site: crate::config::Site::Chaoxing,
            ..Config::default()
        };
        let c = ChaoxingClient::new(cfg, PathBuf::from("config.json")).unwrap();
        assert!(!c.session.has_cookies());
    }
}
