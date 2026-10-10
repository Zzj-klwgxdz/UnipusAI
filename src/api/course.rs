use crate::api::session::{Session, progress_url, unit_progress_url};
use anyhow::Result;
use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 反序列化兜底：字段缺失或显式 `null` 时使用类型默认值。
/// （`#[serde(default)]` 只处理缺失，不处理服务端返回的显式 null）
fn null_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize)]
pub struct CourseProgressResponse {
    pub rt: CourseProgressRt,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CourseProgressRt {
    #[serde(default)]
    pub units: BTreeMap<String, CourseUnitEntry>,
    #[serde(default, deserialize_with = "null_default")]
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
    #[serde(default, deserialize_with = "null_default")]
    pub duration_time: i64,
    #[serde(default, deserialize_with = "null_default")]
    pub flag: String,
    #[serde(default)]
    pub leafs: BTreeMap<String, LeafEntry>,
    #[serde(default)]
    pub micros: BTreeMap<String, MicroEntry>,
    #[serde(default, deserialize_with = "null_default")]
    pub open_id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub publish_version: String,
    #[serde(default, rename = "tutorialId", deserialize_with = "null_default")]
    pub tutorial_id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub unit_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LeafEntry {
    #[serde(default, deserialize_with = "null_default")]
    pub duration: i64,
    pub state: LeafState,
    pub strategies: Strategies,
    #[serde(default)]
    pub tab_type: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct LeafState {
    #[serde(default, deserialize_with = "null_default")]
    pub pass: u8,
    #[serde(default, deserialize_with = "null_default")]
    pub pass2: u8,
    #[serde(default, deserialize_with = "null_default")]
    pub perm: u8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Strategies {
    #[serde(default)]
    pub end_time: Option<i64>,
    #[serde(default)]
    pub min_score_pct: Option<i32>,
    #[serde(default, deserialize_with = "null_default")]
    pub required: bool,
    #[serde(default)]
    pub start_time: Option<i64>,
    #[serde(default, deserialize_with = "null_default")]
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
    let url = unit_progress_url(session.course_id(), unit_id, &session.open_id());
    let resp: ProgressResponse = session.get_json(&url).await?;
    Ok(resp.rt)
}

pub async fn fetch_course_progress(session: &Session) -> Result<CourseProgressRt> {
    let url = progress_url(session.course_id(), &session.open_id());
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
            min_score_pct: leaf.strategies.min_score_pct.unwrap_or(0),
            start_time: leaf.strategies.start_time.unwrap_or(0),
            end_time: leaf.strategies.end_time.unwrap_or(0),
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

/// 账号下的一门课程（首页课程列表解析结果）。
#[derive(Debug, Clone)]
pub struct HomeCourse {
    pub name: String,
    pub course_id: String,
    /// 班级 id（courseList[].classId），讨论题 BBS 接口使用。
    pub class_id: String,
    /// 课程组 id（courseList[].id），即页面 URL 的 cloudCurriculaId。
    pub curricula_id: String,
    /// 来源标签（首页列表为空时来自"我的教材"：班级课程/个人学习），首页列表为空字符串。
    pub group_label: String,
}

/// 解析首页课程列表响应（抽出便于单测）。
pub fn parse_home_courses(v: &serde_json::Value) -> Vec<HomeCourse> {
    let mut out = Vec::new();
    let Some(courses) = v.pointer("/value/courseList").and_then(|c| c.as_array()) else {
        return out;
    };
    for c in courses {
        let class_id = c
            .get("classId")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let curricula_id = c
            .get("id")
            .map(|x| match x {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        let fallback_name = c
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let Some(res_list) = c.get("courseResourceList").and_then(|r| r.as_array()) else {
            continue;
        };
        for res in res_list {
            let course_id = res
                .get("instanceId")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if course_id.is_empty() {
                continue;
            }
            let name = res
                .get("name")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| fallback_name.clone());
            out.push(HomeCourse {
                name,
                course_id,
                class_id: class_id.clone(),
                curricula_id: curricula_id.clone(),
                group_label: String::new(),
            });
        }
    }
    out
}

/// 拉取当前账号的全部课程：优先首页课程列表；为空时回退"我的教材"（书架）。
pub async fn fetch_home_courses(session: &Session) -> Result<Vec<HomeCourse>> {
    let body = session
        .get_bytes(HOME_COURSE_LIST_URL)
        .await
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| anyhow::anyhow!("获取课程列表失败: {:#}", e))?;
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("课程列表响应非 JSON: {:#}", e))?;
    let ok = v.get("code").and_then(|c| c.as_i64()) == Some(1)
        || v.get("success").and_then(|s| s.as_bool()) == Some(true);
    if !ok {
        anyhow::bail!(
            "课程列表接口返回异常: {}",
            crate::api::parser::truncate_text(&body, 200)
        );
    }
    log::debug!("课程列表原始响应: {}", crate::api::parser::truncate_text(&body, 4000));
    let list = parse_home_courses(&v);
    if !list.is_empty() {
        return Ok(list);
    }
    // 首页列表为空（部分账号服务端不返回）：改用"我的教材"兜底
    let bookshelf = fetch_bookshelf_courses(session).await?;
    if bookshelf.is_empty() {
        log::warn!("首页课程列表与我的教材均为空（账号可能未加入班级/激活教材）");
    } else {
        log::info!(
            "首页课程列表为空，改用我的教材列表（{} 门）",
            bookshelf.len()
        );
    }
    Ok(bookshelf)
}

/// "我的教材"接口（首页课程列表为空时的兜底来源）。
const BOOKSHELF_URL: &str = "https://uai.unipus.cn/api/cmgt/course/my/bookshelf";

/// 拉取并解析"我的教材"中的课程（班级课程 + 个人学习，跳过已过期）。
pub async fn fetch_bookshelf_courses(session: &Session) -> Result<Vec<HomeCourse>> {
    let body = session
        .get_bytes(BOOKSHELF_URL)
        .await
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| anyhow::anyhow!("获取我的教材失败: {:#}", e))?;
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("我的教材响应非 JSON: {:#}", e))?;
    let ok = v.get("code").and_then(|c| c.as_i64()) == Some(1)
        || v.get("success").and_then(|s| s.as_bool()) == Some(true);
    if !ok {
        anyhow::bail!(
            "我的教材接口返回异常: {}",
            crate::api::parser::truncate_text(&body, 200)
        );
    }
    log::debug!("我的教材原始响应: {}", crate::api::parser::truncate_text(&body, 4000));
    Ok(parse_bookshelf_courses(&v))
}

/// 解析"我的教材"响应：CLASS_COURSE（班级课程）优先，其次 PERSONAL（个人学习），
/// 跳过 EXPIRED；按 course_id 去重（班级课程优先）。
pub fn parse_bookshelf_courses(v: &serde_json::Value) -> Vec<HomeCourse> {
    let mut out: Vec<HomeCourse> = Vec::new();
    let Some(groups) = v.pointer("/value/groups").and_then(|g| g.as_object()) else {
        return out;
    };
    // 固定顺序：班级课程 → 个人学习（EXPIRED 跳过）
    for (key, label) in [
        ("CLASS_COURSE", "班级课程"),
        ("PERSONAL", "个人学习"),
    ] {
        let Some(items) = groups.get(key).and_then(|x| x.as_array()) else {
            continue;
        };
        for item in items {
            let course_id = item
                .get("instanceId")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if course_id.is_empty() {
                continue;
            }
            let name = item
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            // classCourses 实际为数组（可能多班级），取第一个；兼容对象形式
            let (class_id, curricula_id) = item
                .get("classCourses")
                .and_then(|cc| match cc {
                    serde_json::Value::Array(arr) => arr.first().cloned(),
                    other => Some(other.clone()),
                })
                .map(|cc| {
                    (
                        cc.get("classId")
                            .map(|x| match x {
                                serde_json::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            })
                            .unwrap_or_default(),
                        cc.get("courseId")
                            .map(|x| match x {
                                serde_json::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            })
                            .unwrap_or_default(),
                    )
                })
                .unwrap_or_default();
            if out.iter().any(|c| c.course_id == course_id) {
                continue; // 班级课程优先，已存在则不覆盖
            }
            out.push(HomeCourse {
                name,
                course_id,
                class_id,
                curricula_id,
                group_label: label.to_string(),
            });
        }
    }
    out
}

/// 账号信息接口（u-school 头所需学校编号）。
const ACCOUNT_USER_INFO_URL: &str = "https://uai.unipus.cn/api/account/user/info";

/// 拉取账号学校编号（value.userInfo.school，如 "8320"）；无则返回 None。
pub async fn fetch_user_school(session: &Session) -> Result<Option<String>> {
    let body = session
        .get_bytes(ACCOUNT_USER_INFO_URL)
        .await
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| anyhow::anyhow!("获取账号信息失败: {:#}", e))?;
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("账号信息响应非 JSON: {:#}", e))?;
    let ok = v.get("code").and_then(|c| c.as_i64()) == Some(1)
        || v.get("success").and_then(|s| s.as_bool()) == Some(true);
    if !ok {
        anyhow::bail!(
            "账号信息接口返回异常: {}",
            crate::api::parser::truncate_text(&body, 200)
        );
    }
    Ok(v.pointer("/value/userInfo/school")
        .map(|x| match x {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .filter(|s| !s.is_empty()))
}

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
        "vls" => name.push_str("视听说教程"),
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

    #[test]
    fn progress_tolerates_null_and_missing_fields() {
        // 复现 bug1：服务端对未发布任务返回 strategies.start_time/end_time = null
        let v = serde_json::json!({
            "rt": {
                "duration_time": null,
                "flag": null,
                "open_id": null,
                "publish_version": "139753",
                "tutorialId": null,
                "leafs": {
                    "g-null": {
                        "duration": null,
                        "state": {"pass": null, "pass2": 0, "perm": null},
                        "strategies": {"end_time": null, "min_score_pct": null, "required": null, "start_time": null, "statistic_mode_out": null},
                        "tab_type": "task"
                    },
                    "g-missing": {
                        "state": {},
                        "strategies": {},
                        "tab_type": "text"
                    },
                    "g-normal": {
                        "state": {"pass": 1, "pass2": 0, "perm": 1},
                        "strategies": {"end_time": 1791647999, "min_score_pct": 60, "required": true, "start_time": 1788710400, "statistic_mode_out": false},
                        "tab_type": "video",
                        "duration": 120
                    }
                }
            }
        });
        let resp: ProgressResponse =
            serde_json::from_value(v).expect("null/缺失字段应可正常解析");
        assert_eq!(resp.rt.duration_time, 0);
        assert_eq!(resp.rt.publish_version, "139753");

        let tasks = build_tasks("u1", &resp.rt);
        let t_null = tasks.iter().find(|t| t.group_id == "g-null").unwrap();
        assert_eq!(
            (t_null.min_score_pct, t_null.start_time, t_null.end_time),
            (0, 0, 0)
        );
        assert!(!t_null.required && !t_null.passed);
        let t_missing = tasks.iter().find(|t| t.group_id == "g-missing").unwrap();
        assert_eq!(
            (t_missing.min_score_pct, t_missing.start_time, t_missing.end_time),
            (0, 0, 0)
        );
        let t_norm = tasks.iter().find(|t| t.group_id == "g-normal").unwrap();
        assert!(t_norm.required && t_norm.passed);
        assert_eq!(
            (t_norm.min_score_pct, t_norm.start_time, t_norm.end_time),
            (60, 1788710400, 1791647999)
        );

        // 课程级进度：publish_version 为 null 也可解析
        let cv = serde_json::json!({"rt": {"units": {}, "publish_version": null}});
        let c: CourseProgressResponse =
            serde_json::from_value(cv).expect("null publish_version 应可解析");
        assert!(c.rt.publish_version.is_empty());
    }

    #[test]
    fn parse_home_courses_extracts_fields() {
        let v = serde_json::json!({
            "code": 1,
            "value": {
                "courseList": [
                    {
                        "id": 369622,
                        "name": "教材课程",
                        "classId": "1840882666005127243",
                        "courseResourceList": [
                            {"instanceId": "course-v2:a+nhce_v4_rw_3+20230116", "name": "新视野3"},
                            {"instanceId": "course-v2:b+nhce_v4_rw_2+20230116", "name": ""}
                        ]
                    },
                    {
                        "id": "888",
                        "classId": "999",
                        "courseResourceList": [
                            {"instanceId": "course-v2:c+x+1", "name": ""}
                        ]
                    }
                ]
            }
        });
        let list = parse_home_courses(&v);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "新视野3");
        assert_eq!(list[0].course_id, "course-v2:a+nhce_v4_rw_3+20230116");
        assert_eq!(list[0].class_id, "1840882666005127243");
        assert_eq!(list[0].curricula_id, "369622");
        // 资源名为空回退 courseList 名称；无 courseList 名称则为空
        assert_eq!(list[1].name, "教材课程");
        assert_eq!(list[2].name, "");
        assert_eq!(list[2].class_id, "999");
        assert_eq!(list[2].curricula_id, "888");

        assert!(parse_home_courses(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn parse_bookshelf_courses_orders_and_dedupes() {
        // 结构摘自真实"我的教材"响应（账号：班级课程 1 门 + 个人 2 门 + 过期 0）
        let v = serde_json::json!({
            "code": 1,
            "value": {
                "groups": {
                    "CLASS_COURSE": [{
                        "resourceId": "course-v2:Unipus+nhce_v4_rw_3+20230116",
                        "instanceId": "course-v2:75b7546e9012b72+nhce_v4_rw_3+20230116",
                        "name": "新视野3(班级)",
                        "activation": 0,
                        "classCourses": [{"courseId": 369619, "classId": 1840882666005127252i64}]
                    }],
                    "PERSONAL": [
                        {"instanceId": "course-v2:75b7546e9002b72+nhce_v4_rw_1+20230116", "name": "新视野1"},
                        {"instanceId": "course-v2:75b7546ea002b72+nhce_v4_rw_2+20230116", "name": "新视野2"}
                    ],
                    "EXPIRED": [
                        {"instanceId": "course-v2:old+nhce_v4_rw_0+20230116", "name": "过期课"}
                    ]
                }
            }
        });
        let list = parse_bookshelf_courses(&v);
        assert_eq!(list.len(), 3, "过期课程应跳过");
        assert_eq!(list[0].group_label, "班级课程");
        assert_eq!(list[0].class_id, "1840882666005127252");
        assert_eq!(list[0].curricula_id, "369619");
        assert_eq!(list[1].group_label, "个人学习");
        assert!(list[1].class_id.is_empty());
        assert!(!list.iter().any(|c| c.name == "过期课"));

        // 同一课程同时出现在班级与个人组 → 班级优先、去重；classCourses 兼容对象形式
        let v2 = serde_json::json!({
            "value": {"groups": {
                "CLASS_COURSE": [{"instanceId": "course-v2:x", "name": "班级", "classCourses": {"classId": "c1", "courseId": "k1"}}],
                "PERSONAL": [{"instanceId": "course-v2:x", "name": "个人"}]
            }}
        });
        let list2 = parse_bookshelf_courses(&v2);
        assert_eq!(list2.len(), 1);
        assert_eq!(list2[0].group_label, "班级课程");
        assert_eq!(list2[0].name, "班级");

        assert!(parse_bookshelf_courses(&serde_json::json!({})).is_empty());
    }
}
