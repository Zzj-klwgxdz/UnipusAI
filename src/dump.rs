use crate::api::course::GroupTask;
use crate::api::parser::ParsedGroup;
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// dump-text 输出目录。
pub const DUMP_DIR: &str = "dump_text";

/// 归档题型目录名：reply_type 优先，空回退 module_type；多题型去重后用 + 连接。
pub fn group_type_name(group: &ParsedGroup) -> String {
    let mut names: Vec<String> = Vec::new();
    for m in &group.modules {
        let raw = if m.reply_type.is_empty() {
            m.module_type.as_str()
        } else {
            m.reply_type.as_str()
        };
        let name = sanitize_dir_name(raw);
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    if names.is_empty() {
        "unknown".to_string()
    } else {
        names.join("+")
    }
}

/// 目录名安全化：替换文件系统不允许的字符。
pub fn sanitize_dir_name(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

/// dump 文件首行（含必修/完成状态）。
pub fn dump_header(
    unit_id: &str,
    gid: &str,
    tab_type: &str,
    required: bool,
    passed: bool,
) -> String {
    format!(
        "==== 单元 {} / 任务组 {} ({}) | {} | {} ====",
        unit_id,
        gid,
        tab_type,
        if required { "必修" } else { "选修" },
        if passed { "已完成" } else { "未完成" }
    )
}

/// 解析首行状态：返回 (是否必修, 是否已完成)；旧格式（无状态）返回 None。
pub fn parse_status(line: &str) -> Option<(bool, bool)> {
    let required = if line.contains("| 必修 |") {
        true
    } else if line.contains("| 选修 |") {
        false
    } else {
        return None;
    };
    let passed = if line.contains("| 已完成 ====") {
        true
    } else if line.contains("| 未完成 ====") {
        false
    } else {
        return None;
    };
    Some((required, passed))
}

/// 替换文本首行；内容相同返回 None（无需写盘）。
pub fn replace_first_line(content: &str, header: &str) -> Option<String> {
    let rest = match content.find('\n') {
        Some(pos) => &content[pos..],
        None => "",
    };
    let new = format!("{}{}", header, rest);
    (new != content).then_some(new)
}

/// 扫描单元目录下各题型子目录，建立 gid -> 文件路径 映射。
pub fn scan_unit_files(unit_dir: &Path) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    let Ok(rd) = std::fs::read_dir(unit_dir) else {
        return map;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let sub = entry.path();
        if !sub.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&sub) else {
            continue;
        };
        for f in files.filter_map(|e| e.ok()) {
            let fp = f.path();
            if fp.extension().map(|x| x == "txt").unwrap_or(false)
                && let Some(stem) = fp.file_stem().and_then(|s| s.to_str())
            {
                map.entry(stem.to_string()).or_insert(fp);
            }
        }
    }
    map
}

/// 递归收集 dump_text 下的 .txt 相对路径（/ 分隔），排除 _summary.txt。
pub fn collect_dump_files(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            collect_dump_files(&path, root, out);
        } else if path.extension().map(|x| x == "txt").unwrap_or(false)
            && path.file_name() != Some(std::ffi::OsStr::new("_summary.txt"))
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// 找到某单元对应的 dump 目录（目录名以 `_{unit_id}` 结尾）。
fn unit_dirs(unit_id: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let Ok(rd) = std::fs::read_dir(DUMP_DIR) else {
        return dirs;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_dir()
            && p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(&format!("_{}", unit_id)))
        {
            dirs.push(p);
        }
    }
    dirs
}

fn update_file_header(path: &Path, header: &str) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Some(new) = replace_first_line(&content, header) else {
        return false;
    };
    std::fs::write(path, new).is_ok()
}

/// 同步单个任务状态到 dump 文件首行；文件不存在或状态未变返回 false。
pub fn update_task_status(
    unit_id: &str,
    gid: &str,
    tab_type: &str,
    required: bool,
    passed: bool,
) -> bool {
    update_unit_statuses(unit_id, &[(gid, tab_type, required, passed)]) > 0
}

/// 批量同步同一单元下多个任务的状态（单元目录只扫描一次）；返回更新条数。
/// tasks: (groupId, tabType, required, passed)
pub fn update_unit_statuses(unit_id: &str, tasks: &[(&str, &str, bool, bool)]) -> usize {
    let dirs = unit_dirs(unit_id);
    if dirs.is_empty() {
        return 0;
    }
    let mut files: HashMap<String, PathBuf> = HashMap::new();
    for dir in &dirs {
        for (gid, path) in scan_unit_files(dir) {
            files.entry(gid).or_insert(path);
        }
    }
    let mut updated = 0;
    for (gid, tab_type, required, passed) in tasks {
        if let Some(path) = files.get(*gid)
            && update_file_header(path, &dump_header(unit_id, gid, tab_type, *required, *passed))
        {
            updated += 1;
        }
    }
    updated
}

/// 将任务完成情况同步到 dump 文件（供 run/group 提交成功后调用）。
pub fn sync_task_status(task: &GroupTask, passed: bool) -> bool {
    update_task_status(
        &task.unit_id,
        &task.group_id,
        &task.tab_type,
        task.required,
        passed,
    )
}

/// 从文件首行读取 (是否必修, 是否已完成)。
fn read_status(path: &Path) -> Option<(bool, bool)> {
    let content = std::fs::read_to_string(path).ok()?;
    parse_status(content.lines().next()?)
}

/// UTC 时间字符串（依赖 std，无额外时区处理）。
fn now_utc_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 由“1970-01-01 起的天数”换算 (年, 月, 日)，Howard Hinnant 算法。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

/// 生成/刷新 dump_text/_summary.txt 状态报告（扫描全部文件首行）；返回文件数。
pub fn write_summary() -> Result<usize> {
    let root = Path::new(DUMP_DIR);
    if !root.exists() {
        return Ok(0);
    }
    let mut files: Vec<String> = Vec::new();
    collect_dump_files(root, root, &mut files);
    files.sort();
    files.dedup();

    let mut total_req = 0usize;
    let mut total_req_done = 0usize;
    let mut total_opt = 0usize;
    let mut total_opt_done = 0usize;
    let mut unknown = 0usize;
    let mut units: BTreeMap<String, (usize, usize, usize, usize)> = BTreeMap::new();
    let mut entries: Vec<(String, String)> = Vec::new();

    for rel in &files {
        let path = root.join(rel);
        let Some((required, passed)) = read_status(&path) else {
            unknown += 1;
            entries.push((rel.clone(), "未知".to_string()));
            continue;
        };
        let unit = rel
            .split_once('/')
            .map(|(u, _)| u.to_string())
            .unwrap_or_else(|| "-".to_string());
        let agg = units.entry(unit).or_insert((0, 0, 0, 0));
        if required {
            total_req += 1;
            agg.0 += 1;
            if passed {
                total_req_done += 1;
                agg.1 += 1;
            }
        } else {
            total_opt += 1;
            agg.2 += 1;
            if passed {
                total_opt_done += 1;
                agg.3 += 1;
            }
        }
        entries.push((
            rel.clone(),
            format!(
                "{} {}",
                if required { "必修" } else { "选修" },
                if passed { "已完成" } else { "未完成" }
            ),
        ));
    }

    let mut out = String::new();
    out.push_str(&format!("dump-text 状态汇总 (更新时间: {})\n", now_utc_string()));
    out.push_str(&format!(
        "任务组文件: {} 个 | 必修: {} (已完成 {}, 未完成 {}) | 选修: {} (已完成 {}, 未完成 {})",
        files.len(),
        total_req,
        total_req_done,
        total_req - total_req_done,
        total_opt,
        total_opt_done,
        total_opt - total_opt_done
    ));
    if unknown > 0 {
        out.push_str(&format!(" | 未识别状态: {}", unknown));
    }
    out.push_str("\n\n按单元:\n");
    for (unit, (r, rd, o, od)) in &units {
        out.push_str(&format!(
            "  {}: 必修 {} (已完成 {}) / 选修 {} (已完成 {})\n",
            unit, r, rd, o, od
        ));
    }
    out.push_str("\n文件清单:\n");
    for (path, status) in &entries {
        out.push_str(&format!("  {} [{}]\n", path, status));
    }
    std::fs::write(root.join("_summary.txt"), out)?;
    Ok(files.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::parser::Module;

    fn module(module_type: &str, reply_type: &str) -> Module {
        Module {
            instance_id: "m".into(),
            module_type: module_type.into(),
            direction: String::new(),
            material: String::new(),
            media_sources: Vec::new(),
            transcript: String::new(),
            reply_type: reply_type.into(),
            word_bank: Vec::new(),
            children: Vec::new(),
        }
    }

    fn group(modules: Vec<Module>) -> ParsedGroup {
        ParsedGroup { modules }
    }

    #[test]
    fn type_name_prefers_reply_type() {
        assert_eq!(
            group_type_name(&group(vec![module("basic", "singlechoice")])),
            "singlechoice"
        );
        assert_eq!(
            group_type_name(&group(vec![module("material-banked-cloze", "bankedcloze")])),
            "bankedcloze"
        );
    }

    #[test]
    fn type_name_falls_back_to_module_type() {
        assert_eq!(
            group_type_name(&group(vec![module("vocabulary", "")])),
            "vocabulary"
        );
        assert_eq!(
            group_type_name(&group(vec![module("video-popup", "")])),
            "video-popup"
        );
    }

    #[test]
    fn type_name_dedupes_and_joins() {
        assert_eq!(
            group_type_name(&group(vec![
                module("basic", "bankedcloze"),
                module("basic", "bankedcloze"),
            ])),
            "bankedcloze"
        );
        assert_eq!(
            group_type_name(&group(vec![
                module("basic", "bankedcloze"),
                module("basic", "singlechoice"),
            ])),
            "bankedcloze+singlechoice"
        );
        assert_eq!(group_type_name(&group(vec![module("", "")])), "unknown");
    }

    #[test]
    fn dir_name_sanitized() {
        assert_eq!(sanitize_dir_name("a/b:c*d?"), "a_b_c_d_");
        assert_eq!(sanitize_dir_name("bankedcloze"), "bankedcloze");
    }

    #[test]
    fn header_status_roundtrip() {
        let h = dump_header("6b5af82a100090b", "6b816769400090b", "task", true, false);
        assert!(h.contains("| 必修 | 未完成 ===="));
        assert_eq!(parse_status(&h), Some((true, false)));

        let h2 = dump_header("u", "g", "text", false, true);
        assert_eq!(parse_status(&h2), Some((false, true)));
        assert_eq!(parse_status("==== 单元 u / 任务组 g (task) ===="), None);
    }

    #[test]
    fn replace_first_line_cases() {
        let old = "==== 旧首行 ====\n第二行\n第三行";
        let new = replace_first_line(old, "==== 新首行 ====").unwrap();
        assert_eq!(new, "==== 新首行 ====\n第二行\n第三行");
        // 相同内容不写盘
        assert_eq!(replace_first_line(&new, "==== 新首行 ===="), None);
        // 空文件
        assert_eq!(replace_first_line("", "H"), Some("H".to_string()));
    }

    #[test]
    fn civil_from_days_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(1), (1970, 1, 2));
        // 2026-09-23 距 1970-01-01 的天数（含闰年）
        assert_eq!(civil_from_days(20719), (2026, 9, 23));
    }
}
