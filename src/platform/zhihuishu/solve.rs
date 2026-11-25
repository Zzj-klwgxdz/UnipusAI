//! 知到作业的 LLM 作答（CLI 与 TUI 共用）。

use super::{ZsQuestion, ZsPaper};
use crate::config::Config;
use anyhow::Result;

pub fn letter(i: usize) -> char {
    (b'A' + (i as u8 % 26)) as char
}

pub fn build_question_prompt(q: &ZsQuestion) -> String {
    let mut lines = vec![format!("题目：{}", q.text)];
    let is_judge = q.type_name.contains("判断");
    if is_judge {
        lines.push("题型：判断题（回答“对”或“错”）".to_string());
    } else if q.type_name.contains("多选") {
        lines.push("题型：多选题（可能有多个正确答案）".to_string());
    } else {
        lines.push("题型：单选题".to_string());
    }
    lines.push("选项：".to_string());
    for (i, o) in q.options.iter().enumerate() {
        lines.push(format!("{}. {}", letter(i), o.text));
    }
    if q.type_name.contains("多选") {
        lines.push("请只输出正确选项的字母，多个用英文逗号分隔，如 A,C。".to_string());
    } else if is_judge {
        lines.push("请只输出“对”或“错”。".to_string());
    } else {
        lines.push("请只输出一个字母（如 B）。".to_string());
    }
    lines.join("\n")
}

/// 让模型作答一题，返回选项 id 列表。
pub async fn solve_question(cfg: &Config, q: &ZsQuestion) -> Result<Vec<i64>> {
    let prompt = build_question_prompt(q);
    let reply =
        crate::llm::ask_with(cfg, "你是课程答题助手，只输出答案，不要解释。", &prompt).await?;
    let mut ids = Vec::new();
    if q.type_name.contains("判断") {
        let want_true = reply.contains('对') && !reply.contains('错');
        let want_false = reply.contains('错');
        let hit = q
            .options
            .iter()
            .find(|o| {
                let t = o.text.as_str();
                let is_true = t.contains('对') || t.contains("正确");
                let is_false = t.contains('错') || t.contains("错误");
                match (want_true, want_false) {
                    (true, false) => is_true,
                    (false, true) => is_false,
                    _ => false,
                }
            })
            .cloned();
        if let Some(o) = hit {
            ids.push(o.id);
        }
    } else {
        for ch in reply.chars() {
            if ch.is_ascii_alphabetic() {
                let idx = (ch.to_ascii_uppercase() as u8 - b'A') as usize;
                if let Some(o) = q.options.get(idx)
                    && !ids.contains(&o.id)
                {
                    ids.push(o.id);
                }
            }
        }
    }
    if ids.is_empty() {
        if let Some(o) = q.options.first() {
            log::warn!("模型回复无法解析（{}），回退到第一个选项", reply.trim());
            ids.push(o.id);
        } else {
            anyhow::bail!("题目没有选项");
        }
    }
    Ok(ids)
}

/// 把选项 id 还原成便于显示的文本。
pub fn answer_text(q: &ZsQuestion, ids: &[i64]) -> String {
    ids.iter()
        .filter_map(|id| q.options.iter().find(|o| o.id == *id).map(|o| o.text.clone()))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// 整卷作答（不保存/不提交）。
pub async fn solve_paper(cfg: &Config, paper: &ZsPaper) -> Vec<(String, Vec<i64>)> {
    let mut out = Vec::new();
    for q in &paper.questions {
        match solve_question(cfg, q).await {
            Ok(ids) => out.push((q.eid.clone(), ids)),
            Err(e) => {
                log::warn!("作答失败（{}）: {:#}", q.eid, e);
                out.push((q.eid.clone(), Vec::new()));
            }
        }
    }
    out
}
