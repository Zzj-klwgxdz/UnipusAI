use crate::api::course::GroupTask;
use crate::api::parser::ParsedGroup;
use anyhow::Result;

/// dump 数据目录（存放 SQLite 数据库 dump.db）。
pub const DUMP_DIR: &str = "dump_text";

/// 浏览类页面（内容为空/非 JSON/无题目模块）的 kind 标识。
pub const VIEW_ONLY_DIR: &str = "view-only";

/// 题型名：reply_type 优先，空回退 module_type；多题型去重后用 + 连接。
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

/// 名称安全化：替换不适合作为展示/键名的字符。
pub fn sanitize_dir_name(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

/// SQLite 数据库路径（dump_text/dump.db）。
pub fn db_path() -> std::path::PathBuf {
    crate::db::db_path()
}

/// 实时生成 dump 汇总文本（空库返回 None）。
pub fn summary_text() -> Result<Option<String>> {
    let conn = crate::db::open()?;
    crate::db::summary_text(&conn)
}

/// 将任务完成情况同步到数据库（供 run/group 提交成功后调用）；
/// 数据库中无该任务返回 false。
pub fn sync_task_status(task: &GroupTask, passed: bool) -> bool {
    match crate::db::open() {
        Ok(conn) => {
            crate::db::set_passed(&conn, &task.unit_id, &task.group_id, passed).unwrap_or(false)
        }
        Err(e) => {
            log::warn!("同步 dump 状态失败: {:#}", e);
            false
        }
    }
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
}
