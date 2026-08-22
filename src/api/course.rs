use crate::api::session::{Session, progress_url, unit_progress_url};
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


/// 进程内按 course_id 缓存课程名，避免重复请求该接口。
fn course_name_cache() -> &'static Mutex<BTreeMap<String, String>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// 首页课程列表接口：value.courseList[].courseResourceList[].instanceId 与 course_id 一致，
/// name 为可读课程名。注意该接口返回 code=1/success，不能用 get_json（其要求 code==0）。
const HOME_COURSE_LIST_URL: &str = "https://uai.unipus.cn/api/cmgt/course/getHomeCourseListByStudent";

/// 查询课程名：优先用首页课程列表接口按 instanceId 精确匹配，
/// 失败或未命中时回退到 course_display_name_fallback 的启发式解析。
pub async fn course_display_name(session: &Session, course_id: &str) -> String {
    if let Ok(guard) = course_name_cache().lock() {
        if let Some(name) = guard.get(course_id) {
            return name.clone();
        }
    }
    let name = match course_display_name_lookup(session, course_id).await {
        Some(n) if !n.is_empty() => n,
        _ => course_display_name_fallback(course_id),
    };
    if let Ok(mut guard) = course_name_cache().lock() {
        guard.insert(course_id.to_string(), name.clone());
    }
    name
}

/// 调用首页课程列表接口，返回与 course_id 匹配的课程名；无匹配或失败返回 None。
/// 失败路径会打 WARN，便于排查（该接口返回 code=1/success，整体不按 code==0 判定）。
pub async fn course_display_name_lookup(session: &Session,course_id: &str,) -> Option<String> {
    fn truncate300(s: &str) -> String {
        if s.chars().count() <= 300 {
            s.to_string()
        } else {
            format!("{}…", s.chars().take(300).collect::<String>())
        }
    }
    let body = match session.get_bytes(HOME_COURSE_LIST_URL).await {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => {
            log::warn!("首页课程列表请求失败: {:#}", e);
            return None;
        }
    };
    let v: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("首页课程列表响应非 JSON: {:#}; body={}", e, truncate300(&body));
            return None;
        }
    };
    let Some(courses) = v.pointer("/value/courseList").and_then(|c| c.as_array()) else {
        log::warn!("首页课程列表缺少 value.courseList; body={}", truncate300(&body));
        return None;
    };
    for course in courses {
        let Some(res_list) = course.get("courseResourceList").and_then(|r| r.as_array()) else {
            continue;
        };
        for res in res_list {
            let inst = res.get("instanceId").and_then(|i| i.as_str()).unwrap_or("");
            if inst == course_id {
                return res.get("name").and_then(|n| n.as_str()).map(str::to_string);
            }
        }
    }
    log::warn!("首页课程列表未找到匹配 course_id={} 的资源", course_id);
    None
}


/// 从 course_id 中解析课程代码并映射为可读课程名；未知代码回退到代码段本身。
/// 例：`course-v2:...nhce_v4_rw_2+...` -> `新视野大学英语(第四版)读写教程 2`。
pub fn course_display_name_fallback(course_id: &str) -> String {
    let code = course_id.split('+').nth(1).unwrap_or(course_id);
    // 课程代码尾部可能是册号，如 nhce_v4_rw_2 -> 基础代码 nhce_v4_rw + 册号 2
    let (base, book) = match code.rfind('_') {
        Some(i)
            if !code[i + 1..].is_empty() && code[i + 1..].chars().all(|c| c.is_ascii_digit()) =>
        {
            (&code[..i], &code[i + 1..])
        }
        _ => (code, ""),
    };
    let mut name = String::new();
    let mut parts = base.split('_');
    let (course, version, kind) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    match course {
        "nhce" => name.push_str("新视野大学英语"),
        _ => name.push_str(&format!("未知书本({})", course)),
    }
    if let Some(version) = version.strip_prefix("v") {
        name.push_str(&format!("(第{}版)", version));
    } else {
        name.push_str(&format!("未知版本({})", version));
    }
    match kind {
        "rw" => name.push_str("读写教程"),
        "ls" => name.push_str("视听说教程"),
        "ur" => name.push_str("视听说教程"),
        _ => name.push_str(&format!("未知课程({})", kind)),
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
            course_display_name_fallback("course-v2:75b7546ea002b72+nhce_v3_rw_4+20230116"),
            "新视野大学英语(第3版)读写教程 4"
        );
        assert_eq!(
            course_display_name_fallback("course-v2:75b7546ea002b72+nhce_v4_rw+20230116"),
            "新视野大学英语(第4版)读写教程"
        );
        assert_eq!(
            course_display_name_fallback("course-v2:x+yz_3+9"),
            "未知书本(yz)未知版本()未知课程() 3"
        );
    }
}
