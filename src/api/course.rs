use crate::api::session::{progress_url, unit_progress_url, Session};
use anyhow::Result;
use serde::Deserialize;
use std::collections::BTreeMap;

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