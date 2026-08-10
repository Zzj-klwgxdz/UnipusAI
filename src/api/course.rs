use crate::api::session::{progress_url, unit_progress_url, Session};
use anyhow::Result;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Deserialize)]
pub struct CourseProgressResponse {
    pub rt: CourseProgressRt,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CourseProgressRt {
    #[serde(default)]
    pub units: BTreeMap<String, CourseUnitEntry>,
    #[serde(default)]
    pub publish_version: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CourseUnitEntry {
    pub strategies: Strategies,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProgressResponse {
    pub rt: ProgressRt,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProgressRt {
    #[serde(default)]
    pub duration_time: i64,
    #[serde(default)]
    pub flag: String,
    #[serde(default)]
    pub leafs: BTreeMap<String, LeafEntry>,
    #[serde(default)]
    pub micros: BTreeMap<String, MicroEntry>,
    #[serde(default)]
    pub open_id: String,
    #[serde(default)]
    pub publish_version: String,
    #[serde(default)]
    #[serde(rename = "tutorialId")]
    pub tutorial_id: String,
    #[serde(default)]
    pub unit_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LeafEntry {
    #[serde(default)]
    pub duration: i64,
    pub state: LeafState,
    pub strategies: Strategies,
    #[serde(default)]
    pub tab_type: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct LeafState {
    pub pass: u8,
    pub pass2: u8,
    pub perm: u8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Strategies {
    #[serde(default)]
    pub end_time: i64,
    #[serde(default)]
    pub min_score_pct: i32,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub start_time: i64,
    #[serde(default)]
    pub statistic_mode_out: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MicroEntry {
    pub state: LeafState,
    pub strategies: Strategies,
}

#[derive(Debug, Clone)]
pub struct GroupTask {
    pub group_id: String,
    pub unit_id: String,
    pub tab_type: String,
    pub required: bool,
    pub passed: bool,
    pub min_score_pct: i32,
    pub start_time: i64,
    pub end_time: i64,
}

pub async fn fetch_unit(session: &Session, unit_id: &str) -> Result<ProgressRt> {
    let url = unit_progress_url(session.course_id(), unit_id, session.open_id());
    let resp: ProgressResponse = session.get_json(&url).await?;
    Ok(resp.rt)
}

pub async fn fetch_course_progress(session: &Session) -> Result<CourseProgressRt> {
    let url = progress_url(session.course_id(), session.open_id());
    let resp: CourseProgressResponse = session.get_json(&url).await?;
    Ok(resp.rt)
}

pub async fn fetch_course_units(session: &Session) -> Result<Vec<String>> {
    let rt = fetch_course_progress(session).await?;
    let mut units: Vec<String> = rt.units.keys().cloned().collect();
    units.sort();
    Ok(units)
}

pub fn build_tasks(unit_id: &str, rt: &ProgressRt) -> Vec<GroupTask> {
    let mut tasks = Vec::new();
    for (gid, leaf) in &rt.leafs {
        tasks.push(GroupTask {
            group_id: gid.clone(),
            unit_id: unit_id.to_string(),
            tab_type: leaf.tab_type.clone(),
            required: leaf.strategies.required,
            passed: leaf.state.pass >= 1,
            min_score_pct: leaf.strategies.min_score_pct,
            start_time: leaf.strategies.start_time,
            end_time: leaf.strategies.end_time,
        });
    }
    tasks.sort_by(|a, b| a.group_id.cmp(&b.group_id));
    tasks
}

pub fn select_tasks(tasks: &[GroupTask], compulsory_only: bool) -> Vec<GroupTask> {
    tasks
        .iter()
        .filter(|t| !compulsory_only || t.required)
        .cloned()
        .collect()
}

const LABEL_CACHE_FILE: &str = ".unit_labels.json";

fn labels() -> &'static Mutex<Option<BTreeMap<String, String>>> {
    static LABELS: OnceLock<Mutex<Option<BTreeMap<String, String>>>> = OnceLock::new();
    LABELS.get_or_init(|| {
        let loaded = std::fs::read_to_string(LABEL_CACHE_FILE)
            .ok()
            .and_then(|s| serde_json::from_str::<BTreeMap<String, String>>(&s).ok());
        Mutex::new(loaded)
    })
}

fn label_cache_key(session: &Session, unit_id: &str) -> String {
    format!("{}|{}", session.course_id(), unit_id)
}

async fn fetch_unit_label(session: &Session, unit_id: &str) -> Result<Option<String>> {
    let rt = fetch_unit(session, unit_id).await?;
    let gid = rt
        .leafs
        .iter()
        .find(|(_, l)| l.tab_type == "task")
        .map(|(g, _)| g.clone())
        .or_else(|| rt.leafs.keys().next().cloned());
    let Some(gid) = gid else {
        return Ok(None);
    };
    let fc = crate::api::content::fetch_content(session, &gid).await?;
    let plain = crate::api::content::decrypt_content(&fc.content, &fc.k)?;
    let dec = crate::api::content::parse_decrypted(&plain)?;
    let label = crate::api::parser::extract_group_label(&dec);
    Ok((!label.is_empty()).then_some(label))
}

/// 取单元可读标签（如 "U1 Pre-reading activities"）。带 `.unit_labels.json` 缓存；
/// 提取不到时返回 None，由调用方回退到 "Unit N"。
pub async fn unit_label(session: &Session, unit_id: &str) -> Result<Option<String>> {
    let key = label_cache_key(session, unit_id);
    {
        let guard = labels().lock().unwrap();
        if let Some(label) = guard.as_ref().and_then(|m| m.get(&key)) {
            return Ok(Some(label.clone()));
        }
    }
    if let Some(label) = fetch_unit_label(session, unit_id).await? {
        let mut guard = labels().lock().unwrap();
        let map = guard.get_or_insert_with(BTreeMap::new);
        map.insert(key, label.clone());
        if let Ok(json) = serde_json::to_string_pretty(map) {
            let _ = std::fs::write(LABEL_CACHE_FILE, json);
        }
        return Ok(Some(label));
    }
    Ok(None)
}

/// 从 course_id 中解析课程代码并映射为可读课程名；未知代码回退到代码段本身。
/// 例：`course-v2:...nhce_v4_rw_2+...` -> `新视野大学英语(第四版)读写教程 2`。
pub fn course_display_name(course_id: &str) -> String {
    let code = course_id.split('+').nth(1).unwrap_or(course_id);
    // 课程代码尾部可能是册号，如 nhce_v4_rw_2 -> 基础代码 nhce_v4_rw + 册号 2
    let (base, book) = match code.rfind('_') {
        Some(i) if !code[i + 1..].is_empty() && code[i + 1..].chars().all(|c| c.is_ascii_digit()) => {
            (&code[..i], &code[i + 1..])
        }
        _ => (code, ""),
    };
    let name = match base {
        "nhce_v4_rw" => "新视野大学英语(第四版)读写教程",
        "nhce_v4_ls" => "新视野大学英语(第四版)听说教程",
        "nhce_v4_ur" => "新视野大学英语(第四版)视听说教程",
        _ => return code.to_string(),
    };
    if book.is_empty() {
        name.to_string()
    } else {
        format!("{} {}", name, book)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn course_display_known_and_fallback() {
        assert_eq!(
            course_display_name("course-v2:75b7546ea002b72+nhce_v4_rw_2+20230116"),
            "新视野大学英语(第四版)读写教程 2"
        );
        assert_eq!(
            course_display_name("course-v2:75b7546ea002b72+nhce_v4_rw+20230116"),
            "新视野大学英语(第四版)读写教程"
        );
        assert_eq!(course_display_name("course-v2:x+yz_3+9"), "yz_3");
    }
}