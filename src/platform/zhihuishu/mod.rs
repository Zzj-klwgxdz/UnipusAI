//! 知到（智慧树）客户端。
//!
//! 两套接口：
//! - 学习：`studyservice-api.zhihuishu.com`（AES，`secretStr`+`dateFormate`）
//! - 作业/考试：`studentexam-api.zhihuishu.com`（AES，只有 `secretStr`）
//!
//! 课程/章节来自 `learning/videolist`；作业列表来自 `student/getStudentHomework`。

/// 知到作业提交开关：服务端新增滑块验证，暂不可用。
pub const HOMEWORK_SUBMIT_ENABLED: bool = false;

/// 提交暂不可用时的统一提示。
pub const HOMEWORK_SUBMIT_DISABLED_MSG: &str =
    "知到作业提交暂不可用（服务端要求滑块验证，等待适配）；可继续刷视频、查看作业与已提交分数";

pub mod cdp;
pub mod crypto;
pub mod login;
pub mod session;
pub mod solve;

use crate::config::ZhihuishuConfig;
use anyhow::{Context, Result, bail};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{Value, json};
use session::ZsSession;

const LEARN_BASE: &str = "https://studyservice-api.zhihuishu.com/gateway/t/v1";
const EXAM_BASE: &str = "https://studentexam-api.zhihuishu.com/studentExam/gateway/t/v1";

const LEARN_REFERER: &str = "https://studyvideoh5.zhihuishu.com/";
const EXAM_REFERER: &str = "https://onlineexamh5new.zhihuishu.com/stuExamWeb.html";

/// 章节（含小节）。
#[derive(Debug, Clone)]
pub struct ZsChapter {
    pub id: i64,
    pub name: String,
    pub lessons: Vec<ZsLesson>,
}

#[derive(Debug, Clone)]
pub struct ZsLesson {
    pub id: i64,
    pub name: String,
    pub video_id: i64,
    pub sec: u64,
    pub chapter_id: i64,
}

/// 作业/测验。
#[derive(Debug, Clone)]
pub struct ZsHomework {
    pub id: String,
    pub exam_id: String,
    pub name: String,
    pub state: i64,
    pub score: Option<String>,
    pub chapter_id: i64,
    pub problem_num: i64,
    pub total_score: String,
}

#[derive(Debug, Clone)]
pub struct ZsOption {
    pub id: i64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ZsQuestion {
    pub eid: String,
    pub type_id: i64,
    pub type_name: String,
    pub text: String,
    pub options: Vec<ZsOption>,
    pub score: String,
}

#[derive(Debug, Clone)]
pub struct ZsPaper {
    pub exam_id: String,
    pub name: String,
    pub questions: Vec<ZsQuestion>,
}

/// 一题的作答（eid + 选项 id 列表）。
#[derive(Debug, Clone)]
pub struct ZsAnswer {
    pub eid: String,
    pub type_id: i64,
    pub option_ids: Vec<i64>,
}

pub struct ZhihuishuClient {
    cfg: ZhihuishuConfig,
    session: ZsSession,
}

impl ZhihuishuClient {
    pub fn new(cfg: &ZhihuishuConfig, timeout: u64) -> Result<Self> {
        let session = ZsSession::new(&cfg.cookie, timeout)?;
        Ok(Self {
            cfg: cfg.clone(),
            session,
        })
    }

    pub fn config(&self) -> &ZhihuishuConfig {
        &self.cfg
    }

    fn ensure_course(&self) -> Result<()> {
        if self.cfg.recruit_id == 0 || self.cfg.course_id == 0 {
            bail!("config.zhihuishu 缺少 recruit_id / course_id");
        }
        Ok(())
    }


    /// 章节 + 小节列表。
    pub async fn fetch_chapters(&self) -> Result<Vec<ZsChapter>> {
        self.ensure_course()?;
        if self.cfg.recruit_and_course_id.is_empty() {
            bail!("config.zhihuishu 缺少 recruit_and_course_id（学习页 URL 的 secret）");
        }
        let form = crypto::learn_form(
            &json!({
                "recruitAndCourseId": self.cfg.recruit_and_course_id,
                "dateFormate": unix_ms(),
            }),
            unix_ms(),
        );
        let url = format!("{LEARN_BASE}/learning/videolist");
        let v = self.session.post_form(&url, form, LEARN_REFERER).await?;
        if v.get("code").and_then(Value::as_i64) != Some(0) {
            bail!("videolist 失败: {}", v);
        }
        let chapters = v
            .pointer("/data/videoChapterDtos")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for c in chapters {
            let id = c
                .get("id")
                .and_then(as_i64)
                .or_else(|| c.get("chapterId").and_then(as_i64))
                .unwrap_or(0);
            let name = {
                let n = str_of(&c, "name");
                if n.is_empty() { str_of(&c, "chapterName") } else { n }
            };
            let mut lessons = Vec::new();
            for l in c
                .get("videoLessons")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let lesson_id = l.get("id").and_then(as_i64).unwrap_or(0);
                let chapter_id = l
                    .get("chapterId")
                    .and_then(as_i64)
                    .filter(|x| *x != 0)
                    .unwrap_or(id);
                lessons.push(ZsLesson {
                    id: lesson_id,
                    name: str_of(&l, "name"),
                    video_id: l.get("videoId").and_then(as_i64).unwrap_or(0),
                    sec: l.get("videoSec").and_then(as_i64).unwrap_or(0).max(0) as u64,
                    chapter_id,
                });
            }
            out.push(ZsChapter { id, name, lessons });
        }
        Ok(out)
    }

    /// 作业列表；flag=1 未提交，flag=2 已提交（带分数）。
    pub async fn fetch_homeworks(&self, flag: u8) -> Result<Vec<ZsHomework>> {
        self.ensure_course()?;
        let form = crypto::exam_form(&json!({
            "recruitId": self.cfg.recruit_id.to_string(),
            "courseId": self.cfg.course_id,
            "flag": flag,
            "pageSize": 100,
            "pageNum": 0,
        }));
        let url = format!("{EXAM_BASE}/student/getStudentHomework");
        let v = self.session.post_form(&url, form, EXAM_REFERER).await?;
        ok_rt(&v).with_context(|| format!("getStudentHomework: {v}"))?;
        let list = v
            .pointer("/rt/studentHomeworkList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for it in list {
            out.push(ZsHomework {
                id: str_of(&it, "id"),
                exam_id: str_of(&it, "examId"),
                name: str_of(&it, "examName"),
                state: it.get("state").and_then(as_i64).unwrap_or(0),
                score: it
                    .get("score")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                chapter_id: it.get("chapterId").and_then(as_i64).unwrap_or(0),
                problem_num: it.get("problemNum").and_then(as_i64).unwrap_or(0),
                total_score: str_of(&it, "totalScore"),
            });
        }
        Ok(out)
    }

    /// 取整卷。
    pub async fn fetch_paper(&self, hw: &ZsHomework) -> Result<ZsPaper> {
        self.ensure_course()?;
        let form = crypto::exam_form(&json!({
            "recruitId": self.cfg.recruit_id.to_string(),
            "examId": hw.exam_id,
            "studentExamId": hw.id,
            "schoolId": self.cfg.school_id.to_string(),
            "courseId": self.cfg.course_id.to_string(),
        }));
        let url = format!("{EXAM_BASE}/student/doHomework");
        let v = self.session.post_form(&url, form, EXAM_REFERER).await?;
        ok_rt(&v).with_context(|| format!("doHomework: {v}"))?;
        let base = v.pointer("/rt/examBase").cloned().unwrap_or_default();
        let mut questions = Vec::new();
        for part in base
            .get("workExamParts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            for q in part
                .get("questionDtos")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let qtype = q.get("questionType").cloned().unwrap_or_default();
                let mut options = Vec::new();
                for o in q
                    .get("questionOptions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    options.push(ZsOption {
                        id: o.get("id").and_then(as_i64).unwrap_or(0),
                        text: html_text(&str_of(&o, "content")),
                    });
                }
                questions.push(ZsQuestion {
                    eid: str_of(&q, "eid"),
                    type_id: qtype.get("id").and_then(as_i64).unwrap_or(0),
                    type_name: str_of(&qtype, "name"),
                    text: html_text(&str_of(&q, "name")),
                    options,
                    score: str_of(&q, "questionScore"),
                });
            }
        }
        Ok(ZsPaper {
            exam_id: str_of(&base, "id"),
            name: str_of(&base, "name"),
            questions,
        })
    }

    /// 保存一题答案（与页面「下一题/保存」等价）。
    pub async fn save_answer(&self, hw: &ZsHomework, a: &ZsAnswer) -> Result<()> {
        self.ensure_course()?;
        let answer = if a.option_ids.len() == 1 {
            json!(a.option_ids[0])
        } else {
            json!(a.option_ids)
        };
        let one = json!({
            "examId": hw.exam_id,
            "recruitId": self.cfg.recruit_id.to_string(),
            "stuExamId": hw.id,
            "eid": a.eid,
            "schoolId": self.cfg.school_id.to_string(),
            "deviceId": "",
            "examType": "",
            "fromType": 3,
            "answer": answer,
            "dataIds": "",
            "questionType": a.type_id,
        });
        let params = json!({
            "stuExamAnswer": serde_json::to_string(&json!([one]))?,
            "recruitId": self.cfg.recruit_id.to_string(),
        });
        let url = format!("{EXAM_BASE}/answer/saveStudentAnswer");
        let v = self
            .session
            .post_form(&url, crypto::exam_form(&params), EXAM_REFERER)
            .await?;
        let statu = v.pointer("/rt/statu").and_then(Value::as_str).unwrap_or("");
        if statu != "1" {
            bail!("saveStudentAnswer 失败: {v}");
        }
        Ok(())
    }

    /// 交卷，返回得分（响应里的 rt.score）。
    ///
    /// `answered` 为已作答题数（对应页面请求里的 `achieveCount`）。
    pub async fn submit(&self, hw: &ZsHomework, answered: usize) -> Result<Option<String>> {
        self.ensure_course()?;
        let params = json!({
            "recruitId": self.cfg.recruit_id.to_string(),
            "examId": hw.exam_id,
            "stuExamId": hw.id,
            "achieveCount": answered.to_string(),
        });
        let url = format!("{EXAM_BASE}/answer/submit");
        let v = self
            .session
            .post_form(&url, crypto::exam_form(&params), EXAM_REFERER)
            .await?;
        if v.get("status").and_then(Value::as_str) != Some("200") {
            bail!("submit 失败: {v}");
        }
        Ok(v.pointer("/rt/score")
            .and_then(Value::as_str)
            .map(ToString::to_string))
    }

    /// 小节学习进度（prelearningNote）。
    pub async fn lesson_progress(&self, lesson: &ZsLesson) -> Result<Value> {
        self.ensure_course()?;
        let form = crypto::learn_form(
            &json!({
                "ccCourseId": self.cfg.course_id,
                "chapterId": lesson.chapter_id,
                "isApply": 1,
                "lessonId": lesson.id,
                "recruitId": self.cfg.recruit_id,
                "videoId": lesson.video_id,
            }),
            unix_ms(),
        );
        let url = format!("{LEARN_BASE}/learning/prelearningNote");
        let v = self.session.post_form(&url, form, LEARN_REFERER).await?;
        if v.get("code").and_then(Value::as_i64) != Some(0) {
            bail!("prelearningNote 失败: {v}");
        }
        Ok(v)
    }

    /// 上报一次视频学时（saveDatabaseIntervalTimeV2）。
    ///
    /// `delta` 为本次新增秒数，`total` 为累计秒数。
    pub async fn report_video(
        &self,
        lesson: &ZsLesson,
        prelearn_id: i64,
        delta: u64,
        total: u64,
        position: u64,
    ) -> Result<Value> {
        self.ensure_course()?;
        let uuid = if self.cfg.uuid.is_empty() {
            bail!("config.zhihuishu 缺少 uuid（抓包/ Cookie 中的 uuid）");
        } else {
            self.cfg.uuid.as_str()
        };
        let plain = format!(
            "{};{};0;{};{};0;{};{};{};{}zhs",
            self.cfg.recruit_id,
            lesson.id,
            lesson.video_id,
            lesson.chapter_id,
            delta,
            total,
            hms(position),
            uuid
        );
        let sdsew = xor_hex(&plain, b"zzpttjd");
        let zwsds = STANDARD.encode(prelearn_id.to_string());
        let form = crypto::learn_form(
            &json!({
                "ewssw": "",
                "sdsew": sdsew,
                "zwsds": zwsds,
                "courseId": self.cfg.course_id,
            }),
            unix_ms(),
        );
        let url = format!("{LEARN_BASE}/learning/saveDatabaseIntervalTimeV2");
        self.session.post_form(&url, form, LEARN_REFERER).await
    }
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn str_of(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn ok_rt(v: &Value) -> Result<()> {
    match v.get("status").and_then(Value::as_str) {
        Some("200") => Ok(()),
        _ => bail!("接口返回异常"),
    }
}

fn html_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn hms(sec: u64) -> String {
    format!("{:02}:{:02}:{:02}", sec / 3600, (sec % 3600) / 60, sec % 60)
}

fn xor_hex(plain: &str, key: &[u8]) -> String {
    let mut out = String::with_capacity(plain.len() * 2);
    for (i, b) in plain.bytes().enumerate() {
        out.push_str(&format!("{:02x}", b ^ key[i % key.len()]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdsew_matches_captured_plain() {
        // 抓包解密后的明文：467029;1000246588;0;43558543;1000094966;0;0;832;00:00:00;Xe6OpKN2zhs
        let plain = "467029;1000246588;0;43558543;1000094966;0;0;832;00:00:00;Xe6OpKN2zhs";
        let hex = xor_hex(plain, b"zzpttjd");
        assert_eq!(
            hex,
            "4e4c474446535f4b4a4044465e524f42484f445150494f454c415e57414b4044445a5d4e4346424f5a5f4a4148474651544a4040444e5a54412215423b1a2f34480a1c07"
        );
    }

    #[test]
    fn hms_formats() {
        assert_eq!(hms(0), "00:00:00");
        assert_eq!(hms(832), "00:13:52");
    }

    #[test]
    fn html_text_strips_tags() {
        assert_eq!(html_text("<p>企业资源计划</p>"), "企业资源计划");
    }
}
