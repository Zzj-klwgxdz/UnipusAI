//! 平台无关的数据模型。
//!
//! 所有网课平台（U 校园 / 学习通 / 未来接入的其它平台）都把数据转换成这里的结构，
//! 上层（CLI/TUI/计划器）只依赖这些类型，不直接依赖某个平台的字段。

use serde::{Deserialize, Serialize};

/// 站点标识（与 config::Site 对应，但这里用字符串便于持久化）。
pub type SiteId = &'static str;

/// 课程。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Course {
    /// 平台标识（unipusai / chaoxing）。
    pub site: String,
    /// 平台内的课程 id。
    pub course_id: String,
    /// 课程名。
    pub name: String,
    /// 教师/学校等展示信息。
    pub teacher: String,
    /// 平台附加标识（clazzid/cpi/enc/bbsid 等），原样保存。
    #[serde(default)]
    pub extra: serde_json::Value,
}

impl Course {
    /// 取平台附加字段（字符串形式）。
    pub fn extra_str(&self, key: &str) -> String {
        self.extra
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    }
}

/// 章节（学习通里对应一个 knowledgeId）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Chapter {
    pub id: String,
    /// 一级章节名，如 "绪论"。
    pub parent_title: String,
    /// 章节名，如 "高校实验室概述"。
    pub title: String,
    /// 编号，如 "1.1"。
    pub number: String,
    pub completed: bool,
    /// 该章未完成任务点数（平台提供时）。
    pub unfinished: Option<u32>,
}

/// 任务点类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TaskKind {
    Video,
    Quiz,
    Document,
    Discussion,
    #[default]
    Other,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::Video => "video",
            TaskKind::Quiz => "quiz",
            TaskKind::Document => "document",
            TaskKind::Discussion => "discussion",
            TaskKind::Other => "other",
        }
    }
}

/// 任务点（学习通里对应一个 mArg attachment）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskPoint {
    /// 平台内 id（视频=jobid，测验=workid）。
    pub id: String,
    pub kind: TaskKind,
    pub title: String,
    /// 视频时长（秒）。
    pub duration: Option<u64>,
    pub finished: bool,
    /// 平台附加字段（objectId/jobid/enc/otherInfo/attDuration 等）。
    #[serde(default)]
    pub extra: serde_json::Value,
}

impl TaskPoint {
    pub fn extra_str(&self, key: &str) -> String {
        match self.extra.get(key) {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(v) if !v.is_null() => v.to_string(),
            _ => String::new(),
        }
    }

    pub fn extra_i64(&self, key: &str) -> Option<i64> {
        match self.extra.get(key) {
            Some(serde_json::Value::Number(n)) => n.as_i64(),
            Some(serde_json::Value::String(s)) => s.parse().ok(),
            _ => None,
        }
    }
}
