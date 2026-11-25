//! 学习通测验的 LLM 作答。
//!
//! 设计要点：
//! - 一次请求覆盖整份测验（题目少、省时省 token）；
//! - 强制 JSON 输出，用 `qid` 对齐，避免模型输出题号错位；
//! - 题型语义写进提示词（单选 1 个、多选可多个、判断对/错）；
//! - 输出后统一走 [`answer_value`] 转成平台提交值，并校验覆盖全部题目；
//! - 解析失败/缺题时重试一次，仍失败则报错而不是瞎填。

use crate::config::Config;
use crate::platform::chaoxing::quiz::{QKind, QuizPage, answer_value};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::HashMap;

/// 系统提示词：只约束输出格式与角色，不泄露平台细节。
pub const SYSTEM_PROMPT: &str = "你是一位严谨的中文课程考试作答助手。\
只根据题目与选项作答，不臆造题目之外的信息；只输出 JSON，不输出解释、不输出 Markdown。";

/// 构造整份测验的作答提示词。
pub fn build_prompt(page: &QuizPage) -> String {
    let mut p = String::new();
    p.push_str("下面是一份中文课程测验，请逐题作答。\n\n");
    p.push_str("题型说明：\n");
    p.push_str("- singlechoice：单选题，只能选 1 个选项；\n");
    p.push_str("- multichoice：多选题，正确答案通常有 2 个及以上，可多选；\n");
    p.push_str("- judge：判断题，答案只能是“对”或“错”。\n\n");
    p.push_str(&format!(
        "请把全部 {} 题的答案放在一个 JSON 对象里，格式严格如下（不要加代码块）：\n",
        page.questions.len()
    ));
    p.push_str("{\"answers\":[{\"qid\":\"题目id\",\"choice\":\"答案\"}]}\n\n");
    p.push_str("作答规则：\n");
    p.push_str("1. 必须覆盖全部题目，不得遗漏；\n");
    p.push_str("2. 单选填单个字母（如 \"C\"）；多选把所有字母连写（如 \"ABD\"）；判断填 \"对\" 或 \"错\"；\n");
    p.push_str("3. choice 里只写字母/对错，不要写选项文本、不要写理由；\n");
    p.push_str("4. 不确定时也要给出你认为最可能的答案。\n\n");
    p.push_str("题目：\n");
    for (i, q) in page.questions.iter().enumerate() {
        p.push_str(&format!(
            "{}. [{}] {}\n",
            i + 1,
            q.kind.as_str(),
            q.stem
        ));
        if q.kind == QKind::Judge {
            p.push_str("   A. 对\n   B. 错\n");
        } else {
            for o in &q.options {
                p.push_str(&format!("   {}. {}\n", o.label, o.text));
            }
        }
        p.push_str(&format!("   qid={}\n", q.qid));
    }
    p
}

/// 调用 LLM 作答，返回 `qid -> 提交值`。
pub async fn solve_with_llm(cfg: &Config, page: &QuizPage) -> Result<HashMap<String, String>> {
    if !cfg.use_llm() {
        bail!("未配置 api_key，无法使用 LLM 作答（可在 config.json 顶层填写）");
    }
    if page.questions.is_empty() {
        bail!("没有可作答的题目");
    }
    let prompt = build_prompt(page);
    let mut last_err = None;
    for attempt in 0..2 {
        let text = match crate::llm::ask_with(cfg, SYSTEM_PROMPT, &prompt).await {
            Ok(t) => t,
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        };
        match parse_answers(&text, page) {
            Ok(map) => return Ok(map),
            Err(e) => {
                last_err = Some(e);
                if attempt == 0 {
                    // 第二次尝试时补充更严格的要求
                    let retry_prompt = format!(
                        "{}\n\n注意：上一次输出的 JSON 不合法或缺少题目。请严格只输出 {{\"answers\":[...]}}，并覆盖全部 {} 题。",
                        prompt,
                        page.questions.len()
                    );
                    match crate::llm::ask_with(cfg, SYSTEM_PROMPT, &retry_prompt).await {
                        Ok(t2) => {
                            if let Ok(map) = parse_answers(&t2, page) {
                                return Ok(map);
                            }
                        }
                        Err(e2) => last_err = Some(e2),
                    }
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("LLM 作答失败")))
}

/// 解析模型输出为 `qid -> 提交值`，并校验覆盖全部题目。
pub fn parse_answers(text: &str, page: &QuizPage) -> Result<HashMap<String, String>> {
    let json = extract_json(text).context("模型输出里找不到 JSON 对象")?;
    let v: Value = serde_json::from_str(&json)
        .with_context(|| format!("模型输出不是合法 JSON：{}", truncate(text, 200)))?;

    let mut raw: Vec<(String, String)> = Vec::new();
    if let Some(arr) = v.get("answers").and_then(|a| a.as_array()) {
        for item in arr {
            let qid = item
                .get("qid")
                .and_then(|x| x.as_str())
                .or_else(|| item.get("id").and_then(|x| x.as_str()))
                .unwrap_or("");
            let choice = item
                .get("choice")
                .and_then(|x| x.as_str())
                .or_else(|| item.get("answer").and_then(|x| x.as_str()))
                .unwrap_or("");
            if !qid.is_empty() && !choice.is_empty() {
                raw.push((qid.to_string(), choice.to_string()));
            }
        }
    }
    // 兜底：{"1":"A","2":"对"} 形式（键为题号）
    if raw.is_empty()
        && let Some(obj) = v.as_object()
    {
        for (k, val) in obj {
            if let Some(s) = val.as_str()
                && k.chars().all(|c| c.is_ascii_digit())
                && let Some(q) = page
                    .questions
                    .get(k.parse::<usize>().unwrap_or(0).saturating_sub(1))
            {
                raw.push((q.qid.clone(), s.to_string()));
            }
        }
    }
    if raw.is_empty() {
        bail!("模型输出里没有可用答案：{}", truncate(text, 200));
    }

    let mut map = HashMap::new();
    for (qid, choice) in raw {
        let Some(q) = page.questions.iter().find(|q| q.qid == qid) else {
            continue;
        };
        let value = normalize_choice(q.kind, q, &choice);
        if value.is_empty() {
            continue;
        }
        map.insert(qid, value);
    }
    let missing: Vec<String> = page
        .questions
        .iter()
        .filter(|q| !map.contains_key(&q.qid))
        .map(|q| q.qid.clone())
        .collect();
    if !missing.is_empty() {
        bail!(
            "模型漏答 {} 题（{}）",
            missing.len(),
            missing.join(", ")
        );
    }
    Ok(map)
}

/// 把模型给的 choice 规范化成平台提交值。
fn normalize_choice(kind: QKind, q: &crate::platform::chaoxing::quiz::Question, choice: &str) -> String {
    match kind {
        QKind::Single => {
            let letters: Vec<char> = choice
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .map(|c| c.to_ascii_uppercase())
                .collect();
            let Some(first) = letters.first() else {
                return String::new();
            };
            answer_value(q, &first.to_string())
        }
        QKind::Multi => {
            let mut letters: Vec<char> = choice
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .map(|c| c.to_ascii_uppercase())
                .collect();
            letters.sort_unstable();
            letters.dedup();
            let joined: String = letters.iter().collect();
            answer_value(q, &joined)
        }
        QKind::Judge => answer_value(q, choice),
        _ => answer_value(q, choice),
    }
}

/// 从模型输出中提取第一个 JSON 对象（容忍 ```json 代码块与前后废话）。
pub fn extract_json(text: &str) -> Option<String> {
    let s = text.trim();
    let s = s.trim_start_matches("```json").trim_start_matches("```").trim();
    let s = s.trim_end_matches("```").trim();
    let start = s.find('{')?;
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    let mut end = None;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        let c = b as char;
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
                end = Some(i + 1);
                break;
            }
        }
    }
    end.map(|e| s[start..e].to_string())
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
    use crate::platform::chaoxing::quiz::{OptionItem, QKind, Question, QuizPage};

    fn page() -> QuizPage {
        let q = |qid: &str, kind: QKind, stem: &str, options: Vec<(&str, &str, &str)>| Question {
            qid: qid.to_string(),
            kind,
            answertype: match kind {
                QKind::Single => 0,
                QKind::Multi => 1,
                QKind::Judge => 3,
                _ => 0,
            },
            stem: stem.to_string(),
            options: options
                .into_iter()
                .map(|(l, v, t)| OptionItem {
                    label: l.to_string(),
                    value: v.to_string(),
                    text: t.to_string(),
                })
                .collect(),
        };
        QuizPage {
            url: String::new(),
            action: String::new(),
            fields: vec![],
            answerwqbid: String::new(),
            questions: vec![
                q(
                    "1",
                    QKind::Single,
                    "单选",
                    vec![("A", "A", "a"), ("B", "B", "b"), ("C", "C", "c")],
                ),
                q(
                    "2",
                    QKind::Multi,
                    "多选",
                    vec![("A", "A", "a"), ("B", "B", "b"), ("C", "C", "c")],
                ),
                q("3", QKind::Judge, "判断", vec![]),
            ],
            font_map: HashMap::new(),
        }
    }

    #[test]
    fn prompt_contains_all_questions() {
        let p = build_prompt(&page());
        assert!(p.contains("qid=1"));
        assert!(p.contains("qid=2"));
        assert!(p.contains("qid=3"));
        assert!(p.contains("judge"));
        assert!(p.contains("multichoice"));
    }

    #[test]
    fn parse_answers_normalizes() {
        let text = r#"```json
        {"answers":[
          {"qid":"1","choice":"c"},
          {"qid":"2","choice":"C,A"},
          {"qid":"3","choice":"对"}
        ]}
        ```"#;
        let m = parse_answers(text, &page()).unwrap();
        assert_eq!(m["1"], "C");
        assert_eq!(m["2"], "AC");
        assert_eq!(m["3"], "true");
    }

    #[test]
    fn parse_rejects_missing() {
        let text = r#"{"answers":[{"qid":"1","choice":"A"}]}"#;
        let err = parse_answers(text, &page()).unwrap_err().to_string();
        assert!(err.contains("漏答"), "{}", err);
    }

    #[test]
    fn extract_json_handles_noise() {
        let s = "好的，答案如下：\n{\"answers\":[]}\n以上。";
        assert_eq!(extract_json(s).as_deref(), Some("{\"answers\":[]}"));
    }
}
