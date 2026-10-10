use crate::api::session::Session;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

const UCLOUD: &str = "https://ucloud.unipus.cn";

fn topic_page_url() -> String {
    format!("{}/api/bbs/utopic/page", UCLOUD)
}

fn topic_add_url() -> String {
    format!("{}/api/bbs/utopic/add", UCLOUD)
}

fn reply_page_url() -> String {
    format!("{}/api/bbs/ureply/top/page", UCLOUD)
}

fn reply_add_url() -> String {
    format!("{}/api/bbs/ureply/add", UCLOUD)
}

/// BBS 主题（讨论区帖子）。
#[derive(Debug, Clone)]
pub struct Topic {
    pub topic_id: i64,
    pub owner_status: bool,
    #[allow(dead_code)]
    pub reply_count: i64,
}

/// 讨论题所需的班级/课程配置，缺一不可（服务端会分别报错）。
fn bbs_scope(session: &Session) -> Result<(String, String)> {
    let cfg = session.cfg();
    if cfg.class_id.is_empty() {
        bail!(
            "讨论题需要 config.json 的 class_id（页面 URL 的 cid），当前为空"
        );
    }
    if cfg.curricula_id.is_empty() {
        bail!(
            "讨论题需要 config.json 的 curricula_id（页面 URL 的 cloudCurriculaId），当前为空"
        );
    }
    Ok((cfg.class_id.clone(), cfg.curricula_id.clone()))
}

/// BBS 接口成功时返回 code=1（而非 0）；重复提交类错误码 10001/10002 也视为成功。
fn parse_bbs_response(body: &str, allow_duplicate: bool) -> Result<Value> {
    let v: Value = serde_json::from_str(body)
        .with_context(|| format!("BBS 响应不是 JSON: {}", crate::api::parser::truncate_text(body, 200)))?;
    let success = v.get("success").and_then(|s| s.as_bool()).unwrap_or(false);
    let code = v.get("code").and_then(|c| c.as_i64());
    if success || code == Some(1) {
        return Ok(v);
    }
    if allow_duplicate && matches!(code, Some(10001 | 10002)) {
        return Ok(v);
    }
    let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
    let message = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
    if code.is_none() && !success && msg.is_empty() && message.is_empty() {
        bail!("BBS 接口返回异常: {}", crate::api::parser::truncate_text(body, 200));
    }
    bail!("BBS 接口错误 code={:?} msg={} message={}", code, msg, message)
}

/// 解析 JWT 的 exp（秒）；解析失败返回 None。
pub fn jwt_exp(token: &str) -> Option<i64> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("exp").and_then(|e| e.as_i64())
}

/// 统一的 BBS 请求：自动选择 exp 最新的 JWT，401 时换另一个候选重试；全部失败后自动刷新登录再试。
async fn post_bbs(
    session: &Session,
    url: &str,
    payload: &Value,
    allow_duplicate: bool,
) -> Result<Value> {
    let body = serde_json::to_string(payload).context("序列化 BBS 请求失败")?;
    let mut last_status = reqwest::StatusCode::UNAUTHORIZED;
    let mut last_body = String::new();
    let mut refreshed = false;
    loop {
        let candidates = session.jwt_candidates();
        if candidates.is_empty() {
            if !refreshed && session.refresh_auth().await.unwrap_or(false) {
                refreshed = true;
                continue;
            }
            bail!("无可用 JWT：请在 config.json 填写 username/password（推荐）或 cookie（含 jwt=）");
        }
        for (i, auth) in candidates.iter().enumerate() {
            let (status, text) = session.post_raw_with_auth(url, &body, Some(auth)).await?;
            if status.is_success() {
                return parse_bbs_response(&text, allow_duplicate);
            }
            last_status = status;
            last_body = text;
            if status == reqwest::StatusCode::UNAUTHORIZED && i + 1 < candidates.len() {
                log::warn!("BBS 请求 401，改用另一个 JWT 重试");
                continue;
            }
            break;
        }
        if last_status == reqwest::StatusCode::UNAUTHORIZED && !refreshed {
            log::warn!("BBS 全部 JWT 均 401，尝试自动刷新登录后重试");
            if session.refresh_auth().await.unwrap_or(false) {
                refreshed = true;
                continue;
            }
        }
        break;
    }
    if last_status == reqwest::StatusCode::UNAUTHORIZED {
        bail!(
            "BBS 接口 401：JWT 均不可用（过期或无权限），\
             请填写 username/password 以自动登录，或重新从浏览器复制 cookie。响应: {}",
            crate::api::parser::truncate_text(&last_body, 150)
        );
    }
    bail!(
        "BBS 接口 HTTP {}: {}",
        last_status,
        crate::api::parser::truncate_text(&last_body, 200)
    )
}

pub fn topics_from_value(v: &Value) -> Vec<Topic> {
    v.pointer("/value/utopics")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let topic_id = t.get("topicId").and_then(|x| x.as_i64())?;
                    Some(Topic {
                        topic_id,
                        owner_status: t
                            .get("ownerStatus")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false),
                        reply_count: t.get("replyCount").and_then(|x| x.as_i64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn has_own_reply_in(v: &Value) -> bool {
    v.pointer("/value/replyContents")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter().any(|r| {
                r.get("ownerStatus")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// 查询讨论组的主题列表（可能为空）。
pub async fn fetch_topics(session: &Session, group_id: &str) -> Result<Vec<Topic>> {
    let (class_id, curricula_id) = bbs_scope(session)?;
    let payload = json!({
        "order": "desc",
        "pageNum": 1,
        "pageSize": 10,
        "courseId": session.course_id(),
        "groupId": group_id,
        "type": 2,
        "classId": class_id,
        "curriculaId": curricula_id,
    });
    let v = post_bbs(session, &topic_page_url(), &payload, false).await?;
    Ok(topics_from_value(&v))
}

/// 创建主题（学生自己的帖子）。
pub async fn create_topic(
    session: &Session,
    group_id: &str,
    title: &str,
    content: &str,
) -> Result<()> {
    let (class_id, curricula_id) = bbs_scope(session)?;
    let payload = json!({
        "title": title,
        "content": content,
        "courseId": session.course_id(),
        "groupId": group_id,
        "type": 2,
        "classId": class_id,
        "curriculaId": curricula_id,
    });
    post_bbs(session, &topic_add_url(), &payload, true)
        .await
        .map(|_| ())
        .with_context(|| format!("创建讨论主题失败 (groupId={})", group_id))
}

/// 确保讨论组存在本人主题：优先 ownerStatus=true，其次第一个，最后创建后重查。
/// 返回可回复的 topicId，以及主题标题/内容建议（调用方用于创建）。
pub async fn ensure_topic(
    session: &Session,
    group_id: &str,
    title: &str,
    content: &str,
) -> Result<i64> {
    let pick = |topics: &[Topic]| -> Option<i64> {
        topics
            .iter()
            .find(|t| t.owner_status)
            .or_else(|| topics.first())
            .map(|t| t.topic_id)
    };
    let topics = fetch_topics(session, group_id).await?;
    if let Some(id) = pick(&topics) {
        return Ok(id);
    }
    create_topic(session, group_id, title, content).await?;
    let topics = fetch_topics(session, group_id).await?;
    pick(&topics).with_context(|| {
        format!(
            "讨论组 {} 创建主题后仍未查询到 topicId，请检查 class_id/curricula_id 配置",
            group_id
        )
    })
}

/// 主题下是否已有本人的回复（用于避免重复发帖）。
pub async fn has_own_reply(session: &Session, topic_id: i64) -> Result<bool> {
    let payload = json!({
        "topicId": topic_id,
        "parentId": "",
        "order": "desc",
        "pageNum": 1,
        "pageSize": 10,
    });
    let v = post_bbs(session, &reply_page_url(), &payload, false).await?;
    Ok(has_own_reply_in(&v))
}

/// 在主题下发表评论。
pub async fn post_reply(session: &Session, topic_id: i64, content: &str) -> Result<()> {
    let payload = json!({
        "type": 2,
        "content": content,
        "topicId": topic_id,
        "parentId": "",
        "topReplyId": "",
        "contentType": "text",
    });
    post_bbs(session, &reply_add_url(), &payload, true)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_parse_with_results() {
        let json = r#"{"code":1,"msg":"SUCCESS","value":{"count":1,"utopics":[{"topicId":10190684,"title":"t","content":"c","ownerStatus":true,"replyCount":0}]},"success":true}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let topics = topics_from_value(&v);
        assert_eq!(topics.len(), 1);
        assert_eq!(topics[0].topic_id, 10190684);
        assert!(topics[0].owner_status);
        assert_eq!(topics[0].reply_count, 0);
    }

    #[test]
    fn topics_parse_empty_null() {
        let json = r#"{"code":1,"msg":"SUCCESS","value":{"count":0,"utopics":null},"success":true}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        assert!(topics_from_value(&v).is_empty());
    }

    #[test]
    fn reply_owner_status() {
        let json = r#"{"code":1,"msg":"SUCCESS","value":{"count":1,"replyTotalCount":1,"replyContents":[{"id":41572409,"content":"1","ownerStatus":true}],"uaiBbs":true},"success":true}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        assert!(has_own_reply_in(&v));
        let empty = r#"{"code":1,"msg":"SUCCESS","value":{"count":0,"replyTotalCount":0,"replyContents":[],"uaiBbs":true},"success":true}"#;
        let v2: Value = serde_json::from_str(empty).unwrap();
        assert!(!has_own_reply_in(&v2));
    }

    #[test]
    fn error_response_rejected() {
        let json = r#"{"code":100,"msg":"Ai版课程Id不能为空","value":null,"success":false}"#;
        let err = parse_bbs_response(json, false).unwrap_err().to_string();
        assert!(err.contains("Ai版课程Id不能为空"));
        // 重复提交类错误码按成功处理
        let dup = r#"{"code":10002,"msg":"重复提交","success":false}"#;
        assert!(parse_bbs_response(dup, true).is_ok());
        assert!(parse_bbs_response(dup, false).is_err());
    }

    fn make_jwt(exp: i64) -> String {
        use base64::Engine;
        let enc = |s: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        format!(
            "{}.{}.{}",
            enc(br#"{"alg":"none"}"#),
            enc(format!("{{\"exp\":{}}}", exp).as_bytes()),
            enc(b"sig")
        )
    }

    #[test]
    fn jwt_exp_decode() {
        let token = make_jwt(4102444800);
        assert_eq!(jwt_exp(&token), Some(4102444800));
        assert_eq!(jwt_exp("not-a-jwt"), None);
        assert_eq!(jwt_exp(""), None);
    }
}
