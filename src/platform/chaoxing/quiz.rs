//! 章节测验：抓答题页、解析题目、生成防作弊参数 `pos`、提交、取分。
//!
//! 关键结论（已实测）：
//! - 提交响应 **不含分数**，只返回批阅页 `url`，要再请求该 url 解析"本次成绩N分"；
//! - `pos` 由 `enc_js_work.js` 的 `getEnc()` 生成，算法已完整逆向（见 [`build_pos`]）；
//! - 题目文本被 `font-cxsecret` 字体混淆，用 [`crate::platform::chaoxing::font`] 解码。

use crate::platform::chaoxing::font::{REF_TABLE, decode_font, decode_text};
use crate::platform::chaoxing::session::Session;
use anyhow::{Context, Result, bail};
use base64::Engine;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;

const MOOC1: &str = "https://mooc1.chaoxing.com";

/// 题型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QKind {
    Single,
    Multi,
    Judge,
    Fill,
    Other,
}

impl QKind {
    pub fn as_str(self) -> &'static str {
        match self {
            QKind::Single => "singlechoice",
            QKind::Multi => "multichoice",
            QKind::Judge => "judge",
            QKind::Fill => "fillblank",
            QKind::Other => "other",
        }
    }

    pub fn from_answertype(t: u32) -> Self {
        match t {
            0 => QKind::Single,
            1 => QKind::Multi,
            3 => QKind::Judge,
            2 | 4 => QKind::Fill,
            _ => QKind::Other,
        }
    }
}

/// 选项。
#[derive(Debug, Clone)]
pub struct OptionItem {
    /// 选项标签（A/B/C…）。
    pub label: String,
    /// 提交值（单选=字母；判断=true/false）。
    pub value: String,
    /// 选项文本。
    pub text: String,
}

/// 一道题。
#[derive(Debug, Clone)]
pub struct Question {
    pub qid: String,
    pub kind: QKind,
    pub answertype: u32,
    pub stem: String,
    pub options: Vec<OptionItem>,
}

/// 答题页解析结果。
#[derive(Debug, Clone)]
pub struct QuizPage {
    pub url: String,
    pub action: String,
    pub fields: Vec<(String, String)>,
    pub answerwqbid: String,
    pub questions: Vec<Question>,
    pub font_map: HashMap<char, char>,
}

impl QuizPage {
    pub fn field(&self, name: &str) -> String {
        self.fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    pub fn total_question_num(&self) -> String {
        self.field("totalQuestionNum")
    }
}

static RE_FONT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)font-cxsecret[^}]*base64,([A-Za-z0-9+/=]+)").unwrap()
});
static RE_ACTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<form[^>]*id="form1"[^>]*>"#).unwrap());
static RE_INPUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<input[^>]*name="([^"]+)"[^>]*value="([^"]*)""#).unwrap());
static RE_ANSWERWQBID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"var a = "([0-9,]+)";"#).unwrap());
static RE_QID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"id="question(\d+)""#).unwrap());
static RE_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"TiMu newTiMu"\s+data="(\d+)""#).unwrap());
static RE_STEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)newZy_TItle">([^<]*)</span>\s*<p>(.*?)</p>"#).unwrap()
});
static RE_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());

/// 构造 `/api/work` 链接。
#[allow(clippy::too_many_arguments)]
pub fn work_url(
    work_id: &str,
    jobid: &str,
    knowledge_id: &str,
    ktoken: &str,
    cpi: &str,
    clazz_id: &str,
    enc: &str,
    utenc: &str,
    course_id: &str,
) -> String {
    format!(
        "{}/mooc-ans/api/work?api=1&workId={}&jobid={}&originJobId={}&needRedirect=true&skipHeader=true&knowledgeid={}&ktoken={}&cpi={}&ut=s&clazzId={}&type=&enc={}&utenc={}&mooc2=1&courseid={}",
        MOOC1, work_id, jobid, jobid, knowledge_id, ktoken, cpi, clazz_id, enc, utenc, course_id
    )
}

/// 抓取并解析答题页。
pub async fn fetch_page(session: &Session, url: &str) -> Result<QuizPage> {
    let html = session.get_text(url).await?;
    parse_page(&html, url)
}

/// 解析答题页 HTML。
pub fn parse_page(html: &str, url: &str) -> Result<QuizPage> {
    if !html.contains("id=\"form1\"") {
        if html.contains("本次成绩")
            || html.contains("marking_dui")
            || html.contains("查看已批阅作业")
        {
            bail!("该测验已提交/已批阅，没有可作答的表单");
        }
        if html.contains("passport2.chaoxing.com") || html.contains("用户登录") {
            bail!("学习通登录已失效，请重新登录");
        }
        bail!("答题页解析失败：未找到 form1（可能作业不可作答或页面结构变化）");
    }
    // 字体
    let font_map = match RE_FONT.captures(html) {
        Some(c) => {
            let b64 = c.get(1).unwrap().as_str();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .context("内嵌字体 base64 解码失败")?;
            decode_font(&bytes, REF_TABLE).unwrap_or_default()
        }
        None => HashMap::new(),
    };
    // 表单 action
    let form_tag = RE_ACTION
        .find(html)
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    let action = Regex::new(r#"action="([^"]*)""#)
        .unwrap()
        .captures(&form_tag)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        .unwrap_or_default();

    // 表单域（截取 form1 内容）
    let form_body = match html.find("id=\"form1\"") {
        Some(i) => {
            let end = html[i..].find("</form>").map(|x| i + x).unwrap_or(html.len());
            &html[i..end]
        }
        None => html,
    };
    let mut fields: Vec<(String, String)> = Vec::new();
    for c in RE_INPUT.captures_iter(form_body) {
        let k = c.get(1).unwrap().as_str().to_string();
        let v = c.get(2).unwrap().as_str().to_string();
        if !fields.iter().any(|(n, _)| *n == k) {
            fields.push((k, v));
        }
    }

    // answerwqbid
    let answerwqbid = RE_ANSWERWQBID
        .captures(html)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        .unwrap_or_default();

    // 题目块
    let positions: Vec<(usize, String)> = RE_QID
        .captures_iter(html)
        .filter_map(|c| {
            let pos = c.get(0)?.start();
            Some((pos, c.get(1)?.as_str().to_string()))
        })
        .collect();
    let mut questions = Vec::new();
    for (idx, (pos, qid)) in positions.iter().enumerate() {
        let end = positions
            .get(idx + 1)
            .map(|(p, _)| *p)
            .unwrap_or(html.len());
        let block = &html[*pos..end];
        let answertype = RE_TYPE
            .captures(block)
            .and_then(|c| c.get(1)?.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        let stem_raw = RE_STEM
            .captures(block)
            .and_then(|c| Some(format!("{}{}", c.get(1)?.as_str(), c.get(2)?.as_str())))
            .unwrap_or_default();
        let stem = clean_text(&decode_text(&strip_tags(&stem_raw), &font_map));
        let options = parse_options(block, qid, &font_map);
        questions.push(Question {
            qid: qid.clone(),
            kind: QKind::from_answertype(answertype),
            answertype,
            stem,
            options,
        });
    }
    if questions.is_empty() {
        bail!("答题页没有解析到题目");
    }
    Ok(QuizPage {
        url: url.to_string(),
        action,
        fields,
        answerwqbid,
        questions,
        font_map,
    })
}

fn parse_options(block: &str, qid: &str, font_map: &HashMap<char, char>) -> Vec<OptionItem> {
    let re_li = Regex::new(&format!(r#"(?s)<li[^>]*qid="{}"[^>]*>(.*?)</li>"#, qid)).unwrap();
    let re_val = Regex::new(r#"data="([^"]*)""#).unwrap();
    let re_txt = Regex::new(r#"(?s)<a[^>]*class="fl after"[^>]*>(.*?)</a>"#).unwrap();
    let mut out = Vec::new();
    for c in re_li.captures_iter(block) {
        let inner = c.get(1).map(|m| m.as_str()).unwrap_or("");
        let value = re_val
            .captures(inner)
            .and_then(|x| x.get(1).map(|m| m.as_str().to_string()))
            .unwrap_or_default();
        if value.is_empty() {
            continue;
        }
        let text_raw = re_txt
            .captures(inner)
            .and_then(|x| x.get(1).map(|m| m.as_str().to_string()))
            .unwrap_or_default();
        let text = clean_text(&decode_text(&strip_tags(&text_raw), font_map));
        let label = match value.as_str() {
            "true" => "对",
            "false" => "错",
            other => other,
        };
        out.push(OptionItem {
            label: label.to_string(),
            value,
            text,
        });
    }
    out
}

/// 生成 `enc_js_work.js` 的 `getEnc()` 结果（无鼠标事件时坐标为 `NaN`，与页面一致）。
///
/// 说明：3 位后缀只出现在 `_edt` 末尾，不参与 cipher（被 keystr[:10] 截断），
/// 因此用随机数字即可，不影响服务端校验。
pub fn build_pos(user_id: &str, work_relation_id: &str) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let value = "(NaN|NaN)";
    let rnd: f64 = rand::random();
    let seed = ((1e9 * rnd).round() as i64).rem_euclid(100_000_000);
    let three = format!(
        "{}{}{}",
        rng.gen_range(0..10),
        rng.gen_range(0..10),
        rng.gen_range(0..10)
    );
    let key = format!("{}_{}|{}", user_id, work_relation_id, three);
    let cipher = cipher_hex(value, &key, seed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!(
        "{}&rd={}&value={}&wid={}&_edt={}{}",
        cipher, rnd, value, work_relation_id, ts, three
    )
}

/// getEnc 的逐字节 XOR 流：`hex(明文 XOR keystream) + seed 的 8 位 hex`。
fn cipher_hex(value: &str, key: &str, seed: i64) -> String {
    let keystr: String = key.chars().map(|c| (c as u32).to_string()).collect();
    let a = keystr.len() / 5;
    let b: i64 = [a, 2 * a, 3 * a, 4 * a]
        .iter()
        .map(|i| keystr.as_bytes()[*i] as char)
        .collect::<String>()
        .parse()
        .unwrap_or(2);
    let c: i64 = key.chars().count().div_ceil(2) as i64;
    const MAX: i64 = 0x7fff_ffff;
    let mut ks_text = format!("{}{}", keystr, seed);
    if ks_text.len() > 10 {
        ks_text = ks_text[..10].trim_start_matches('0').to_string();
        if ks_text.is_empty() {
            ks_text = "0".to_string();
        }
    }
    let mut ks: i64 = (b * ks_text.parse::<i64>().unwrap_or(0) + c) % MAX;
    let mut out = String::with_capacity(value.chars().count() * 2 + 8);
    for ch in value.chars() {
        let x = (ch as u32) ^ ((ks as f64 / MAX as f64 * 255.0).floor() as u32);
        out.push_str(&format!("{:02x}", x));
        ks = (b * ks + c) % MAX;
    }
    out.push_str(&format!("{:08x}", seed));
    out
}

/// 提交答案。`answers` 为 `qid -> 提交值`（单选字母 / 多选 `A,B` / 判断 true|false）。
pub async fn submit(
    session: &Session,
    page: &QuizPage,
    answers: &HashMap<String, String>,
) -> Result<Value> {
    let user_id = page.field("userId");
    let work_relation_id = page.field("workRelationId");
    if user_id.is_empty() || work_relation_id.is_empty() {
        bail!("答题页缺少 userId/workRelationId，无法生成 pos");
    }
    if page.action.is_empty() {
        bail!("答题页缺少 form action");
    }
    let pos = build_pos(&user_id, &work_relation_id);
    let url = format!(
        "{}/mooc-ans/work/{}&ua=pc&formType=post&saveStatus=1&pos={}&version=1",
        MOOC1, page.action, pos
    );
    let mut form: Vec<(String, String)> = page.fields.clone();
    form.retain(|(k, _)| !k.starts_with("answer") && k != "answerwqbid" && k != "enc");
    for q in &page.questions {
        let val = answers.get(&q.qid).cloned().unwrap_or_default();
        form.push((format!("answer{}", q.qid), val));
        form.push((format!("answertype{}", q.qid), q.answertype.to_string()));
    }
    form.push((
        "answerwqbid".to_string(),
        if page.answerwqbid.is_empty() {
            page.questions
                .iter()
                .map(|q| q.qid.clone())
                .collect::<Vec<_>>()
                .join(",")
                + ","
        } else {
            page.answerwqbid.clone()
        },
    ));
    form.push(("enc".to_string(), String::new()));

    // 与页面一致：先 validate
    let validate = format!(
        "{}/mooc-ans/work/validate?classId={}&courseId={}&knowledgeId={}&cpi={}",
        MOOC1,
        page.field("classId"),
        page.field("courseId"),
        page.field("knowledgeid"),
        page.field("cpi")
    );
    let _ = session.get_text(&validate).await;

    let text = session.post_form_text(&url, form).await?;
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("提交响应不是 JSON：{}", truncate(&text, 200)))?;
    if v.get("status").and_then(|x| x.as_bool()) != Some(true) {
        bail!(
            "提交失败：{}",
            v.get("msg").and_then(|x| x.as_str()).unwrap_or("未知错误")
        );
    }
    Ok(v)
}

/// 从提交响应里的批阅页 url 取分数。
pub async fn fetch_score(session: &Session, submit_response: &Value) -> Result<Option<f64>> {
    let url = submit_response
        .get("url")
        .and_then(|x| x.as_str())
        .unwrap_or_default();
    if url.is_empty() {
        return Ok(None);
    }
    let full = if url.starts_with("http") {
        url.to_string()
    } else {
        format!("{}{}", MOOC1, url)
    };
    let html = session.get_text(&full).await?;
    Ok(parse_score(&html))
}

/// 解析批阅页的"本次成绩N分"。
pub fn parse_score(html: &str) -> Option<f64> {
    let re = Regex::new(r"本次成绩\s*<[^>]*>\s*([0-9]+(?:\.[0-9]+)?)").unwrap();
    if let Some(c) = re.captures(html) {
        return c.get(1)?.as_str().parse().ok();
    }
    let re2 = Regex::new(r"本次成绩\s*([0-9]+(?:\.[0-9]+)?)").unwrap();
    re2.captures(html)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

/// 把 LLM/人工答案（如 "C"、"对"、"A,C"）转换为提交值。
pub fn answer_value(q: &Question, choice: &str) -> String {
    let choice = choice.trim();
    match q.kind {
        QKind::Judge => {
            let lower = choice.to_ascii_lowercase();
            if choice.contains('对')
                || lower == "true"
                || lower == "a"
                || lower == "t"
                || lower == "正确"
            {
                "true".to_string()
            } else if choice.contains('错')
                || lower == "false"
                || lower == "b"
                || lower == "f"
                || lower == "错误"
            {
                "false".to_string()
            } else {
                "true".to_string()
            }
        }
        QKind::Single | QKind::Multi => {
            let mut tokens: Vec<String> = Vec::new();
            for part in choice.split([',', '，', ' ', ';', '、']) {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                // 允许模型直接给 "AC" 这种连写，拆成单字母
                if part.chars().count() > 1 && part.chars().all(|c| c.is_ascii_alphabetic()) {
                    tokens.extend(part.chars().map(|c| c.to_string()));
                } else {
                    tokens.push(part.to_string());
                }
            }
            let mut out = String::new();
            for part in tokens {
                if let Some(opt) = q
                    .options
                    .iter()
                    .find(|o| o.label.eq_ignore_ascii_case(&part) || o.value == part)
                {
                    out.push_str(&opt.value);
                }
            }
            out
        }
        _ => choice.to_string(),
    }
}

fn strip_tags(s: &str) -> String {
    RE_TAG.replace_all(s, " ").to_string()
}

fn clean_text(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_value_maps_letters_and_judge() {
        let q = Question {
            qid: "1".into(),
            kind: QKind::Single,
            answertype: 0,
            stem: "s".into(),
            options: vec![
                OptionItem { label: "A".into(), value: "A".into(), text: "a".into() },
                OptionItem { label: "B".into(), value: "B".into(), text: "b".into() },
            ],
        };
        assert_eq!(answer_value(&q, "B"), "B");
        let j = Question {
            qid: "2".into(),
            kind: QKind::Judge,
            answertype: 3,
            stem: "s".into(),
            options: vec![],
        };
        assert_eq!(answer_value(&j, "对"), "true");
        assert_eq!(answer_value(&j, "错"), "false");
        assert_eq!(answer_value(&j, "B"), "false");
    }

    #[test]
    fn score_parse() {
        assert_eq!(parse_score("<span>本次成绩<i>100</i>分</span>"), Some(100.0));
        assert_eq!(parse_score("本次成绩 87.5分"), Some(87.5));
        assert_eq!(parse_score("没有分数"), None);
    }

    #[test]
    fn pos_has_expected_shape() {
        let pos = build_pos("413421686", "57384638");
        let parts: Vec<&str> = pos.split('&').collect();
        assert_eq!(parts.len(), 5, "pos 应有 5 段: {}", pos);
        // 前 5 字节只取决于 userId/workRelationId 的字符码前 10 位，
        // 与真实页面 getEnc() 的输出一致（见探针报告 §10.4）
        assert!(
            parts[0].starts_with("00249cabf3cf52a2a5"),
            "cipher 前缀与页面实测不一致: {}",
            parts[0]
        );
        assert!(parts[1].starts_with("rd="));
        assert_eq!(parts[2], "value=(NaN|NaN)");
        assert_eq!(parts[3], "wid=57384638");
        assert!(parts[4].starts_with("_edt="));
    }
}
