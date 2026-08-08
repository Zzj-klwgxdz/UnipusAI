use crate::api::session::{content_url, user_module_url, Session};
use anyhow::Result;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct UserRecord {
    pub ts: i64,
    pub group_id: String,
    pub module: Value,
    pub my_ques: Vec<Value>,
}

pub async fn fetch_my_records(session: &Session, group_id: &str) -> Result<UserRecord> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let url = user_module_url(session.course_id(), group_id, ts);
    let mut value: Value = session.get_json(&url).await?;
    let rows = value
        .get_mut("data")
        .and_then(|d| d.get_mut("rows"))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(UserRecord {
        ts,
        group_id: group_id.to_string(),
        module: rows,
        my_ques: Vec::new(),
    })
}

pub async fn fetch_single_content(session: &Session, group_id: &str) -> Result<Value> {
    let url = content_url(session.course_id(), group_id);
    let value: Value = session.get_json(&url).await?;
    Ok(value)
}