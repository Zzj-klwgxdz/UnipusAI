use crate::api::content::{decrypt_content, fetch_content, parse_decrypted};
use crate::api::course::{fetch_course_units, fetch_unit};
use crate::api::parser::{
    ParsedGroup, extract_group_label, extract_vocabulary, parse_group, question_count,
};
use crate::api::session::Session;
use crate::db::{self, MediaEntry, TaskInput};
use crate::dump;
use crate::reporter::{DumpKind, ReportEvent, Reporter};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

/// dump-text 运行参数。
#[derive(Debug, Default, Clone)]
pub struct DumpOptions {
    /// 清空数据库并全量重建
    pub force: bool,
    /// 额外输出课程名与单元名
    pub with_names: bool,
    /// 指定单元（为空则全课程）
    pub unit_ids: Vec<String>,
    /// 只处理指定任务组（用于 TUI 单任务重抓；匹配时强制覆盖内容）
    pub group_ids: Vec<String>,
}

/// dump-text 本次运行统计。
#[derive(Debug, Default)]
pub struct DumpSummary {
    pub units: usize,
    pub generated: usize,
    pub skipped: usize,
    pub updated: usize,
    pub total_files: usize,
    pub modules: usize,
    pub questions: usize,
    pub media: usize,
    pub media_chars: usize,
}

fn media_count(group: &ParsedGroup) -> usize {
    group.modules.iter().map(|m| m.media_sources.len()).sum()
}

/// 导出题目文本与媒体转写（不答题、不提交）到 SQLite（dump_text/dump.db）。
/// 所有叶子全量归档；浏览类页面（内容为空/非 JSON/无模块）按 view-only 保存。
pub async fn run_dump_text(
    session: &Session,
    opts: &DumpOptions,
    reporter: &dyn Reporter,
    cancel: &CancellationToken,
) -> Result<DumpSummary> {
    let mut conn = db::open()?;
    if opts.force {
        db::clear_all(&conn)?;
    }
    let course_id = session.course_id().to_string();

    // 记录课程信息（meta），供 TUI 离线显示课程名；单任务重抓时跳过
    if opts.group_ids.is_empty() {
        let name = crate::api::course::course_display_name(session, session.course_id()).await;
        let _ = db::save_meta(&conn, "course_id", &course_id);
        let _ = db::save_meta(&conn, "course_name", &name);
        let _ = db::save_meta(&conn, &format!("course_name:{}", course_id), &name);
    }

    if opts.with_names {
        reporter.report(ReportEvent::Plain(format!(
            "课程: {}",
            crate::api::course::course_display_name(session, session.course_id()).await
        )));
    }

    // 指定单元时仍拉取全量列表以确定课程序号（失败则退化为参数顺序）
    let all_units = if opts.unit_ids.is_empty() {
        fetch_course_units(session).await?
    } else {
        fetch_course_units(session).await.unwrap_or_default()
    };
    let units = if opts.unit_ids.is_empty() {
        all_units.clone()
    } else {
        opts.unit_ids.clone()
    };

    let mut summary = DumpSummary {
        units: units.len(),
        ..Default::default()
    };
    let db_path = dump::db_path().display().to_string();

    'units: for (ui, uid) in units.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        // 课程序号，指定单元时也能与全量跑法保持一致
        let unit_no = all_units
            .iter()
            .position(|u| u == uid)
            .map(|i| i + 1)
            .unwrap_or(ui + 1);
        let rt = fetch_unit(session, uid).await?;
        let mut unit_label: String = String::new();
        let mut unit_header_printed = false;
        for (gid, leaf) in &rt.leafs {
            if cancel.is_cancelled() {
                break 'units;
            }
            // 单任务重抓：只处理匹配的 gid，且强制覆盖内容
            let only_mode = !opts.group_ids.is_empty();
            if only_mode && !opts.group_ids.contains(gid) {
                continue;
            }
            let required = leaf.strategies.required;
            let passed = leaf.state.pass >= 1;

            // 已入库：仅刷新状态（状态未变不写库），不重新抓题；
            // 旧库缺少 raw_json / course_id 不匹配时重新抓取补全
            if !only_mode
                && db::task_exists(&conn, uid, gid)?
                && !db::task_needs_refresh(&conn, uid, gid, &course_id)?
            {
                summary.skipped += 1;
                let changed = db::task_status(&conn, uid, gid)?
                    .map(|(old_req, old_passed)| old_req != required || old_passed != passed)
                    .unwrap_or(true);
                if changed
                    && db::update_status(
                        &conn,
                        uid,
                        gid,
                        unit_no,
                        if unit_label.is_empty() {
                            None
                        } else {
                            Some(unit_label.as_str())
                        },
                        &leaf.tab_type,
                        required,
                        passed,
                    )?
                {
                    summary.updated += 1;
                    reporter.report(ReportEvent::Dump {
                        unit_id: uid.clone(),
                        group_id: gid.clone(),
                        kind: DumpKind::Refreshed,
                        path: db_path.clone(),
                        modules: 0,
                        questions: 0,
                        media: 0,
                    });
                }
                continue;
            }

            let Ok(fc) = fetch_content(session, gid).await else {
                continue;
            };
            let Ok(plain) = decrypt_content(&fc.content, &fc.k) else {
                continue;
            };
            // 可解析出题目模块 → 正常归档；否则（空/非 JSON/无模块）为浏览类页面
            let parsed = parse_decrypted(&plain)
                .ok()
                .and_then(|dec| parse_group(&dec).ok().map(|group| (dec, group)));

            if unit_label.is_empty()
                && let Some((dec, _)) = &parsed
            {
                unit_label = extract_group_label(dec);
            }
            if opts.with_names && !unit_header_printed {
                let label = if unit_label.is_empty() {
                    format!("Unit {}", ui + 1)
                } else {
                    unit_label.clone()
                };
                reporter.report(ReportEvent::Plain(format!("单元 {} ({})", uid, label)));
                unit_header_printed = true;
            }

            summary.generated += 1;
            let label = if unit_label.is_empty() {
                None
            } else {
                Some(unit_label.as_str())
            };

            let Some((dec, group)) = parsed else {
                // 浏览类页面（自定义/空内容）：原始内容全文入库
                let trimmed = plain.trim();
                let raw = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                };
                db::save_task(
                    &mut conn,
                    &TaskInput {
                        unit_index: unit_no,
                        unit_id: uid,
                        unit_label: label,
                        group_id: gid,
                        tab_type: &leaf.tab_type,
                        group_type: dump::VIEW_ONLY_DIR,
                        kind: dump::VIEW_ONLY_DIR,
                        required,
                        passed,
                        raw_content: raw,
                        raw_json: None,
                        course_id: &course_id,
                        group: None,
                        vocab: &[],
                        media: &[],
                    },
                )?;
                reporter.report(ReportEvent::Dump {
                    unit_id: uid.clone(),
                    group_id: gid.clone(),
                    kind: DumpKind::ViewOnly,
                    path: db_path.clone(),
                    modules: 0,
                    questions: 0,
                    media: 0,
                });
                continue;
            };

            let vocab = extract_vocabulary(&dec);
            let raw_json = serde_json::to_string_pretty(&dec).ok();
            let mut media: Vec<MediaEntry> = Vec::new();
            for (mi, m) in group.modules.iter().enumerate() {
                summary.modules += 1;
                summary.questions += m.children.len();
                for (url_idx, url) in m.media_sources.iter().enumerate() {
                    summary.media += 1;
                    match crate::transcribe::transcribe_media(session, url).await {
                        Ok(t) => {
                            summary.media_chars += t.chars().count();
                            media.push(MediaEntry {
                                module_idx: mi,
                                url_idx,
                                url: url.clone(),
                                text: Some(t),
                                error: None,
                            });
                        }
                        Err(e) => {
                            media.push(MediaEntry {
                                module_idx: mi,
                                url_idx,
                                url: url.clone(),
                                text: None,
                                error: Some(format!("{:#}", e)),
                            });
                        }
                    }
                }
            }

            db::save_task(
                &mut conn,
                &TaskInput {
                    unit_index: unit_no,
                    unit_id: uid,
                    unit_label: label,
                    group_id: gid,
                    tab_type: &leaf.tab_type,
                    group_type: &dump::group_type_name(&group),
                    kind: "task",
                    required,
                    passed,
                    raw_content: None,
                    raw_json: raw_json.as_deref(),
                    course_id: &course_id,
                    group: Some(&group),
                    vocab: &vocab,
                    media: &media,
                },
            )?;
            reporter.report(ReportEvent::Dump {
                unit_id: uid.clone(),
                group_id: gid.clone(),
                kind: DumpKind::Generated,
                path: db_path.clone(),
                modules: group.modules.len(),
                questions: question_count(&group),
                media: media_count(&group),
            });
        }
    }

    summary.total_files = db::task_count(&conn, &course_id)?;
    Ok(summary)
}
