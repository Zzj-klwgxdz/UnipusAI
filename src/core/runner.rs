use crate::api::content::{decrypt_content, fetch_content, parse_decrypted};
use crate::api::course::{
    GroupTask, build_tasks, fetch_course_progress, fetch_course_units, fetch_unit, select_tasks,
};
use crate::api::parser::parse_group;
use crate::api::session::Session;
use crate::api::submit::{
    RateLimited, build_answer_payload, build_mark_seen_payload, empty_answers, submit_raw,
};
use anyhow::{Result, bail};
use log::{error, info};

/// 提交并处理限频：命中限频则等待冷却后重试（仅重做提交，不重复 LLM 作答）。
async fn submit_with_rate_retry(session: &Session, payload: &str) -> Result<serde_json::Value> {
    const MAX_RETRIES: u32 = 5;
    const COOLDOWN_SECS: u64 = 180;
    let mut attempt = 0u32;
    loop {
        match submit_raw(session, payload).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if !e.is::<RateLimited>() {
                    return Err(e);
                }
                attempt += 1;
                if attempt >= MAX_RETRIES {
                    return Err(e);
                }
                let secs = COOLDOWN_SECS * (attempt as u64);
                log::warn!(
                    "触发限频，等待 {} 秒后重试 ({}/{}): {}",
                    secs,
                    attempt,
                    MAX_RETRIES,
                    e
                );
                tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            }
        }
    }
}

pub async fn process_group(session: &Session, task: &GroupTask) -> Result<serde_json::Value> {
    match task.tab_type.as_str() {
        "text" | "video" => {
            let payload = build_mark_seen_payload(session, &task.group_id)?;
            submit_with_rate_retry(session, &payload).await
        }
        "task" => {
            let rt = fetch_content(session, &task.group_id).await?;
            let plain = decrypt_content(&rt.content, &rt.k)?;
            let dec = parse_decrypted(&plain)?;
            let group = parse_group(&dec)?;
            let mut modules = empty_answers(&group);
            for (mi, m) in group.modules.iter().enumerate() {
                let values = crate::solve::solve_module(session, m).await?;
                for (ci, v) in values.into_iter().enumerate() {
                    if ci < modules[mi].children.len() {
                        modules[mi].children[ci].value = v;
                    }
                }
            }
            let payload = build_answer_payload(session, &task.group_id, &modules)?;
            submit_with_rate_retry(session, &payload).await
        }
        other => bail!("未知 tab_type: {}", other),
    }
}

pub async fn run_course(session: &mut Session, with_names: bool) -> Result<RunSummary> {
    let course = fetch_course_progress(session).await?;
    let version = course.publish_version.clone();
    if !version.is_empty() {
        session.set_publish_version(&version)?;
    }
    let units = fetch_course_units(session).await?;
    run_course_units(session, &units, with_names).await
}

pub async fn run_course_units(
    session: &mut Session,
    unit_ids: &[String],
    with_names: bool,
) -> Result<RunSummary> {
    let compulsory_only = session.cfg().compulsory_only();
    let mut summary = RunSummary::default();
    if with_names {
        info!(
            "课程: {}",
            crate::api::course::course_display_name(session.course_id())
        );
    }
    for (ui, unit_id) in unit_ids.iter().enumerate() {
        let rt = fetch_unit(session, unit_id).await?;
        let tasks = select_tasks(&build_tasks(unit_id, &rt), compulsory_only);
        if with_names {
            let label = crate::api::course::unit_label(session, unit_id)
                .await?
                .unwrap_or_else(|| format!("Unit {}", ui + 1));
            info!(
                "单元 {} ({}) ：任务 {} 个{}",
                unit_id,
                label,
                tasks.len(),
                if compulsory_only { " (仅必修)" } else { "" }
            );
        } else {
            info!(
                "单元 {} ：任务 {} 个{}",
                unit_id,
                tasks.len(),
                if compulsory_only { " (仅必修)" } else { "" }
            );
        }
        for task in &tasks {
            if task.passed {
                summary.skipped += 1;
                continue;
            }
            match process_group(session, task).await {
                Ok(resp) => {
                    summary.done += 1;
                    info!("[OK] {} {} -> {}", task.tab_type, task.group_id, resp);
                }
                Err(e) => {
                    summary.failed += 1;
                    error!("[FAIL] {} {} -> {:#}", task.tab_type, task.group_id, e);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(session.cfg().interval_ms)).await;
        }
    }
    Ok(summary)
}

pub async fn mock_task(session: &Session, group_id: &str) -> Result<GroupTask> {
    let units = fetch_course_units(session).await?;
    for unit_id in units {
        let rt = fetch_unit(session, &unit_id).await?;
        for (gid, leaf) in &rt.leafs {
            if gid == group_id {
                return Ok(GroupTask {
                    group_id: group_id.to_string(),
                    unit_id,
                    tab_type: if leaf.tab_type.is_empty() {
                        "task".to_string()
                    } else {
                        leaf.tab_type.clone()
                    },
                    required: leaf.strategies.required,
                    passed: leaf.state.pass >= 1,
                    min_score_pct: leaf.strategies.min_score_pct,
                    start_time: leaf.strategies.start_time,
                    end_time: leaf.strategies.end_time,
                });
            }
        }
    }
    anyhow::bail!("在所有单元中找不到 group {}", group_id);
}

#[derive(Debug, Default)]
pub struct RunSummary {
    pub skipped: u32,
    pub done: u32,
    pub failed: u32,
}
