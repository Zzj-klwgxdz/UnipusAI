use crate::api::bbs;
use crate::api::content::{decrypt_content, fetch_content, parse_decrypted};
use crate::api::course::{
    GroupTask, build_tasks, fetch_course_progress, fetch_course_units, fetch_unit, select_tasks,
};
use crate::api::parser::{Module, ParsedGroup, parse_group};
use crate::api::session::Session;
use crate::api::submit::{
    RateLimited, build_answer_payload, build_mark_seen_payload, empty_answers, submit_raw,
};
use crate::reporter::{ReportEvent, Reporter};
use anyhow::{Result, bail};
use log::info;
use tokio_util::sync::CancellationToken;

/// 提交并处理限频：命中限频则等待冷却后重试（仅重做提交，不重复 LLM 作答）。
/// 等待期间可被 cancel 中断。
async fn submit_with_rate_retry(
    session: &Session,
    payload: &str,
    reporter: &dyn Reporter,
    cancel: &CancellationToken,
) -> Result<serde_json::Value> {
    const MAX_RETRIES: u32 = 5;
    const COOLDOWN_SECS: u64 = 180;
    let mut attempt = 0u32;
    loop {
        if cancel.is_cancelled() {
            bail!("已取消");
        }
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
                reporter.report(ReportEvent::RateLimited {
                    attempt,
                    max: MAX_RETRIES,
                    wait_secs: secs,
                    message: e.to_string(),
                });
                tokio::select! {
                    _ = cancel.cancelled() => bail!("已取消"),
                    _ = tokio::time::sleep(std::time::Duration::from_secs(secs)) => {}
                }
            }
        }
    }
}

pub async fn process_group(
    session: &Session,
    task: &GroupTask,
    reporter: &dyn Reporter,
    cancel: &CancellationToken,
) -> Result<serde_json::Value> {
    reporter.report(ReportEvent::TaskStart {
        group_id: task.group_id.clone(),
        tab_type: task.tab_type.clone(),
    });
    let resp = process_group_inner(session, task, reporter, cancel).await?;
    // 答题成功后同步 dump 状态（无对应记录则忽略）
    if crate::dump::sync_task_status(session, task, true) {
        log::debug!("dump 状态已更新: {} -> 已完成", task.group_id);
    }
    reporter.report(ReportEvent::TaskDone {
        group_id: task.group_id.clone(),
        tab_type: task.tab_type.clone(),
        detail: resp.to_string(),
    });
    Ok(resp)
}

async fn process_group_inner(
    session: &Session,
    task: &GroupTask,
    reporter: &dyn Reporter,
    cancel: &CancellationToken,
) -> Result<serde_json::Value> {
    match task.tab_type.as_str() {
        "text" | "video" => {
            // 讨论题可能挂在 text/video 叶子下：尝试解析内容，检测到 discussion 则先发帖。
            if let Some(group) = try_parse_group(session, &task.group_id).await
                && group.modules.iter().any(|m| m.reply_type == "discussion")
            {
                post_discussion_comments(session, &task.group_id, &group).await?;
            }
            let payload = build_mark_seen_payload(session, &task.group_id)?;
            submit_with_rate_retry(session, &payload, reporter, cancel).await
        }
        "task" => {
            let rt = fetch_content(session, &task.group_id).await?;
            let plain = decrypt_content(&rt.content, &rt.k)?;
            // 内容为空/非 JSON/无题目模块的“浏览类页面”：与浏览器一致，直接标记已看。
            let group = match parse_decrypted(&plain)
                .ok()
                .and_then(|dec| parse_group(&dec).ok())
            {
                Some(group) => group,
                None => {
                    log::warn!(
                        "任务组 {} 内容为空/非题目模块，按“浏览即完成”标记已看",
                        task.group_id
                    );
                    let payload = build_mark_seen_payload(session, &task.group_id)?;
                    return submit_with_rate_retry(session, &payload, reporter, cancel).await;
                }
            };

            if group.modules.iter().any(|m| m.reply_type == "discussion") {
                post_discussion_comments(session, &task.group_id, &group).await?;
            }

            let mut modules = empty_answers(&group);
            for (mi, m) in group.modules.iter().enumerate() {
                if m.reply_type == "discussion" {
                    continue;
                }
                let values = crate::solve::solve_module(session, m).await?;
                for (ci, v) in values.into_iter().enumerate() {
                    if ci < modules[mi].children.len() {
                        modules[mi].children[ci].value = v;
                    }
                }
            }

            // 无可答模块（纯讨论/单词卡朗读/视频弹题等）：与浏览器一致，空 quesDatas + submitType=2 标记完成。
            if !has_answerable_module(&group) {
                let payload = build_mark_seen_payload(session, &task.group_id)?;
                return submit_with_rate_retry(session, &payload, reporter, cancel).await;
            }

            // 混合组：剔除 discussion 模块后按普通答案提交。
            let discussion_ids: std::collections::HashSet<&str> = group
                .modules
                .iter()
                .filter(|m| m.reply_type == "discussion")
                .map(|m| m.instance_id.as_str())
                .collect();
            modules.retain(|m| !discussion_ids.contains(m.instance_id.as_str()));

            let payload = build_answer_payload(session, &task.group_id, &modules)?;
            submit_with_rate_retry(session, &payload, reporter, cancel).await
        }
        other => bail!("未知 tab_type: {}", other),
    }
}

/// 是否存在需要普通作答的子题模块（discussion 走讨论区发帖，不算普通作答）。
fn has_answerable_module(group: &ParsedGroup) -> bool {
    group
        .modules
        .iter()
        .any(|m| !m.children.is_empty() && m.reply_type != "discussion")
}

/// best-effort 解析任务组内容；text/video 叶子可能内容为空，失败返回 None。
async fn try_parse_group(session: &Session, group_id: &str) -> Option<ParsedGroup> {
    let rt = fetch_content(session, group_id).await.ok()?;
    let plain = decrypt_content(&rt.content, &rt.k).ok()?;
    let dec = parse_decrypted(&plain).ok()?;
    parse_group(&dec).ok()
}

/// 为组内所有 discussion 模块生成发言并发布到 BBS；已有本人回复则跳过。
pub async fn post_discussion_comments(
    session: &Session,
    group_id: &str,
    group: &ParsedGroup,
) -> Result<()> {
    for m in group.modules.iter().filter(|m| m.reply_type == "discussion") {
        let values = crate::solve::solve_module(session, m).await?;
        let comment = values.into_iter().next().unwrap_or_default().trim().to_string();
        if comment.is_empty() {
            bail!("讨论题 {} 生成内容为空，无法发帖", m.instance_id);
        }
        let title = discussion_title(m);
        let topic_id = bbs::ensure_topic(session, group_id, &title, &comment).await?;
        if bbs::has_own_reply(session, topic_id).await? {
            info!("讨论组 {} 主题 {} 已有本人回复，跳过发帖", group_id, topic_id);
            continue;
        }
        bbs::post_reply(session, topic_id, &comment).await?;
        info!("讨论组 {} 已发表评论 (topic {})", group_id, topic_id);
    }
    Ok(())
}

/// 创建主题时的标题：优先答题说明首行，其次讨论题目首行，截断 60 字。
fn discussion_title(m: &Module) -> String {
    let src = if !m.direction.is_empty() {
        m.direction.as_str()
    } else {
        m.material.as_str()
    };
    let line = src
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("Discussion");
    crate::api::parser::truncate_text(line, 60)
}

pub async fn run_course(
    session: &mut Session,
    with_names: bool,
    reporter: &dyn Reporter,
    cancel: &CancellationToken,
) -> Result<RunSummary> {
    let course = fetch_course_progress(session).await?;
    let version = course.publish_version.clone();
    if !version.is_empty() {
        session.set_publish_version(&version)?;
    }
    let units = fetch_course_units(session).await?;
    run_course_units(session, &units, with_names, reporter, cancel).await
}

pub async fn run_course_units(
    session: &mut Session,
    unit_ids: &[String],
    with_names: bool,
    reporter: &dyn Reporter,
    cancel: &CancellationToken,
) -> Result<RunSummary> {
    let compulsory_only = session.cfg().compulsory_only();
    let mut summary = RunSummary::default();
    if with_names {
        info!(
            "课程: {}",
            crate::api::course::course_display_name(session, session.course_id()).await
        );
    }
    'units: for (ui, unit_id) in unit_ids.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        let rt = fetch_unit(session, unit_id).await?;
        let tasks = select_tasks(&build_tasks(unit_id, &rt), compulsory_only);
        let label = if with_names {
            Some(
                crate::api::course::unit_label(session, unit_id)
                    .await?
                    .unwrap_or_else(|| format!("Unit {}", ui + 1)),
            )
        } else {
            None
        };
        reporter.report(ReportEvent::UnitStart {
            unit_id: unit_id.clone(),
            label,
            total: tasks.len(),
            required_only: compulsory_only,
        });
        for task in &tasks {
            if cancel.is_cancelled() {
                break 'units;
            }
            if task.passed {
                summary.skipped += 1;
                // 跳过已通过任务时也同步 dump 状态（可能是旧状态未更新）
                crate::dump::sync_task_status(session, task, true);
                reporter.report(ReportEvent::TaskSkipped {
                    group_id: task.group_id.clone(),
                    reason: "已通过".to_string(),
                });
                continue;
            }
            match process_group(session, task, reporter, cancel).await {
                Ok(_) => {
                    summary.done += 1;
                }
                Err(e) => {
                    if cancel.is_cancelled() {
                        break 'units;
                    }
                    summary.failed += 1;
                    reporter.report(ReportEvent::TaskFailed {
                        group_id: task.group_id.clone(),
                        tab_type: task.tab_type.clone(),
                        error: format!("{:#}", e),
                    });
                }
            }
            tokio::select! {
                _ = cancel.cancelled() => break 'units,
                _ = tokio::time::sleep(std::time::Duration::from_millis(session.cfg().interval_ms)) => {}
            }
        }
    }
    // 每个任务完成后已实时同步状态到数据库
    reporter.report(ReportEvent::RunFinished {
        done: summary.done,
        skipped: summary.skipped,
        failed: summary.failed,
    });
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
                    min_score_pct: leaf.strategies.min_score_pct.unwrap_or(0),
                    start_time: leaf.strategies.start_time.unwrap_or(0),
                    end_time: leaf.strategies.end_time.unwrap_or(0),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::parser::ChildQ;

    fn module(reply_type: &str, children: usize) -> Module {
        Module {
            instance_id: "m".into(),
            module_type: reply_type.into(),
            direction: String::new(),
            material: String::new(),
            media_sources: Vec::new(),
            transcript: String::new(),
            reply_type: reply_type.into(),
            word_bank: Vec::new(),
            children: (0..children)
                .map(|i| ChildQ {
                    question_type: "basic".into(),
                    reply_type: reply_type.into(),
                    question_text: format!("q{}", i),
                    options: Vec::new(),
                    option_count: 0,
                })
                .collect(),
        }
    }

    fn group(modules: Vec<Module>) -> ParsedGroup {
        ParsedGroup { modules }
    }

    #[test]
    fn vocabulary_group_is_not_answerable() {
        assert!(!has_answerable_module(&group(vec![module("vocabulary", 0)])));
        assert!(!has_answerable_module(&group(vec![module("", 0)])));
        assert!(!has_answerable_module(&group(vec![module("discussion", 1)])));
    }

    #[test]
    fn choice_group_is_answerable() {
        assert!(has_answerable_module(&group(vec![module("singlechoice", 1)])));
        // 混合组：讨论 + 选择题 → 仍需答案提交
        assert!(has_answerable_module(&group(vec![
            module("discussion", 1),
            module("fillblank", 1),
        ])));
    }
}
