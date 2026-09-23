use UnipusAI::api::session::Session;
use UnipusAI::config::Config;
use anyhow::{Context, Result};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .format_timestamp_secs()
        .init();

    let config_path = PathBuf::from("config.json");
    let cfg = Config::load(&config_path)?;
    let session = Session::new(cfg.clone(), config_path.clone())?;

    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("help");

    match cmd {
        "progress" => cmd_progress(&session, &args[2..]).await?,
        "group" => cmd_group(&session, &args[2..]).await?,
        "debug" => cmd_debug(&session, &args[2..]).await?,
        "run" => cmd_run(session, &args[2..]).await?,
        "test-types" => cmd_test_types(&session).await?,
        "transcribe" => {
            let url = args.get(2).map(|s| s.as_str()).unwrap_or_default();
            cmd_transcribe(&session, url).await?
        }
        "dump-text" => cmd_dump_text(&session, &args[2..]).await?,
        "help" | "-h" | "--help" => print_help(),
        other => {
            eprintln!("未知命令: {}", other);
            print_help();
        }
    }
    Ok(())
}

/// 每个题型抽一题测试答题链路（含媒体转写），不提交。
async fn cmd_test_types(session: &Session) -> Result<()> {
    use UnipusAI::api::content::{decrypt_content, fetch_content, parse_decrypted};
    use UnipusAI::api::course::{fetch_course_units, fetch_unit};
    use UnipusAI::api::parser::{Module, parse_group};
    use std::collections::BTreeMap;

    let units = fetch_course_units(session).await?;
    // key: module_type + child reply_type；取每个题型的第一题
    let mut samples: BTreeMap<String, (String, String, Module, usize)> = BTreeMap::new();

    for uid in &units {
        let rt = fetch_unit(session, uid).await?;
        for (gid, leaf) in &rt.leafs {
            if leaf.tab_type != "task" {
                continue;
            }
            let Ok(fc) = fetch_content(session, gid).await else {
                continue;
            };
            let Ok(plain) = decrypt_content(&fc.content, &fc.k) else {
                continue;
            };
            let Ok(dec) = parse_decrypted(&plain) else {
                continue;
            };
            let Ok(group) = parse_group(&dec) else {
                continue;
            };
            for m in &group.modules {
                if m.children.is_empty() {
                    continue;
                }
                for (ci, c) in m.children.iter().enumerate() {
                    let key = format!("{} / {}", m.module_type, c.reply_type);
                    samples
                        .entry(key)
                        .or_insert_with(|| (uid.clone(), gid.clone(), m.clone(), ci));
                }
            }
        }
    }

    println!("共发现 {} 种题型，逐个测试：\n", samples.len());
    let mut failed = 0usize;
    for (key, (unit, gid, m, ci)) in &samples {
        let _ = unit;
        let qtext = m
            .children
            .get(*ci)
            .map(|c| UnipusAI::api::parser::truncate_text(&c.question_text, 50))
            .unwrap_or_default();
        let media = if m.media_sources.is_empty() {
            "无".to_string()
        } else {
            format!("{}个", m.media_sources.len())
        };
        print!(
            "[{:<4}] {} 题干={} 媒体={} ...",
            format!("{}/{}", key, ci),
            gid,
            qtext,
            media
        );
        match UnipusAI::solve::solve_module(session, m).await {
            Ok(vals) => {
                let ans = vals.get(*ci).cloned().unwrap_or_default();
                println!("答={}", UnipusAI::api::parser::truncate_text(&ans, 40));
            }
            Err(e) => {
                failed += 1;
                println!("失败: {:#}", e);
            }
        }
    }
    println!("\n题型 {} 个，失败 {} 个", samples.len(), failed);
    if failed > 0 {
        anyhow::bail!("部分题型测试失败");
    }
    Ok(())
}

/// 打印全部题目文本与媒体转写结果（不答题、不提交）。
/// 输出结构: dump_text/{单元序号}_{unitId}/{题型}/{groupId}.txt，汇总写入 dump_text/_summary.txt。
/// task 叶子全量导出；text/video 叶子只导出含讨论题（discussion）或单词卡（vocabulary）的任务组。
/// 文件首行含必修/完成状态：已存在的文件每次只刷新状态（不重新抓题），缺失的才抓取生成；
/// `--force` 清空重生成（目录结构变更后建议先 --force）。
async fn cmd_dump_text(session: &Session, unit_ids: &[String]) -> Result<()> {
    use UnipusAI::api::content::{decrypt_content, fetch_content, parse_decrypted};
    use UnipusAI::api::course::{fetch_course_units, fetch_unit};
    use UnipusAI::api::parser::{parse_group, truncate_text};
    use UnipusAI::dump::{self, DUMP_DIR};
    use std::fs;

    // `--force` 清空并重建输出目录，否则保留已有文件（已生成的刷新状态、缺失的抓取生成）；
    // `--names` 额外打印课程名与单元名。
    let (force, rest) = split_flag(unit_ids, "--force");
    let (with_names, unit_ids) = split_flag(&rest, "--names");
    if force && std::path::Path::new(DUMP_DIR).exists() {
        fs::remove_dir_all(DUMP_DIR).ok();
    }
    fs::create_dir_all(DUMP_DIR)?;

    if with_names {
        println!(
            "课程: {}",
            UnipusAI::api::course::course_display_name(session, session.course_id()).await
        );
    }

    // 指定单元时仍拉取全量列表以确定课程序号（失败则退化为参数顺序）
    let all_units = if unit_ids.is_empty() {
        fetch_course_units(session).await?
    } else {
        fetch_course_units(session).await.unwrap_or_default()
    };
    let units = if unit_ids.is_empty() {
        all_units.clone()
    } else {
        unit_ids.to_vec()
    };

    let mut n_group = 0usize;
    let mut n_module = 0usize;
    let mut n_question = 0usize;
    let mut n_media = 0usize;
    let mut n_media_chars = 0usize;
    let mut n_skipped = 0usize;
    let mut n_updated = 0usize;

    for (ui, uid) in units.iter().enumerate() {
        // 课程序号（目录名前缀），指定单元时也能与全量跑法保持一致
        let unit_no = all_units
            .iter()
            .position(|u| u == uid)
            .map(|i| i + 1)
            .unwrap_or(ui + 1);
        let unit_dir =
            std::path::PathBuf::from(format!("{}/{:02}_{}", DUMP_DIR, unit_no, uid));
        // 已有文件映射（gid -> 路径）：只刷新状态，避免重复抓题
        let existing = if force {
            std::collections::HashMap::new()
        } else {
            dump::scan_unit_files(&unit_dir)
        };
        let rt = fetch_unit(session, uid).await?;
        let mut unit_label: String = String::new();
        let mut unit_header_printed = false;
        for (gid, leaf) in &rt.leafs {
            let header = dump::dump_header(
                uid,
                gid,
                &leaf.tab_type,
                leaf.strategies.required,
                leaf.state.pass >= 1,
            );

            // 已存在：只刷新首行状态（状态未变不写盘），不重新抓题
            if let Some(path) = existing.get(gid) {
                n_skipped += 1;
                if let Ok(content) = fs::read_to_string(path)
                    && let Some(new) = dump::replace_first_line(&content, &header)
                    && fs::write(path, new).is_ok()
                {
                    n_updated += 1;
                }
                continue;
            }

            let is_task = leaf.tab_type == "task";
            let Ok(fc) = fetch_content(session, gid).await else {
                continue;
            };
            let Ok(plain) = decrypt_content(&fc.content, &fc.k) else {
                continue;
            };
            let Ok(dec) = parse_decrypted(&plain) else {
                continue;
            };
            let Ok(group) = parse_group(&dec) else {
                continue;
            };
            // 非 task 叶子（text/video）只导出含讨论题或单词卡的任务组
            let has_discussion = group.modules.iter().any(|m| m.reply_type == "discussion");
            let has_vocabulary = group.modules.iter().any(|m| m.module_type == "vocabulary");
            if !is_task && !has_discussion && !has_vocabulary {
                continue;
            }

            if with_names && !unit_header_printed {
                if unit_label.is_empty() {
                    unit_label = UnipusAI::api::parser::extract_group_label(&dec);
                }
                let label = if unit_label.is_empty() {
                    format!("Unit {}", ui + 1)
                } else {
                    unit_label.clone()
                };
                println!("单元 {} ({})", uid, label);
                unit_header_printed = true;
            }

            n_group += 1;
            let mut lines: Vec<String> = Vec::new();
            lines.push(header);

            let vocab = UnipusAI::api::parser::extract_vocabulary(&dec);
            for m in &group.modules {
                n_module += 1;
                n_question += m.children.len();
                lines.push(String::new());
                lines.push(format!(
                    "[模块] type={} reply_type={} instance_id={}",
                    m.module_type, m.reply_type, m.instance_id
                ));
                if m.module_type == "vocabulary" {
                    lines.push(format!(
                        "【单词卡】共 {} 个单词（朗读练习，提交时标记已看）",
                        vocab.len()
                    ));
                    for w in &vocab {
                        lines.push(format!(
                            "  {} | {}",
                            w.name,
                            if w.sound.is_empty() { "-" } else { w.sound.as_str() }
                        ));
                    }
                }
                if !m.direction.is_empty() {
                    lines.push(format!("【答题说明】\n{}", m.direction));
                }
                if !m.material.is_empty() {
                    lines.push(format!(
                        "【材料文本】({}字)\n{}",
                        m.material.chars().count(),
                        m.material
                    ));
                }
                if !m.transcript.is_empty() {
                    lines.push(format!(
                        "【内嵌字幕】({}字)\n{}",
                        m.transcript.chars().count(),
                        m.transcript
                    ));
                }
                for url in &m.media_sources {
                    n_media += 1;
                    match UnipusAI::transcribe::transcribe_media(session, url).await {
                        Ok(t) => {
                            n_media_chars += t.chars().count();
                            lines.push(format!(
                                "【媒体转写】(来源 {})\n{}",
                                url,
                                truncate_text(&t, 5000)
                            ));
                        }
                        Err(e) => {
                            lines.push(format!("【媒体转写失败】(来源 {})\n{:#}", url, e));
                        }
                    }
                }
                for (ci, c) in m.children.iter().enumerate() {
                    let mut buf = format!("  [{:>2}] {} ", ci + 1, c.reply_type);
                    if !c.question_text.is_empty() {
                        buf.push_str(&format!("| 题干: {}", truncate_text(&c.question_text, 300)));
                    }
                    if !c.options.is_empty() {
                        let opts: Vec<String> = c
                            .options
                            .iter()
                            .map(|o| {
                                let label = if o.name.is_empty() {
                                    o.value.clone()
                                } else {
                                    o.name.clone()
                                };
                                let txt = if o.text.is_empty() {
                                    o.value.clone()
                                } else {
                                    o.text.clone()
                                };
                                format!("{}: {}", label, truncate_text(&txt, 100))
                            })
                            .collect();
                        buf.push_str(&format!(" | 选项: {}", opts.join(" ; ")));
                    }
                    lines.push(buf);
                }
            }

            let type_dir = unit_dir.join(dump::group_type_name(&group));
            let path = type_dir.join(format!("{}.txt", gid));
            fs::create_dir_all(&type_dir)?;
            fs::write(&path, lines.join("\n"))?;
            println!(
                "任务组 {} -> {} (模块{} 题{} 媒体{})",
                gid,
                path.display(),
                group.modules.len(),
                UnipusAI::api::parser::question_count(&group),
                m_media_count(&group)
            );
        }
    }

    // 生成状态报告（扫描全部文件首行，含必修/完成情况）
    let total = dump::write_summary()?;
    println!(
        "dump-text 完成: 单元 {} 个, 新生成 {} 个, 已存在 {} 个(其中刷新状态 {} 个), 目录累计文件 {} 个",
        units.len(),
        n_group,
        n_skipped,
        n_updated,
        total
    );
    println!(
        "本次新生成: 模块 {} 个, 题目 {} 道, 媒体转写 {} 条, 共 {} 字符",
        n_module, n_question, n_media, n_media_chars
    );
    println!("状态汇总已写入 {}/_summary.txt", DUMP_DIR);
    Ok(())
}

fn m_media_count(group: &UnipusAI::api::parser::ParsedGroup) -> usize {
    group.modules.iter().map(|m| m.media_sources.len()).sum()
}

async fn cmd_debug(session: &Session, args: &[String]) -> Result<()> {
    use UnipusAI::api::content::{decrypt_content, fetch_content, parse_decrypted};
    use UnipusAI::api::parser::parse_group;
    let (force, rest) = split_flag(args, "--force");
    let group_id = rest.first().map(|s| s.as_str()).unwrap_or_default();
    if group_id.is_empty() {
        anyhow::bail!("用法: UnipusAI debug <groupId> [--force]");
    }
    // 已通过任务默认只做只读解析预览、不调用 LLM；--force 可强制生成
    let passed = if force {
        false
    } else {
        UnipusAI::core::runner::mock_task(session, group_id)
            .await
            .map(|t| t.passed)
            .unwrap_or(false)
    };
    if passed {
        println!("[已完成] 跳过 LLM 作答预览（--force 可强制生成）");
    }
    let rt = fetch_content(session, group_id).await?;
    let plain = decrypt_content(&rt.content, &rt.k)?;
    let dec = parse_decrypted(&plain)?;
    println!("=== 解密后完整 JSON ===");
    println!("{}", serde_json::to_string_pretty(&dec)?);
    let group = parse_group(&dec)?;
    let has_discussion = group.modules.iter().any(|m| m.reply_type == "discussion");
    let vocab = UnipusAI::api::parser::extract_vocabulary(&dec);
    for m in &group.modules {
        println!(
            "[module {}] reply_type={} children={} material_len={} word_bank={}",
            m.instance_id,
            m.reply_type,
            m.children.len(),
            m.material.chars().count(),
            m.word_bank.len()
        );
        let values = if passed {
            Vec::new()
        } else {
            UnipusAI::solve::solve_module(session, m).await?
        };
        for (ci, c) in m.children.iter().enumerate() {
            let v = values.get(ci).cloned().unwrap_or_default();
            println!(
                "   [{:>2}] {} | qt={} | q={} | ans={} | opts={}",
                ci + 1,
                c.reply_type,
                c.question_type,
                UnipusAI::api::parser::truncate_text(&c.question_text, 60),
                UnipusAI::api::parser::truncate_text(&v, 60),
                c.option_count
            );
        }
        // 讨论题：完整展示发言草稿（run/group 时发布到讨论区）
        if m.reply_type == "discussion"
            && let Some(draft) = values.first()
            && !draft.trim().is_empty()
        {
            println!(
                "   ── 讨论发言草稿（完整，run/group 时发布）──\n{}",
                draft.trim()
            );
        }
        // 单词卡朗读练习：展示词表摘要（无需作答，run/group 时标记已看）
        if m.module_type == "vocabulary" {
            let names: Vec<&str> = vocab.iter().take(10).map(|w| w.name.as_str()).collect();
            println!(
                "   ── 单词卡朗读练习：共 {} 个单词（提交时标记已看，无需作答）──",
                vocab.len()
            );
            if !names.is_empty() {
                println!("   {}", names.join(" / "));
            }
            if vocab.len() > 10 {
                println!("   …（其余 {} 个）", vocab.len() - 10);
            }
        }
    }
    if has_discussion {
        print_discussion_status(session, group_id).await;
    }
    Ok(())
}

/// debug 用：只读打印讨论区状态（查询失败仅提示，不影响调试）。
async fn print_discussion_status(session: &Session, group_id: &str) {
    use UnipusAI::api::bbs;
    match bbs::fetch_topics(session, group_id).await {
        Err(e) => println!("\n[讨论区] 查询失败（不影响调试）: {:#}", e),
        Ok(topics) if topics.is_empty() => {
            println!("\n[讨论区] 当前无主题，run/group 时将自动创建主题后发帖");
        }
        Ok(topics) => {
            println!("\n[讨论区]");
            for t in topics {
                let replied = bbs::has_own_reply(session, t.topic_id)
                    .await
                    .unwrap_or(false);
                println!(
                    "  topicId={} owner={} replyCount={} 本人已回复={}",
                    t.topic_id, t.owner_status, t.reply_count, replied
                );
            }
        }
    }
}

async fn cmd_group(session: &Session, args: &[String]) -> Result<()> {
    let (force, rest) = split_flag(args, "--force");
    let group_id = rest.first().map(|s| s.as_str()).unwrap_or_default();
    if group_id.is_empty() {
        anyhow::bail!("用法: UnipusAI group <groupId> [--force]");
    }
    let task = UnipusAI::core::runner::mock_task(session, group_id).await?;
    // 已通过任务默认跳过：不调用 LLM、不提交；--force 可强制重做
    if task.passed && !force {
        println!(
            "[SKIP] {} 已通过，跳过作答与提交（--force 可强制重做）",
            group_id
        );
        UnipusAI::dump::sync_task_status(&task, true);
        if let Err(e) = UnipusAI::dump::write_summary() {
            log::warn!("刷新 dump 状态汇总失败: {:#}", e);
        }
        return Ok(());
    }
    match UnipusAI::core::runner::process_group(session, &task).await {
        Ok(resp) => {
            println!("[OK] {} -> {}", task.tab_type, resp);
            if let Err(e) = UnipusAI::dump::write_summary() {
                log::warn!("刷新 dump 状态汇总失败: {:#}", e);
            }
        }
        Err(e) => println!("[FAIL] {} -> {:#}", task.tab_type, e),
    }
    Ok(())
}

async fn cmd_transcribe(session: &Session, url: &str) -> Result<()> {
    if url.is_empty() {
        anyhow::bail!("用法: UnipusAI transcribe <mediaUrl>  (测试媒体转写链路)");
    }
    let media = UnipusAI::api::parser::clean_url(url);
    let start = std::time::Instant::now();
    let text = UnipusAI::transcribe::transcribe_media(session, &media).await?;
    println!(
        "[{}] {}ms 转写结果 {} 字:",
        url,
        start.elapsed().as_millis(),
        text.chars().count()
    );
    if text.is_empty() {
        anyhow::bail!("转写为空，请确认 whisper_enabled=true 且 ffmpeg 可用、模型已下载");
    }
    println!("{}", UnipusAI::api::parser::truncate_text(&text, 300));
    Ok(())
}

/// 从参数中滤出开关标志，返回 (是否出现, 其余参数)。
fn split_flag(args: &[String], flag: &str) -> (bool, Vec<String>) {
    let mut present = false;
    let mut rest = Vec::new();
    for a in args {
        if a == flag {
            present = true;
        } else {
            rest.push(a.clone());
        }
    }
    (present, rest)
}

/// 提取带值的标志，支持 "--flag value" 与 "--flag=value"。
/// 返回 (取值, 其余参数)；出现但缺值时 value 为 None。
fn extract_flag_value(args: &[String], flag: &str) -> (Option<String>, Vec<String>) {
    let mut value: Option<String> = None;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(rest_part) = a.strip_prefix(&format!("{}=", flag)) {
            value = Some(rest_part.to_string());
        } else if a == flag {
            if let Some(next) = args.get(i + 1) {
                if next.starts_with('-') {
                    value = None;
                } else {
                    value = Some(next.clone());
                    i += 1;
                }
            } else {
                value = None;
            }
        } else {
            rest.push(a.clone());
        }
        i += 1;
    }
    (value, rest)
}

async fn cmd_progress(session: &Session, args: &[String]) -> Result<()> {
    use UnipusAI::api::course::course_display_name;
    use UnipusAI::core::planner::plan_course;
    let with_names = args.iter().any(|a| a == "--names");
    let plan = plan_course(session).await?;
    if with_names {
        println!(
            "课程: {}",
            course_display_name(session, session.course_id()).await
        );
    }
    println!("学习策略: {}", session.cfg().learning_strategy);
    for (i, unit) in plan.units.iter().enumerate() {
        if with_names {
            let label = UnipusAI::api::course::unit_label(session, &unit.unit_id)
                .await?
                .unwrap_or_else(|| format!("Unit {}", i + 1));
            println!(
                "单元 {} ({}) ：任务 {} 个",
                unit.unit_id,
                label,
                unit.tasks.len()
            );
        } else {
            println!("单元 {} ：任务 {} 个", unit.unit_id, unit.tasks.len());
        }
        for t in &unit.tasks {
            println!(
                "{:6} {:15} required={:<5} pass={} {}",
                t.tab_type,
                t.group_id,
                t.required,
                t.passed,
                if t.passed { "✔" } else { "" }
            );
        }
    }
    println!("总计 {} 个任务，待完成 {} 个", plan.total, plan.todo);
    Ok(())
}

async fn cmd_run(mut session: Session, args: &[String]) -> Result<()> {
    let (with_names, rest) = split_flag(args, "--names");
    let (interval, unit_ids) = extract_flag_value(&rest, "--interval");
    if let Some(v) = interval {
        let ms: u64 = v
            .trim()
            .parse()
            .with_context(|| format!("--interval 需要毫秒数（正整数），收到: {}", v))?;
        session.set_interval_ms(ms);
        println!("提交间隔设为 {}ms", ms);
    } else {
        println!("提交间隔使用默认 {}ms", session.cfg().interval_ms);
    }
    let summary = if unit_ids.is_empty() {
        UnipusAI::core::runner::run_course(&mut session, with_names).await?
    } else {
        UnipusAI::core::runner::run_course_units(&mut session, &unit_ids, with_names).await?
    };
    println!(
        "完成: done={} skipped={} failed={}",
        summary.done, summary.skipped, summary.failed
    );
    Ok(())
}

fn print_help() {
    println!(
        r#"UnipusAI - U校园 AI 版刷课脚本

用法:
  UnipusAI <命令> [参数]

命令:
  progress [--names]
      打印课程全部单元/任务树（按 learning_strategy 过滤）

  run [--names] [--interval <毫秒>] [unitId...]
      自动完成课程（默认全部单元，也可指定单元）

  group <groupId> [--force]
      直接提交指定任务组（LLM 答题；讨论题自动发帖）
      已通过任务默认跳过（不调用 LLM、不提交），--force 强制重做

  debug <groupId> [--force]
      本地求解指定任务组（不提交，用于调试；讨论题显示草稿与讨论区状态，单词卡显示词表）
      已通过任务默认只做解析预览（不调用 LLM），--force 强制生成

  test-types
      每种题型抽一题测试答题链路（不提交）

  transcribe <url>
      测试媒体转写链路（下载 -> ffmpeg -> whisper）

  dump-text [--names] [--force] [unitId...]
      导出题目文本与媒体转写到 dump_text/{{单元序号}}_{{unitId}}/{{题型}}/{{groupId}}.txt（不答题）
      task 叶子全量导出；text/video 叶子仅导出含讨论题（discussion）或单词卡（vocabulary）的任务组
      文件首行含“必修/完成”状态，每次运行刷新；_summary.txt 为状态汇总
      run/group 答题完成后同样会自动同步状态；--force 清空并重新生成

参数:
  --names       显示课程名与单元名（如 新视野大学英语(第四版)读写教程 / U1 Pre-reading activities），
                结果缓存到 .unit_labels.json
  --interval    两次提交间隔，默认 3000ms，如 --interval 5000 或 --interval=5000
  --force       dump-text 清空并全量重新生成；group/debug 忽略“已通过”跳过、强制重做/生成
  <unitId...>   只处理指定单元（可多个），省略则处理全部单元

示例:
  UnipusAI progress --names
  UnipusAI run --interval 5000
  UnipusAI run 6bbb2df99001b1e
  UnipusAI group 6bbb307f30d1b1e
  UnipusAI debug 6bbb307f30d1b1e
  UnipusAI dump-text --force

说明:
  配置见 config.json（含 cookie、course_id 等，讨论题另需 class_id/curricula_id），unit_id 已无需填写。
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_interval_space_form() {
        let args = vec!["--names".to_string(), "--interval".to_string(), "5000".to_string()];
        let (v, rest) = extract_flag_value(&args, "--interval");
        assert_eq!(v.as_deref(), Some("5000"));
        assert_eq!(rest, vec!["--names"]);
    }

    #[test]
    fn extract_interval_equals_form() {
        let args = vec!["--interval=8000".to_string(), "unit1".to_string()];
        let (v, rest) = extract_flag_value(&args, "--interval");
        assert_eq!(v.as_deref(), Some("8000"));
        assert_eq!(rest, vec!["unit1"]);
    }

    #[test]
    fn extract_interval_missing_value() {
        let args = vec!["--interval".to_string(), "--names".to_string()];
        let (v, rest) = extract_flag_value(&args, "--interval");
        assert_eq!(v, None);
        assert_eq!(rest, vec!["--names"]);
    }

    #[test]
    fn extract_interval_absent() {
        let args = vec!["unit1".to_string(), "unit2".to_string()];
        let (v, rest) = extract_flag_value(&args, "--interval");
        assert_eq!(v, None);
        assert_eq!(rest, args);
    }
}
