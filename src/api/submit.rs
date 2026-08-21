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
    let (status, body) = session.post_raw(submit_url(), payload).await?;
    if !status.is_success() {
        anyhow::bail!("submit HTTP {}", status);
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
}
