//! 课程列表、章节树、任务点（mArg）。
//!
//! 学习通这些接口都返回 **HTML**，需要从页面里解析：
//! - 课程列表：`/visit/interaction` 的课程卡片链接（`stucoursemiddle?courseid&clazzid&cpi`）；
//! - 章节树：`/mooc-ans/mycourse/studentstudycourselist` 的 `posCatalog_select` 节点；
//! - 任务点：`/mooc-ans/knowledge/cards?num=N` 内嵌的 `mArg` JSON。

use crate::platform::chaoxing::session::Session;
use crate::platform::types::{Chapter, Course, TaskKind, TaskPoint};
use anyhow::{Result, bail};
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

const MOOC1: &str = "https://mooc1.chaoxing.com";
const MOOC1_1: &str = "https://mooc1-1.chaoxing.com";

static RE_STU_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<a[^>]*href="([^"]*stucoursemiddle\?[^"]*)"[^>]*>(.*?)</a>"#).unwrap()
});
static RE_ANY_STU_HREF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"stucoursemiddle\?courseid=(\d+)&clazzid=(\d+)(?:[^"']*)?"#).unwrap()
});
static RE_CPI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"cpi=(\d+)").unwrap());
static RE_FIRST_LAYER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)class="posCatalog_select firstLayer" id="(\d+)"[^>]*>.*?title="([^"]*)""#)
        .unwrap()
});
static RE_CUR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"id="cur(\d+)""#).unwrap());
static RE_CUR_TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"class="posCatalog_name" title="([^"]*)""#).unwrap());
static RE_SBAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"class="posCatalog_sbar">([^<]*)<"#).unwrap());
static RE_STUDYSTATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"id="_studystate" value="\{nextChapterId:(\d+),unfinishCount:(\d+)\}""#).unwrap()
});
static RE_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());

/// 拉取账号下的课程列表。
///
/// 课程列表是 AJAX 渲染的：先取 `/visit/interaction` 壳页（校验登录），
/// 再 POST `/visit/courselistdata`。`courseType=1` 是"超星班级"，`0` 是"我学的课"，
/// 两页都要请求（不同账号的课在不同分类里）。
pub async fn fetch_courses(session: &Session) -> Result<Vec<Course>> {
    let home = format!("{}/visit/interaction", MOOC1_1);
    let html = session.get_text(&home).await?;
    if !html.contains("courselistArea") && !html.contains("stucoursemiddle") {
        if html.contains("passport2") || html.contains("用户登录") || html.contains("请登录") {
            bail!("学习通登录已失效，请重新登录或更新 cookie");
        }
        bail!("课程列表解析失败（页面结构变化）");
    }
    let url = format!("{}/visit/courselistdata", MOOC1_1);
    let mut out: Vec<Course> = Vec::new();
    for course_type in ["1", "0"] {
        let form = vec![
            ("courseType".to_string(), course_type.to_string()),
            ("courseFolderId".to_string(), "0".to_string()),
            ("baseEducation".to_string(), "0".to_string()),
            ("superstarClass".to_string(), String::new()),
            ("courseFolderSize".to_string(), "0".to_string()),
        ];
        let page = match session.post_form_text(&url, form).await {
            Ok(p) => p,
            Err(_) => continue,
        };
        for c in parse_course_cards(&page) {
            if out
                .iter()
                .any(|x| x.course_id == c.course_id && x.extra_str("clazz_id") == c.extra_str("clazz_id"))
            {
                continue;
            }
            out.push(c);
        }
    }
    if out.is_empty() {
        out = parse_courses(&html);
    }
    if out.is_empty() {
        bail!("没有解析到课程（账号可能没有课程，或页面结构变化）");
    }
    Ok(out)
}

/// 解析课程 HTML（优先课程卡片，兜底旧版锚点链接）。
pub fn parse_courses(html: &str) -> Vec<Course> {
    let cards = parse_course_cards(html);
    if !cards.is_empty() {
        return cards;
    }
    let mut out: Vec<Course> = Vec::new();
    for caps in RE_STU_LINK.captures_iter(html) {
        let href = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let inner = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let Some(params) = RE_ANY_STU_HREF.captures(href) else {
            continue;
        };
        let course_id = params.get(1).unwrap().as_str().to_string();
        let clazz_id = params.get(2).unwrap().as_str().to_string();
        let cpi = RE_CPI
            .captures(href)
            .map(|c| c.get(1).unwrap().as_str().to_string())
            .unwrap_or_default();
        let name = clean_text(inner);
        if name.is_empty() {
            continue;
        }
        if out
            .iter()
            .any(|c| c.course_id == course_id && c.extra_str("clazz_id") == clazz_id)
        {
            continue;
        }
        out.push(Course {
            site: "chaoxing".to_string(),
            course_id,
            name,
            teacher: String::new(),
            extra: json!({ "clazz_id": clazz_id, "cpi": cpi, "url": href }),
        });
    }
    out
}

/// 解析课程卡片（`li.course` + `span.course-name`）。
pub fn parse_course_cards(html: &str) -> Vec<Course> {
    static RE_LI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<li class="course[^"]*"[^>]*courseId="(\d+)"[^>]*clazzId="(\d+)"[^>]*personId="(\d+)"[^>]*>"#).unwrap()
    });
    static RE_NAME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"class="course-name[^"]*"[^>]*title="([^"]*)""#).unwrap());
    static RE_SCHOOL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<p class="margint10 line2"[^>]*title="([^"]*)""#).unwrap());
    let heads: Vec<(usize, String, String, String)> = RE_LI
        .captures_iter(html)
        .filter_map(|c| {
            Some((
                c.get(0)?.end(),
                c.get(1)?.as_str().to_string(),
                c.get(2)?.as_str().to_string(),
                c.get(3)?.as_str().to_string(),
            ))
        })
        .collect();
    let mut out = Vec::new();
    for (idx, (pos, course_id, clazz_id, cpi)) in heads.iter().enumerate() {
        let end = heads.get(idx + 1).map(|(p, _, _, _)| *p).unwrap_or(html.len());
        // 下一张卡片的起始标签也在切片内，截掉它，避免把下一张的名称算进来
        let block_end = html[*pos..end]
            .rfind("<li class=\"course")
            .map(|i| *pos + i)
            .unwrap_or(end);
        let block = &html[*pos..block_end];
        let name = RE_NAME
            .captures(block)
            .map(|x| clean_text(x.get(1).unwrap().as_str()))
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        let teacher = RE_SCHOOL
            .captures(block)
            .map(|x| clean_text(x.get(1).unwrap().as_str()))
            .unwrap_or_default();
        out.push(Course {
            site: "chaoxing".to_string(),
            course_id: course_id.clone(),
            name,
            teacher,
            extra: json!({
                "clazz_id": clazz_id,
                "cpi": cpi,
            }),
        });
    }
    out
}

/// 拉取课程章节树。
pub async fn fetch_chapters(
    session: &Session,
    course_id: &str,
    clazz_id: &str,
    cpi: &str,
) -> Result<Vec<Chapter>> {
    let url = format!(
        "{}/mooc-ans/mycourse/studentstudycourselist?courseId={}&chapterId=&clazzid={}&cpi={}&mooc2=1&searchChapterListByName=",
        MOOC1, course_id, clazz_id, cpi
    );
    let html = session.get_text(&url).await?;
    if !html.contains("posCatalog") {
        bail!("章节列表解析失败（未找到 posCatalog 节点）");
    }
    Ok(parse_chapters(&html))
}

/// 从章节树 HTML 解析章节（含一级章节名/编号/完成状态）。
pub fn parse_chapters(html: &str) -> Vec<Chapter> {
    // 一级章节位置与标题
    let parents: Vec<(usize, String)> = RE_FIRST_LAYER
        .captures_iter(html)
        .filter_map(|c| {
            let pos = c.get(0)?.start();
            let title = clean_text(c.get(2)?.as_str());
            Some((pos, title))
        })
        .collect();

    let mut out = Vec::new();
    for caps in RE_CUR.captures_iter(html) {
        let pos = caps.get(0).map(|m| m.start()).unwrap_or(0);
        let id = caps.get(1).unwrap().as_str().to_string();
        let window_len: usize = html[pos..]
            .chars()
            .take(1200)
            .map(char::len_utf8)
            .sum();
        let window_end = pos + window_len;
        let window = &html[pos..window_end];
        let title = RE_CUR_TITLE
            .captures(window)
            .map(|c| clean_text(c.get(1).unwrap().as_str()))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let number = RE_SBAR
            .captures(window)
            .map(|c| clean_text(c.get(1).unwrap().as_str()))
            .unwrap_or_default();
        let completed = window.contains("icon_Completed");
        let parent_title = parents
            .iter()
            .rev()
            .find(|(p, _)| *p < pos)
            .map(|(_, t)| t.clone())
            .unwrap_or_default();
        out.push(Chapter {
            id,
            parent_title,
            title,
            number,
            completed,
            unfinished: None,
        });
    }
    out
}

/// 解析 `_studystate` 里的未完成任务点数（全课程）。
pub fn parse_unfinish_count(html: &str) -> Option<u32> {
    RE_STUDYSTATE
        .captures(html)
        .and_then(|c| c.get(2)?.as_str().parse().ok())
}

/// 任务点（mArg attachment）原始结构。
#[derive(Debug, Clone, Default)]
pub struct Attachment {
    pub kind: TaskKind,
    pub id: String,
    pub title: String,
    pub duration: Option<u64>,
    pub finished: bool,
    pub extra: Value,
}

impl Attachment {
    pub fn extra_str(&self, key: &str) -> String {
        match self.extra.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(v) if !v.is_null() => v.to_string(),
            _ => String::new(),
        }
    }
}

/// cards 页 `mArg.defaults` 里的课程上下文（视频/测验接口都要用）。
#[derive(Debug, Clone, Default)]
pub struct MargDefaults {
    pub fid: String,
    pub cfid: String,
    pub userid: String,
    pub cpi: String,
    pub clazzid: String,
    pub courseid: String,
    pub ktoken: String,
}

impl MargDefaults {
    pub fn from_marg(marg: &Value) -> Self {
        let d = marg.get("defaults").cloned().unwrap_or(Value::Null);
        let s = |k: &str| {
            d.get(k)
                .and_then(|v| {
                    v.as_str()
                        .map(|x| x.to_string())
                        .or_else(|| v.as_i64().map(|n| n.to_string()))
                })
                .unwrap_or_default()
        };
        Self {
            fid: s("fid"),
            cfid: s("cFid"),
            userid: s("userid"),
            cpi: s("cpi"),
            clazzid: s("clazzId"),
            courseid: s("courseId"),
            ktoken: s("ktoken"),
        }
    }
}

/// 一张卡片（= 一个 knowledge 下的一批任务点）。
#[derive(Debug, Clone)]
pub struct CardPage {
    pub defaults: MargDefaults,
    pub attachments: Vec<Attachment>,
}

/// 逐张拉取某章节的卡片，直到没有 mArg / 没有附件。
pub async fn fetch_cards(
    session: &Session,
    course_id: &str,
    clazz_id: &str,
    cpi: &str,
    knowledge_id: &str,
) -> Result<Vec<CardPage>> {
    let mut pages = Vec::new();
    for num in 0..10 {
        let url = format!(
            "{}/mooc-ans/knowledge/cards?clazzid={}&courseid={}&knowledgeid={}&num={}&ut=s&cpi={}&mooc2=1&isMicroCourse=false&editorPreview=0",
            MOOC1, clazz_id, course_id, knowledge_id, num, cpi
        );
        let html = session.get_text(&url).await?;
        let Some(marg) = extract_marg(&html) else {
            break;
        };
        let defaults = MargDefaults::from_marg(&marg);
        let attachments = parse_attachments(&marg);
        if attachments.is_empty() {
            break;
        }
        pages.push(CardPage {
            defaults,
            attachments,
        });
    }
    Ok(pages)
}

/// 拉取某章节的所有任务点（逐张卡片请求，直到没有 mArg）。
pub async fn fetch_tasks(
    session: &Session,
    course_id: &str,
    clazz_id: &str,
    cpi: &str,
    knowledge_id: &str,
) -> Result<Vec<TaskPoint>> {
    let mut out = Vec::new();
    for page in fetch_cards(session, course_id, clazz_id, cpi, knowledge_id).await? {
        for a in page.attachments {
            if out.iter().any(|t: &TaskPoint| t.id == a.id) {
                continue;
            }
            out.push(TaskPoint {
                id: a.id,
                kind: a.kind,
                title: a.title,
                duration: a.duration,
                finished: a.finished,
                extra: a.extra,
            });
        }
    }
    Ok(out)
}

/// 从 cards 页面提取 `mArg` 对象（括号配平，允许非严格 JSON）。
///
/// 页面里通常先出现 `mArg = ""` 再出现真正的 `mArg = {...}`，所以要按
/// `mArg\s*=\s*{` 定位并逐个尝试解析。
pub fn extract_marg(html: &str) -> Option<Value> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"mArg\s*=\s*\{").unwrap());
    for m in RE.find_iter(html) {
        let brace_pos = m.end() - 1;
        if let Some(v) = parse_object_at(html, brace_pos)
            && v.get("attachments").is_some()
        {
            return Some(v);
        }
    }
    None
}

/// 从 `{` 开始做括号配平并解析 JSON。
fn parse_object_at(html: &str, begin: usize) -> Option<Value> {
    let bytes = html.as_bytes();
    let mut i = begin;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
        } else if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return serde_json::from_str(&html[begin..=i]).ok();
            }
        }
        i += 1;
    }
    None
}

/// 把 mArg.attachments 转换成任务点。
pub fn parse_attachments(marg: &Value) -> Vec<Attachment> {
    let Some(list) = marg.get("attachments").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for a in list {
        let prop = a.get("property").cloned().unwrap_or(Value::Null);
        let ty = a.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let module = prop.get("module").and_then(|v| v.as_str()).unwrap_or("");
        let title = prop
            .get("name")
            .or_else(|| prop.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if module == "work" || ty == "workid" {
            let workid = prop
                .get("workid")
                .and_then(|v| v.as_str())
                .or_else(|| prop.get("workId").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            if workid.is_empty() {
                continue;
            }
            let enc = a.get("enc").and_then(|v| v.as_str()).unwrap_or("");
            out.push(Attachment {
                kind: TaskKind::Quiz,
                id: workid.clone(),
                title,
                duration: None,
                finished: false,
                extra: json!({
                    "workid": workid,
                    "jobid": a.get("jobid").and_then(|v| v.as_str()).unwrap_or(""),
                    "mid": a.get("mid").and_then(|v| v.as_str()).unwrap_or(""),
                    "enc": enc,
                    "worktype": prop.get("worktype").and_then(|v| v.as_str()).unwrap_or(""),
                }),
            });
            continue;
        }
        if ty == "video" || module == "insertvideo" {
            let object_id = a
                .get("objectId")
                .and_then(|v| v.as_str())
                .or_else(|| prop.get("objectid").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let jobid = a.get("jobid").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if object_id.is_empty() || jobid.is_empty() {
                continue;
            }
            out.push(Attachment {
                kind: TaskKind::Video,
                id: jobid.clone(),
                title,
                duration: a.get("attDuration").and_then(|v| v.as_u64()),
                finished: a.get("isPassed").and_then(|v| v.as_bool()).unwrap_or(false),
                extra: json!({
                    "objectId": object_id,
                    "jobid": jobid,
                    "mid": a.get("mid").and_then(|v| v.as_str()).unwrap_or(""),
                    "attDuration": a.get("attDuration").and_then(|v| v.as_i64()).unwrap_or(0),
                    "attDurationEnc": a.get("attDurationEnc").and_then(|v| v.as_str()).unwrap_or(""),
                    "videoFaceCaptureEnc": a.get("videoFaceCaptureEnc").and_then(|v| v.as_str()).unwrap_or(""),
                    "otherInfo": a.get("otherInfo").and_then(|v| v.as_str()).unwrap_or(""),
                }),
            });
            continue;
        }
        // 其它类型（文档/直播等）暂按 Other 记录
        let id = a
            .get("jobid")
            .and_then(|v| v.as_str())
            .or_else(|| prop.get("objectid").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        out.push(Attachment {
            kind: TaskKind::Other,
            id,
            title,
            duration: None,
            finished: a.get("isPassed").and_then(|v| v.as_bool()).unwrap_or(false),
            extra: json!({ "type": ty, "module": module }),
        });
    }
    out
}

/// 去掉 HTML 标签、压缩空白、解码常见实体。
pub fn clean_text(s: &str) -> String {
    let no_tags = RE_TAG.replace_all(s, " ");
    let decoded = no_tags
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 课程页 URL（用于取 userId/enc 等隐藏参数）。
pub fn course_page_url(course_id: &str, clazz_id: &str, cpi: &str) -> String {
    format!(
        "https://mooc2-ans.chaoxing.com/mooc2-ans/mycourse/stu?courseid={}&clazzid={}&cpi={}&pageHeader=1&v=2&hideHead=0",
        course_id, clazz_id, cpi
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_course_cards_real_markup() {
        let html = r#"<li class="course clearfix" courseId="1" clazzId="2" personId="3" id="course_1_2">
            <div class="course-cover"><a href="x"><img src="y"></a><ul class="hanlde-list"><li class="move-to">移动</li></ul></div>
            <div class="course-info"><h3 class="inlineBlock"><a class="color1" href="x"><span class="course-name overHidden2" title="机械与电气实验室安全">机械与电气实验室安全</span></a></h3>
            <p class="margint10 line2" title="南京农业大学">南京农业大学</p></div></li>"#;
        let list = parse_course_cards(html);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "机械与电气实验室安全");
        assert_eq!(list[0].teacher, "南京农业大学");
    }

    #[test]
    fn parse_courses_from_html() {
        let html = r#"<a href="https://mooc1-1.chaoxing.com/mooc-ans/visit/stucoursemiddle?courseid=1&clazzid=2&vc=1&cpi=3&ismooc2=1&v=2"><span>测试课程</span></a>
        <a href="https://mooc1-1.chaoxing.com/mooc-ans/visit/stucoursemiddle?courseid=4&clazzid=5&cpi=6"><b>另一门</b></a>"#;
        let list = parse_courses(html);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "测试课程");
        assert_eq!(list[0].extra_str("clazz_id"), "2");
        assert_eq!(list[0].extra_str("cpi"), "3");
    }

    #[test]
    fn parse_chapters_from_html() {
        let html = r#"<input id="_studystate" value="{nextChapterId:1269662059,unfinishCount:102}"/>
        <div class="posCatalog_select firstLayer" id="1"><span class="posCatalog_title posCatalog_rotate titleIcon" title="绪论">绪论</span></div>
        <div class="posCatalog_select" id="cur1269662038"><span class="posCatalog_name" title="高校实验室概述" onclick="x"><em class="posCatalog_sbar">1.1</em> 高校实验室概述</span><span class="icon_Completed prevTips">已完成</span></div>
        <div class="posCatalog_select" id="cur1269662044"><span class="posCatalog_name" title="安全基本知识"><em class="posCatalog_sbar">1.2</em> 安全基本知识</span></div>"#;
        let list = parse_chapters(html);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].parent_title, "绪论");
        assert_eq!(list[0].title, "高校实验室概述");
        assert_eq!(list[0].number, "1.1");
        assert!(list[0].completed);
        assert!(!list[1].completed);
        assert_eq!(parse_unfinish_count(html), Some(102));
    }

    #[test]
    fn extract_marg_and_attachments() {
        let html = r#"<script>mArg = {"attachments":[{"type":"video","jobid":"j1","attDuration":896,"attDurationEnc":"e1","isPassed":false,"mid":"m1","objectId":"o1","otherInfo":"oi","property":{"module":"insertvideo","name":"视频A"}},{"type":"workid","jobid":"work-w1","property":{"module":"work","workid":"w1","title":"测验A"},"enc":"enc1","mid":"m2"}]}</script>"#;
        let marg = extract_marg(html).expect("mArg 应能解析");
        let atts = parse_attachments(&marg);
        assert_eq!(atts.len(), 2);
        assert_eq!(atts[0].kind, TaskKind::Video);
        assert_eq!(atts[0].duration, Some(896));
        assert_eq!(atts[0].extra_str("objectId"), "o1");
        assert_eq!(atts[1].kind, TaskKind::Quiz);
        assert_eq!(atts[1].id, "w1");
        assert_eq!(atts[1].extra_str("enc"), "enc1");
    }
}
