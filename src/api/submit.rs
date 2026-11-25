use crate::api::parser::{ParsedGroup, build_context};
use crate::api::session::{Session, submit_url};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};

/// submit 命中服务端限频（操作过于频繁）。
/// 供上层捕获后等待冷却并重试。
#[derive(Debug)]
pub struct RateLimited {
    pub code: String,
    pub msg: String,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "限频提交被拒 (code={} msg={})", self.code, self.msg)
    }
}

impl std::error::Error for RateLimited {}

/// 判断响应是否限频：msg 含关键字优先，其次命中常见限频 code。
pub fn is_rate_limited(v: &Value) -> bool {
    let msg = v
        .get("msg")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let msg_limited = msg.contains("操作过于频繁");
    let code_limited = v
        .get("code")
        .and_then(|c| c.as_i64())
        .is_some_and(|c| matches!(c, 600001 | 600002));
    msg_limited || code_limited
}

/// 提交响应中的判分信息。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubmitScore {
    /// 各小题得分（原始值，通常 0..1 或 0..100）。
    pub items: Vec<f64>,
    /// 百分比得分（0..100）；None 表示响应中无百分比。
    pub percent: Option<f64>,
}

impl SubmitScore {
    /// 展示用文本：`100%（各题 1/1/1/1/1/1）`。
    pub fn display(&self) -> String {
        let mut out = match self.percent {
            Some(p) => format_percent(p),
            None => "未知".to_string(),
        };
        if !self.items.is_empty() {
            out.push_str("（各题 ");
            let shown: Vec<String> = self.items.iter().take(12).map(|v| fmt_score(*v)).collect();
            out.push_str(&shown.join("/"));
            if self.items.len() > 12 {
                out.push_str(&format!("…(+{})", self.items.len() - 12));
            }
            out.push('）');
        }
        out
    }
}

/// 从 submit 响应提取判分（`data.state.score` / `score_pct` / `record_grade.scorePct` 等）。
pub fn parse_submit_score(v: &Value) -> Option<SubmitScore> {
    let state = v.pointer("/data/state");
    let mut items = Vec::new();
    if let Some(score) = state.and_then(|s| s.get("score")) {
        collect_scores(score, &mut items);
    }
    let percent = state
        .and_then(|s| s.get("score_pct"))
        .and_then(value_f64)
        .or_else(|| {
            state
                .and_then(|s| s.get("real_score_pct"))
                .and_then(value_f64)
        })
        .or_else(|| {
            v.pointer("/data/record_grade/scorePct")
                .and_then(value_f64)
        })
        .or_else(|| state.and_then(|s| s.get("score_avg")).and_then(value_f64))
        .map(normalize_percent)
        .or_else(|| {
            if items.is_empty() {
                None
            } else {
                let mean = items.iter().sum::<f64>() / items.len() as f64;
                Some(normalize_percent(mean))
            }
        });
    if items.is_empty() && percent.is_none() {
        return None;
    }
    Some(SubmitScore { items, percent })
}

fn value_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn collect_scores(v: &Value, out: &mut Vec<f64>) {
    match v {
        Value::Array(arr) => {
            for x in arr {
                if let Some(n) = value_f64(x) {
                    out.push(n);
                }
            }
        }
        Value::Object(map) => {
            for x in map.values() {
                if let Some(n) = value_f64(x) {
                    out.push(n);
                }
            }
        }
        _ => {
            if let Some(n) = value_f64(v) {
                out.push(n);
            }
        }
    }
}

/// 0..1 视为比例（×100），其余视为已是百分比；限制在 0..=100。
fn normalize_percent(v: f64) -> f64 {
    let pct = if v <= 1.0 { v * 100.0 } else { v };
    (pct.clamp(0.0, 100.0) * 10.0).round() / 10.0
}

fn format_percent(p: f64) -> String {
    if (p - p.round()).abs() < 0.05 {
        format!("{:.0}%", p.round())
    } else {
        format!("{:.1}%", p)
    }
}

fn fmt_score(v: f64) -> String {
    if (v - v.round()).abs() < 0.005 {
        format!("{:.0}", v.round())
    } else {
        let s = format!("{:.2}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// 子题作答。
#[derive(Debug, Clone)]
pub struct ChildAnw {
    pub question_type: String,
    pub reply_type: String,
    pub value: String,
    /// 选项个数（选择题用于随机作答）
    pub option_count: usize,
}

/// 单个模块的作答。
#[derive(Debug, Clone)]
pub struct ModuleAnsw {
    pub instance_id: String,
    pub children: Vec<ChildAnw>,
}

// serde 字段重命名用宏省事：直接手动实现 Serialize 会太长，
// 改用 serde(rename_all) 会生成 snake_case，这里在顶层构造 JSON。
fn build_payload_json(
    sessions: &Session,
    group_id: &str,
    ques_datas: Vec<SubmitQues>,
    is_completed: Vec<bool>,
    third_party_judges: String,
    submit_type: u32,
    hide_loading: bool,
) -> Result<String> {
    let obj = json!({
        "quesDatas": ques_datas,
        "groupId": group_id,
        "isCompleted": is_completed,
        "thirdPartyJudges": third_party_judges,
        "submitType": submit_type,
        "hideLoading": hide_loading,
        "associationGroupId": "",
        "courseId": sessions.course_id(),
        "openId": sessions.open_id(),
        "version": "default",
    });
    serde_json::to_string(&obj).context("序列化提交载荷失败")
}

#[derive(Debug, Clone, Serialize)]
struct SubmitQues {
    instanceId: String,
    answer: String,
    context: String,
    contextVersion: u32,
    answerVersion: u32,
}

fn versions(session: &Session) -> Value {
    let course = session.cfg().publish_version.parse::<i64>().unwrap_or(1);
    json!({
        "course": course,
        "group": 1,
        "template": 1,
        "answer": 3,
        "content": 0,
    })
}

fn build_answer_json(children: &[ChildAnw]) -> String {
    let children_json: Vec<Value> = children
        .iter()
        .map(|c| {
            let v: Value = if c.value.is_empty() {
                json!([])
            } else if c.reply_type == "multichoice" {
                // 多选题: "A,B,C" -> ["A","B","C"]
                let parts: Vec<&str> = c.value.split(',').map(|s| s.trim()).collect();
                json!(parts)
            } else {
                json!([c.value])
            };
            json!({ "value": v, "isDone": !c.value.is_empty() })
        })
        .collect();
    json!({
        "value": [],
        "children": children_json,
        "progress": {},
        "record": {"url": ""}
    })
    .to_string()
}

fn make_judges(session: &Session, modules: &[ModuleAnsw]) -> String {
    let versions = versions(session);
    let mut arr: Vec<Value> = Vec::new();
    for m in modules {
        for c in &m.children {
            arr.push(json!({
                "value": c.value,
                "question_type": c.question_type,
                "reply_type": c.reply_type,
                "versions": versions,
                "payloads": [],
            }));
        }
    }
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

pub fn build_answer_payload(
    session: &Session,
    group_id: &str,
    modules: &[ModuleAnsw],
) -> Result<String> {
    let questions_datas: Vec<SubmitQues> = modules
        .iter()
        .map(|m| SubmitQues {
            instanceId: m.instance_id.clone(),
            answer: build_answer_json(&m.children),
            context: build_context(),
            contextVersion: 1,
            answerVersion: 1,
        })
        .collect();

    let is_completed: Vec<bool> = modules
        .iter()
        .flat_map(|m| m.children.iter().map(|c| !c.value.is_empty()))
        .collect();

    let judges = make_judges(session, modules);

    let s = build_payload_json(
        session,
        group_id,
        questions_datas,
        is_completed,
        judges,
        1,
        false,
    )?;
    Ok(s)
}

pub fn build_mark_seen_payload(session: &Session, group_id: &str) -> Result<String> {
    let s = build_payload_json(
        session,
        group_id,
        Vec::new(),
        Vec::new(),
        "[]".to_string(),
        2,
        true,
    )?;
    Ok(s)
}

pub fn empty_answers(group: &ParsedGroup) -> Vec<ModuleAnsw> {
    group
        .modules
        .iter()
        .map(|m| ModuleAnsw {
            instance_id: m.instance_id.clone(),
            children: m
                .children
                .iter()
                .map(|c| ChildAnw {
                    question_type: c.question_type.clone(),
                    reply_type: c.reply_type.clone(),
                    value: String::new(),
                    option_count: c.option_count,
                })
                .collect(),
        })
        .collect()
}

pub async fn submit_raw(session: &Session, payload: &str) -> Result<Value> {
    // 服务端偶发 503（openresty 限流/WAF），退避重试几次
    let mut attempt = 0u32;
    let (status, body) = loop {
        let (s, b) = session.post_raw(submit_url(), payload).await?;
        if s.is_server_error() && attempt < 3 {
            attempt += 1;
            let wait = std::time::Duration::from_secs(2u64.pow(attempt));
            log::warn!("submit 返回 {}，{}s 后重试（{}/3）", s, wait.as_secs(), attempt);
            tokio::time::sleep(wait).await;
            continue;
        }
        break (s, b);
    };
    if !status.is_success() {
        let brief: String = body.chars().take(300).collect();
        anyhow::bail!("submit HTTP {}: {}", status, brief);
    }
    let v: Value = serde_json::from_str(&body)?;
    let code = v.get("code").and_then(|c| c.as_i64());
    if code != Some(0) {
        let code_s = v.get("code").map(|c| c.to_string()).unwrap_or_default();
        let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
        let message = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
        if is_rate_limited(&v) {
            return Err(RateLimited {
                code: code_s,
                msg: msg.to_string(),
            }
            .into());
        }
        anyhow::bail!(
            "submit code={} msg={} message={}",
            code_s,
            msg,
            message
        );
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multichoice_answer_split() {
        let c = ChildAnw {
            question_type: "basic".into(),
            reply_type: "multichoice".into(),
            value: "A,B,C".into(),
            option_count: 3,
        };
        let s = build_answer_json(&[c]);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(
            v["children"][0]["value"],
            serde_json::json!(["A", "B", "C"])
        );
    }

    #[test]
    fn singlechoice_answer() {
        let c = ChildAnw {
            question_type: "basic".into(),
            reply_type: "singlechoice".into(),
            value: "B".into(),
            option_count: 4,
        };
        let s = build_answer_json(&[c]);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["children"][0]["value"], serde_json::json!(["B"]));
    }

    #[test]
    fn score_parses_real_submit_response() {
        // 实际 dropdownSelection 提交响应（节选）
        let v: Value = serde_json::from_str(
            r#"{"code":0,"data":{"record_grade":{"scorePct":1,"ts":1792403102},
                "state":{"score":[1,1,1,1,1,1],"score_avg":100,"score_pct":1,"state":1}},"message":"成功"}"#,
        )
        .unwrap();
        let s = parse_submit_score(&v).unwrap();
        assert_eq!(s.items, vec![1.0; 6]);
        assert_eq!(s.percent, Some(100.0));
        assert_eq!(s.display(), "100%（各题 1/1/1/1/1/1）");
    }

    #[test]
    fn score_falls_back_and_tolerates_types() {
        // 无 state.score，仅 record_grade.scorePct（0..1）
        let v = json!({"data":{"record_grade":{"scorePct":0.8}}});
        let s = parse_submit_score(&v).unwrap();
        assert_eq!(s.percent, Some(80.0));
        assert!(s.items.is_empty());

        // 字符串/浮点混合 + score_avg 兜底（已是百分比）
        let v = json!({"data":{"state":{"score":["0.5", 1, null], "score_avg":"85"}}});
        let s = parse_submit_score(&v).unwrap();
        assert_eq!(s.items, vec![0.5, 1.0]);
        assert_eq!(s.percent, Some(85.0));
        assert_eq!(s.display(), "85%（各题 0.5/1）");

        // 无任何判分数据
        assert!(parse_submit_score(&json!({"code":0,"data":null})).is_none());
        assert!(parse_submit_score(&json!({"code":0,"data":{"state":{"score":[]}}})).is_none());
    }

    #[test]
    fn score_display_truncates_long_item_list() {
        let s = SubmitScore {
            items: (0..15)
                .map(|i| if i == 0 { 1.0 } else { 0.5 })
                .collect(),
            percent: Some(66.7),
        };
        let text = s.display();
        assert!(text.starts_with("66.7%"), "{}", text);
        assert!(text.contains("…(+3)"), "{}", text);
    }
}
