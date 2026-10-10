use UnipusAI::api::session::Session;
use UnipusAI::config::Config;
use anyhow::{Context, Result};
use colored::Colorize;
use crossterm::event::{self, Event, KeyEventKind};
// use crossterm::style::Stylize;
use crossterm::terminal;
use figlet_rs::FIGlet;

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;


/// env_logger 输出分流：同时写 stderr 与本次运行日志文件。
struct TeeWriter {
    log: Arc<UnipusAI::logging::RunLog>,
}

impl std::io::Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        self.log.write_bytes(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        self.log.flush();
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    print_hello()?;
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let config_path = PathBuf::from("config.json");

    // 初始化运行日志（logs/ 每次运行新建文件，自动清理 3 天前）
    let run_log: Option<Arc<UnipusAI::logging::RunLog>> = match UnipusAI::logging::prepare() {
        Ok(l) => Some(Arc::new(l)),
        Err(e) => {
            eprintln!("警告: 初始化日志文件失败，仅输出到控制台: {:#}", e);
            None
        }
    };

    // 无参数 / --tui：交互式界面
    if args.len() <= 1 || cmd == "--tui" || cmd == "tui" {
        wait_any_key()?;
        let cfg = Config::load(&config_path)?;
        let mut session = Session::new(cfg, config_path)?;
        // 离线可用：登录失败仅提示，仍进入 TUI（任务树来自本地数据库）
        if let Err(e) = UnipusAI::api::login::ensure_login(&mut session).await {
            eprintln!("提示: 自动登录未完成（{:#}），将以本地数据模式运行", e);
        }
        return UnipusAI::tui::run_tui(session, run_log).await;
    }

    match &run_log {
        Some(log) => {
            let tee = TeeWriter { log: log.clone() };
            env_logger::builder()
                .filter_level(log::LevelFilter::Info)
                .target(env_logger::Target::Pipe(Box::new(tee)))
                .format(|buf, record| {
                    let (t, _) = UnipusAI::logging::now();
                    writeln!(
                        buf,
                        "[{} {} {}] {}",
                        UnipusAI::logging::stamp_human(t),
                        record.level(),
                        record.target(),
                        record.args()
                    )
                })
                .init();
            log::info!("日志文件: {}", log.path.display());
        }
        None => {
            env_logger::builder()
                .filter_level(log::LevelFilter::Info)
                .format_timestamp_secs()
                .init();
        }
    }

    let cfg = Config::load(&config_path)?;
    let mut session = Session::new(cfg.clone(), config_path.clone())?;

    // 帮助最优先，不触发登录
    if matches!(cmd, "help" | "-h" | "--help") {
        print_help();
        return Ok(());
    }
    // 登录/刷新与课程选择命令
    if cmd == "login" {
        cmd_login(&mut session, &args[2..]).await?;
        return Ok(());
    }
    if cmd == "courses" {
        UnipusAI::api::login::ensure_login(&mut session).await?;
        UnipusAI::api::login::ensure_profile(&mut session).await?;
        cmd_courses(&session).await?;
        return Ok(());
    }
    if cmd == "course" {
        UnipusAI::api::login::ensure_login(&mut session).await?;
        UnipusAI::api::login::ensure_profile(&mut session).await?;
        cmd_course(&mut session, &args[2..]).await?;
        return Ok(());
    }
    if cmd == "annotator" {
        cmd_annotator(&mut session, &args[2..]).await?;
        return Ok(());
    }

    // 其余命令：先确保登录凭证有效，并刷新课程相关字段
    UnipusAI::api::login::ensure_login(&mut session).await?;
    UnipusAI::api::login::ensure_profile(&mut session).await?;
    // 与课程内容相关的命令要求已选课程
    if matches!(
        cmd,
        "progress" | "run" | "group" | "debug" | "test-types" | "dump-text"
    ) {
        UnipusAI::api::login::ensure_course(&session)?;
    }

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
        other => {
            eprintln!("未知命令: {}", other);
            print_help();
        }
    }
    Ok(())
}

/// `login [--force]`：查看登录态；--force 用账号密码强制重新登录。
async fn cmd_login(session: &mut Session, args: &[String]) -> Result<()> {
    let force = args.iter().any(|a| a == "--force");
    let cfg = session.cfg().clone();
    if force {
        if cfg.username.is_empty() || cfg.password.is_empty() {
            anyhow::bail!("config.json 缺 username/password，无法强制登录");
        }
        let c = UnipusAI::api::login::login(&cfg.username, &cfg.password).await?;
        let mut nc = cfg.clone();
        UnipusAI::api::login::apply(&mut nc, &c);
        session.update_config(nc)?;
        println!("已重新登录（账号 {}）", cfg.username);
    } else {
        let changed = UnipusAI::api::login::ensure_login(session).await?;
        if !changed {
            println!("登录凭证仍有效，无需刷新");
        }
    }
    // 每次启动刷新 class_id/curricula_id/u_school/publish_version
    UnipusAI::api::login::ensure_profile(session).await?;
    let cfg = session.cfg();
    let jwt = cfg.cookie_jwt().unwrap_or_default();
    let exp = UnipusAI::api::login::jwt_exp(&jwt).unwrap_or(0);
    println!(
        "jwt 过期时间: {} | refresh_token 过期: {}",
        fmt_unix(exp),
        if cfg.rt_expire > 0 { fmt_unix(cfg.rt_expire) } else { "未知".into() }
    );
    println!("open_id: {}", session.open_id());
    Ok(())
}

fn fmt_unix(secs: i64) -> String {
    if secs <= 0 {
        return "未知".into();
    }
    time::OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .and_then(|t| {
            t.format(time::macros::format_description!(
                "[year]-[month]-[day] [hour]:[minute]"
            ))
            .ok()
        })
        .unwrap_or_else(|| secs.to_string())
}

/// `annotator [--extract]`：查看/重新提取 x-annotator-auth-token 签发参数。
async fn cmd_annotator(session: &mut Session, args: &[String]) -> Result<()> {
    use UnipusAI::api::login::{self, AnnotatorParams};
    let force = args.iter().any(|a| a == "--extract");
    if force {
        let mut cfg = session.cfg().clone();
        if cfg.open_id.is_empty() {
            anyhow::bail!("config.json open_id 为空：请先填写 username/password 自动登录");
        }
        match login::extract_annotator_params().await {
            Ok(p) => {
                let cur = AnnotatorParams::from_config(&cfg);
                let same = p == cur;
                println!(
                    "从 bundle 提取成功: iss={} aud={} ttl={}ms key={}…{}",
                    p.iss,
                    p.aud,
                    p.ttl_ms,
                    &p.key[..p.key.len().min(8)],
                    if same { "（与当前参数一致）" } else { "（已更新）" }
                );
                cfg.annotator_key = p.key;
                cfg.annotator_iss = p.iss;
                cfg.annotator_aud = p.aud;
                cfg.annotator_ttl_ms = p.ttl_ms;
            }
            Err(e) => println!("从 bundle 提取失败: {:#}（保留现有参数并重签）", e),
        }
        let params = AnnotatorParams::from_config(&cfg);
        cfg.x_annotator_auth_token = login::generate_annotator_token(&cfg.open_id, &params);
        session.update_config(cfg)?;
        println!("已重新签发 x_annotator_auth_token");
    }
    let cfg = session.cfg().clone();
    let params = AnnotatorParams::from_config(&cfg);
    let source = if params.is_builtin() {
        "内置默认"
    } else {
        "bundle 提取/手动"
    };
    println!(
        "签发参数: 来源={} iss={} aud={} ttl={}天 key={}…",
        source,
        params.iss,
        params.aud,
        params.ttl_ms / 86_400_000,
        &params.key[..params.key.len().min(8)]
    );
    match login::jwt_payload(&cfg.x_annotator_auth_token)
        .and_then(|p| p.get("exp").and_then(|x| x.as_u64()))
    {
        Some(exp_ms) => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let days = exp_ms.saturating_sub(now_ms) / 86_400_000;
            println!(
                "当前 token: 剩余 {} 天{}",
                days,
                if login::annotator_token_ok(&cfg.x_annotator_auth_token) {
                    ""
                } else {
                    "（已失效/需重签）"
                }
            );
        }
        None => println!("当前 token: 缺失或无法解析"),
    }
    Ok(())
}

/// `courses`：列出账号下全部课程（* 标记当前选择）。
async fn cmd_courses(session: &Session) -> Result<()> {
    let list = UnipusAI::api::course::fetch_home_courses(session).await?;
    if list.is_empty() {
        println!("未查询到课程（账号下暂无课程；请确认已加入班级/激活教材）");
        return Ok(());
    }
    let current = session.course_id();
    println!("共 {} 门课程：", list.len());
    for (i, c) in list.iter().enumerate() {
        let mark = if c.course_id == current { "*" } else { " " };
        let tag = if c.group_label.is_empty() {
            String::new()
        } else {
            format!("[{}]", c.group_label)
        };
        println!("{} [{}]{} {}  {}", mark, i + 1, tag, c.name, c.course_id);
    }
    if current.is_empty() {
        println!("\n当前未选择课程：运行 `UnipusAI course <序号>` 选择");
    }
    Ok(())
}

/// `course [序号|course_id]`：查看或选择当前课程（写入 config.json）。
async fn cmd_course(session: &mut Session, args: &[String]) -> Result<()> {
    let list = UnipusAI::api::course::fetch_home_courses(session).await?;
    if args.is_empty() {
        let cur = session.course_id();
        if cur.is_empty() {
            println!("当前未选择课程；可用课程：");
        } else {
            let name = list
                .iter()
                .find(|c| c.course_id == cur)
                .map(|c| c.name.as_str())
                .unwrap_or("(不在课程列表中)");
            println!("当前课程: {}  {}", name, cur);
            println!("可用课程：");
        }
        for (i, c) in list.iter().enumerate() {
            let mark = if c.course_id == cur { "*" } else { " " };
            let tag = if c.group_label.is_empty() {
                String::new()
            } else {
                format!("[{}]", c.group_label)
            };
            println!("{} [{}]{} {}  {}", mark, i + 1, tag, c.name, c.course_id);
        }
        return Ok(());
    }
    let sel = args[0].trim();
    let chosen = if let Ok(n) = sel.parse::<usize>() {
        list.get(n.saturating_sub(1))
    } else {
        list.iter().find(|c| c.course_id == sel)
    };
    let Some(c) = chosen else {
        anyhow::bail!("未找到课程「{}」：运行 `UnipusAI courses` 查看可用课程", sel);
    };
    let mut cfg = session.cfg().clone();
    let old_class = cfg.class_id.clone();
    cfg.course_id = c.course_id.clone();
    if !c.class_id.is_empty() {
        cfg.class_id = c.class_id.clone();
    }
    if !c.curricula_id.is_empty() {
        cfg.curricula_id = c.curricula_id.clone();
    } else if old_class != cfg.class_id {
        // 列表异常未返回 curricula_id：换课程时清空旧值避免用错
        if !cfg.curricula_id.is_empty() {
            println!(
                "注意：课程列表未返回 curricula_id，已清空旧值（原 {}）；讨论题需手动填写新值（页面 URL 的 cloudCurriculaId）",
                cfg.curricula_id
            );
        }
        cfg.curricula_id.clear();
    }
    session.update_config(cfg)?;
    // 缓存课程名，供 TUI 离线显示
    if let Ok(conn) = UnipusAI::db::open_for(&session.open_id()) {
        let _ = UnipusAI::db::save_meta(&conn, &format!("course_name:{}", c.course_id), &c.name);
    }
    println!(
        "已选择课程: {}{}  {}",
        if c.group_label.is_empty() {
            String::new()
        } else {
            format!("[{}] ", c.group_label)
        },
        c.name,
        c.course_id
    );
    if c.class_id.is_empty() {
        println!("提示：未获取到班级信息，讨论题不可用；其余功能正常");
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

/// dump-text CLI 入口（逻辑见 UnipusAI::dump_text，供 CLI 与 TUI 共用）。
async fn cmd_dump_text(session: &Session, unit_ids: &[String]) -> Result<()> {
    let (force, rest) = split_flag(unit_ids, "--force");
    let (with_names, unit_ids) = split_flag(&rest, "--names");
    let opts = UnipusAI::dump_text::DumpOptions {
        force,
        with_names,
        unit_ids,
        group_ids: Vec::new(),
    };
    let reporter = UnipusAI::reporter::PrintReporter;
    let cancel = tokio_util::sync::CancellationToken::new();
    let s = UnipusAI::dump_text::run_dump_text(session, &opts, &reporter, &cancel).await?;
    println!(
        "dump-text 完成: 单元 {} 个, 新生成 {} 个, 已存在 {} 个(其中刷新状态 {} 个), 库内累计任务组 {} 个",
        s.units, s.generated, s.skipped, s.updated, s.total_files
    );
    println!(
        "本次新生成: 模块 {} 个, 题目 {} 道, 媒体转写 {} 条, 共 {} 字符",
        s.modules, s.questions, s.media, s.media_chars
    );
    println!("数据已保存到 {}", UnipusAI::db::db_path_for(&session.open_id()).display());
    Ok(())
}

async fn cmd_debug(session: &Session, args: &[String]) -> Result<()> {
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
    let preview = UnipusAI::preview::load_preview(session, group_id).await?;
    if let Some(pretty) = &preview.json_pretty {
        println!("=== 解密后完整 JSON ===");
        println!("{}", pretty);
    }
    let Some(group) = &preview.group else {
        println!("[浏览类页面] 内容为空/非 JSON/无题目模块，无题目数据；run/group 将直接标记已看");
        let trimmed = preview.plain.trim();
        if !trimmed.is_empty() {
            println!(
                "【原始内容】({}字)\n{}",
                preview.plain.chars().count(),
                UnipusAI::api::parser::truncate_text(trimmed, 2000)
            );
        }
        return Ok(());
    };
    let has_discussion = group.modules.iter().any(|m| m.reply_type == "discussion");
    let vocab = &preview.vocab;
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
        UnipusAI::dump::sync_task_status(session, &task, true);
        return Ok(());
    }
    let reporter = UnipusAI::reporter::PrintReporter;
    let cancel = tokio_util::sync::CancellationToken::new();
    match UnipusAI::core::runner::process_group(session, &task, &reporter, &cancel).await {
        Ok(_) => {}
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
    let reporter = UnipusAI::reporter::PrintReporter;
    let cancel = tokio_util::sync::CancellationToken::new();
    let summary = if unit_ids.is_empty() {
        UnipusAI::core::runner::run_course(&mut session, with_names, &reporter, &cancel).await?
    } else {
        UnipusAI::core::runner::run_course_units(
            &mut session,
            &unit_ids,
            with_names,
            &reporter,
            &cancel,
        )
        .await?
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
  UnipusAI                    启动交互式 TUI（推荐；任务/预览来自当前账号的 dump_text/dump-<open_id>.db，含课程浏览/运行/导出/设置；p 预览、D 导出）
  UnipusAI <命令> [参数]      命令行模式（见下方命令）

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
      导出题目文本与媒体转写到当前账号的 SQLite 数据库 dump_text/dump-<open_id>.db（不答题；多账号隔离）
      保存单元索引/题型/必修/完成情况/模块/答题说明/材料文本/媒体转写/选项等全部内容
      所有叶子全量导出；浏览类页面（内容为空/非 JSON/无题目模块）按 view-only 保存原始内容
      run/group 答题完成后自动同步状态；--force 清空数据库并重新生成

  login [--force]
      查看登录状态（jwt/refresh_token 有效期）；--force 用账号密码强制重新登录

  courses
      列出账号下全部课程（* 标记当前选择；首页列表为空时自动改用"我的教材"）

  course [序号|courseId]
      查看或选择当前课程（写入 config.json，所有命令共用）

  annotator [--extract]
      查看 x-annotator-auth-token 签发参数与剩余有效期；
      --extract 强制从 ucontent 前端 bundle 重新提取参数并重签（密钥轮换时自动自愈用）

参数:
  --names       显示课程名与单元名（如 新视野大学英语(第四版)读写教程 / U1 Pre-reading activities），
                结果缓存到 .unit_labels.json
  --interval    两次提交间隔，默认 3000ms，如 --interval 5000 或 --interval=5000
  --force       dump-text 清空并全量重新生成；group/debug 忽略“已通过”跳过、强制重做/生成；login 强制重登
  <unitId...>   只处理指定单元（可多个），省略则处理全部单元

示例:
  UnipusAI login
  UnipusAI courses
  UnipusAI course 1
  UnipusAI progress --names
  UnipusAI run --interval 5000
  UnipusAI run 6bbb2df99001b1e
  UnipusAI group 6bbb307f30d1b1e
  UnipusAI debug 6bbb307f30d1b1e
  UnipusAI dump-text --force

说明:
  config.json 推荐只填 username/password（自动登录并刷新 cookie/open_id，24h 内不重复登录）；
  也可继续使用 cookie（含 jwt=）方式。class_id/curricula_id/u_school/publish_version 每次启动
  自动从接口刷新，x_annotator_auth_token 由程序本地签发（1 年有效，剩余 <30 天自动续签），
  均无需手填；unit_id 已无需填写。
"#
    );
}
fn print_hello() -> Result<()> {
    let font = FIGlet::standard().map_err(|e| anyhow::anyhow!(e))?;
    if let Some(title) = font.convert("UnipusAI") {
        println!("{}", title.to_string().cyan());
    }
    println!("{}","UnipusAI_v3.5\n\t\t\t--by Zzj\nU校园AI版自动刷题脚本".truecolor(255, 153, 255).bold());
    println!("{}",r#"v3.5更新日志
        - 新增**账号密码自动登录**：`config.json` 只填 `username`/`password` 即可；启动时 jwt 剩余 >24h 直接用、否则 refresh_token 刷新、失败再用账密重登，凭证自动写回 cookie/open_id/refresh_token；运行中接口 401 自动刷新并重试（单飞防并发）；受服务端验证码（极验/图形）限制时给出明确提示并保留 cookie 兜底
        - 新增 CLI `login [--force]`（查看/刷新登录态）、`courses`（列出账号课程）、`course <序号|id>`（选择课程，所有命令共用，自动填充 course_id/class_id）
        - TUI 新增课程选择界面（`c` 键，未选课程时启动自动打开）；设置页新增 username/password 字段，保存后自动尝试登录
        - 数据库 `tasks` 新增 `course_id` 列并按课程隔离（TUI 任务树/dump 汇总只显示当前课程，旧数据按 meta 自动回填）
        - `x_annotator_auth_token` 改为**本地自动签发**（前端同算法/同密钥的 HS256 JWT，1 年有效，剩余 <30 天自动续签）——至此凭证类字段全部无需手动填写
        - annotator 签名参数改为**按需从 ucontent 前端 bundle 动态提取**（仅 token 需重签或请求 401 时触发），密钥/iss/aud/TTL 轮换后可自动自愈；401 处理升级为"jwt 刷新 → annotator 重提取重签 → 提示手动兜底"阶梯；新增 `annotator [--extract]` 命令查看/强制重提取"#.green()
    );
    println!("{}","提示：如果你现在用的是v3.4及以前的config.json请更新到v3.5版本的".bright_magenta().bold());
    println!("{}","本软件完全免费，并且在https://github.com/Zzj-klwgxdz/UnipusAI上开源，作者未授权给任何人售卖。如果你是通过购买获得的，请立刻退货，并向平台举报".yellow().bold());
    println!("{}","导狗死全家".red().bold());
    Ok(())
}

/// 仅交互终端等待任意按键；忽略按键松开（Windows 下启动命令的 Enter 弹起事件会残留）。
fn wait_any_key() -> Result<()> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Ok(());
    }
    println!("输入任意按键继续");
    std::io::stdout().flush()?;
    terminal::enable_raw_mode()?;
    loop {
        match event::read() {
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    terminal::disable_raw_mode()?;
    Ok(())
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
