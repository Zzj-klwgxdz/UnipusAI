use UnipusAI::api::session::Session;
use UnipusAI::config::Config;
use anyhow::{Context, Result};
use std::io::IsTerminal;
use colored::Colorize;
#[cfg(not(debug_assertions))]
use crossterm::event::{self, Event, KeyEventKind};
#[cfg(not(debug_assertions))]
use crossterm::terminal;
#[cfg(not(debug_assertions))]
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
    #[cfg(not(debug_assertions))]
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
        #[cfg(not(debug_assertions))]
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
    if matches!(cmd, "chaoxing" | "cx") {
        return UnipusAI::platform::chaoxing::cli::run(cfg, config_path, &args[2..]).await;
    }
    if matches!(cmd, "zhihuishu" | "zhs" | "zhidao") {
        return cmd_zhihuishu(cfg, &config_path, &args[2..]).await;
    }
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
        "progress" | "run" | "group" | "debug" | "test-types" | "dump-text" | "probe"
    ) {
        UnipusAI::api::login::ensure_course(&session)?;
    }

    match cmd {
        "progress" => cmd_progress(&session, &args[2..]).await?,
        "group" => cmd_group(&session, &args[2..]).await?,
        "debug" => cmd_debug(&session, &args[2..]).await?,
        "probe" => cmd_probe(&session, &args[2..]).await?,
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

/// `zhihuishu <login|chapters|homeworks|submitted|paper|solve|video>`。
async fn cmd_zhihuishu(mut cfg: Config, config_path: &std::path::Path, args: &[String]) -> Result<()> {
    use UnipusAI::platform::zhihuishu::{ZsAnswer, ZhihuishuClient};
    let sub = args.first().map(|s| s.as_str()).unwrap_or("help");
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    let flag = |name: &str| rest.iter().any(|a| a == name);
    let opt_val = |name: &str| -> Option<String> {
        let mut it = rest.iter();
        while let Some(a) = it.next() {
            if a == name {
                return it.next().cloned();
            }
            if let Some(v) = a.strip_prefix(&format!("{}=", name)) {
                return Some(v.to_string());
            }
        }
        None
    };
    let login_timeout = opt_val("--timeout")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(240);
    // 首次使用：没 Cookie 但有账号密码时，自动打开浏览器登录一次
    if sub != "login"
        && cfg.zhihuishu.cookie.is_empty()
        && !cfg.zhihuishu.username.is_empty()
        && !cfg.zhihuishu.password.is_empty()
    {
        println!("尚未登录知到，正在打开浏览器…");
        let outcome =
            UnipusAI::platform::zhihuishu::login::login_with_browser(&cfg.zhihuishu, login_timeout).await?;
        cfg.zhihuishu.cookie = outcome.cookie;
        if !outcome.uuid.is_empty() {
            cfg.zhihuishu.uuid = outcome.uuid;
        }
        cfg.save(config_path)?;
        println!("登录成功，Cookie 已保存");
    }
    let client = ZhihuishuClient::new(&cfg.zhihuishu, cfg.timeout)?;

    match sub {
        "login" => {
            // 先看现有 Cookie 是否可用（需要已配置课程）
            if !cfg.zhihuishu.cookie.is_empty()
                && cfg.zhihuishu.recruit_id != 0
                && cfg.zhihuishu.course_id != 0
            {
                match client.fetch_homeworks(1).await {
                    Ok(list) => {
                        println!("Cookie 有效：未提交作业 {} 个", list.len());
                        return Ok(());
                    }
                    Err(e) => println!("现有 Cookie 不可用（{:#}），准备浏览器登录…", e),
                }
            }
            println!("将打开系统浏览器（Edge/Chrome）登录知到；请手动完成验证码。");
            let outcome =
                UnipusAI::platform::zhihuishu::login::login_with_browser(&cfg.zhihuishu, login_timeout)
                    .await?;
            let mut nc = cfg.clone();
            nc.zhihuishu.cookie = outcome.cookie;
            if !outcome.uuid.is_empty() {
                nc.zhihuishu.uuid = outcome.uuid;
            }
            nc.save(config_path)?;
            println!(
                "登录成功，Cookie 已写入 {}（uuid={}）",
                config_path.display(),
                if nc.zhihuishu.uuid.is_empty() {
                    "未取到"
                } else {
                    &nc.zhihuishu.uuid
                }
            );
        }
        "chapters" => {
            let chapters = client.fetch_chapters().await?;
            let mut total = 0;
            for (ci, c) in chapters.iter().enumerate() {
                println!("{}. {}（{} 节）", ci + 1, c.name, c.lessons.len());
                for (li, l) in c.lessons.iter().enumerate() {
                    total += 1;
                    println!(
                        "   {}.{} {}  [lesson={} video={}]  {}s",
                        ci + 1,
                        li + 1,
                        l.name,
                        l.id,
                        l.video_id,
                        l.sec
                    );
                }
            }
            println!("共 {} 章 / {} 节", chapters.len(), total);
        }
        "homeworks" | "submitted" => {
            let flag_no = if sub == "submitted" { 2 } else { 1 };
            let list = client.fetch_homeworks(flag_no).await?;
            for h in &list {
                println!(
                    "{} {}  exam={} 题数={} 总分={} 状态={}{}",
                    h.id,
                    h.name,
                    h.exam_id,
                    h.problem_num,
                    h.total_score,
                    h.state,
                    h.score
                        .as_ref()
                        .map(|s| format!(" 得分={s}"))
                        .unwrap_or_default()
                );
            }
            println!("共 {} 个", list.len());
        }
        "paper" => {
            let id = rest.first().cloned().unwrap_or_default();
            let hw = find_homework(&client, &id).await?;
            let paper = client.fetch_paper(&hw).await?;
            println!("{}（{} 题）", paper.name, paper.questions.len());
            for (i, q) in paper.questions.iter().enumerate() {
                println!("\n{}. 【{}】{}（{} 分）", i + 1, q.type_name, q.text, q.score);
                for (j, o) in q.options.iter().enumerate() {
                    println!("   {}. {}", UnipusAI::platform::zhihuishu::solve::letter(j), o.text);
                }
            }
        }
        "solve" | "answer" => {
            if !UnipusAI::platform::zhihuishu::HOMEWORK_SUBMIT_ENABLED {
                println!(
                    "{}",
                    UnipusAI::platform::zhihuishu::HOMEWORK_SUBMIT_DISABLED_MSG
                );
                return Ok(());
            }
            let id = rest.first().cloned().unwrap_or_default();
            let hw = find_homework(&client, &id).await?;
            let paper = client.fetch_paper(&hw).await?;
            println!("{}（{} 题）", paper.name, paper.questions.len());
            let mut answers: Vec<ZsAnswer> = Vec::new();
            for (i, q) in paper.questions.iter().enumerate() {
                match UnipusAI::platform::zhihuishu::solve::solve_question(&cfg, q).await {
                    Ok(ids) => {
                        let text: Vec<String> = ids
                            .iter()
                            .filter_map(|id| {
                                q.options
                                    .iter()
                                    .find(|o| o.id == *id)
                                    .map(|o| o.text.clone())
                            })
                            .collect();
                        println!("{}. {} -> {}", i + 1, q.text, text.join(" / "));
                        answers.push(ZsAnswer {
                            eid: q.eid.clone(),
                            type_id: q.type_id,
                            option_ids: ids,
                        });
                    }
                    Err(e) => println!("{}. {} -> 作答失败: {:#}", i + 1, q.text, e),
                }
            }
            if flag("--dry") {
                println!("（--dry：不保存、不提交）");
                return Ok(());
            }
            let mut saved = 0;
            for a in &answers {
                match client.save_answer(&hw, a).await {
                    Ok(()) => saved += 1,
                    Err(e) => eprintln!("保存失败（{}）: {:#}", a.eid, e),
                }
            }
            println!("已保存 {}/{} 题", saved, answers.len());
            if saved == 0 && !answers.is_empty() {
                anyhow::bail!(
                    "一道题都没保存成功（服务端可能要求滑块验证），为避免 0 分交卷已跳过提交"
                );
            }
            if flag("--submit") {
                let score = client.submit(&hw, answers.len()).await?;
                println!(
                    "已交卷：{}",
                    score
                        .map(|s| format!("得分 {s}"))
                        .unwrap_or_else(|| "未返回分数".into())
                );
            }
        }
        "video" => {
            let step = opt_val("--step")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(cfg.zhihuishu.video_step)
                .max(1);
            let sleep_sec = opt_val("--sleep")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(step);
            let dry = flag("--dry");
            let want_chapter = opt_val("--chapter").and_then(|v| v.parse::<i64>().ok());
            let want_lesson = opt_val("--lesson").and_then(|v| v.parse::<i64>().ok());
            let chapters = client.fetch_chapters().await?;
            let mut verify_rounds = 0u32;
            for c in &chapters {
                if want_chapter.is_some_and(|w| c.id != w) {
                    continue;
                }
                for l in &c.lessons {
                    if want_lesson.is_some_and(|w| l.id != w) {
                        continue;
                    }
                    let v = client.lesson_progress(l).await?;
                    let prelearn_id = v
                        .pointer("/data/studiedLessonDto/id")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    let learned = v
                        .pointer("/data/studiedLessonDto/learnTimeSec")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0)
                        .max(0) as u64;
                    let study_total = v
                        .pointer("/data/studiedLessonDto/studyTotalTime")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0)
                        .max(0) as u64;
                    println!(
                        "{}（{}s，已学 {}s，服务器累计 {}s，prelearn={}）",
                        l.name, l.sec, learned, study_total, prelearn_id
                    );
                    if learned >= l.sec && l.sec > 0 {
                        println!("  已完成，跳过");
                        continue;
                    }
                    if dry {
                        println!(
                            "  --dry：计划上报 {} 次（每次 {}s）",
                            l.sec.div_ceil(step),
                            step
                        );
                        continue;
                    }
                    if prelearn_id == 0 {
                        eprintln!("  缺少 prelearnId，跳过");
                        continue;
                    }
                    let gap = opt_val("--gap")
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(2);
                    if gap > 0 {
                        tokio::time::sleep(std::time::Duration::from_secs(gap)).await;
                    }
                    // total = 服务器累计学习时长（studyTotalTime），pos = 当前播放位置
                    let mut total = study_total.max(learned);
                    let mut pos = learned;
                    let mut i = 0u32;
                    while pos < l.sec {
                        if i > 0 {
                            tokio::time::sleep(std::time::Duration::from_secs(sleep_sec)).await;
                        }
                        let next = (pos + step).min(l.sec);
                        let delta = next - pos;
                        let mut attempt = 0u32;
                        let report_total = total + delta;
                        let result = loop {
                            match client
                                .report_video(l, prelearn_id, delta, report_total, next)
                                .await
                            {
                                Ok(resp) => break Ok(resp),
                                Err(e) => {
                                    let msg = format!("{:#}", e);
                                    let limited = msg.contains("429")
                                        || msg.contains("Too Many")
                                        || msg.contains("502")
                                        || msg.contains("503");
                                    if limited && attempt < 4 {
                                        attempt += 1;
                                        let wait = 10 * attempt as u64;
                                        println!("  被限流，{}s 后重试（{}/4）", wait, attempt);
                                        tokio::time::sleep(std::time::Duration::from_secs(wait))
                                            .await;
                                        continue;
                                    }
                                    break Err(e);
                                }
                            }
                        };
                        match result {
                            Ok(resp) => {
                                let code = resp.get("code").and_then(|x| x.as_i64()).unwrap_or(0);
                                if code == -12 || code == -13 {
                                    verify_rounds += 1;
                                    let page = format!(
                                        "https://studyvideoh5.zhihuishu.com/stuStudy?recruitAndCourseId={}",
                                        cfg.zhihuishu.recruit_and_course_id
                                    );
                                    eprintln!(
                                        "  触发风控（code={}，第 {} 次）：{}",
                                        code,
                                        verify_rounds,
                                        if code == -12 {
                                            "需要在播放页完成字符点选验证"
                                        } else {
                                            "需要在播放页完成人脸验证"
                                        }
                                    );
                                    eprintln!("  播放页: {}", page);
                                    let wait_verify = flag("--wait-verify")
                                        || (std::io::stdin().is_terminal()
                                            && !flag("--no-wait"));
                                    if (flag("--open-browser")
                                        || wait_verify
                                        || flag("--wait-verify"))
                                        && cfg!(windows)
                                    {
                                        let _ = std::process::Command::new("cmd")
                                            .args(["/C", "start", "", &page])
                                            .spawn();
                                    }
                                    if wait_verify && verify_rounds <= 120 {
                                        // 类似 Autovisor：打开浏览器后不抢答，轮询等待人工完成验证
                                        eprintln!(
                                            "  请在浏览器完成验证，程序每 5 秒自动重试（最多 10 分钟）…"
                                        );
                                        tokio::time::sleep(std::time::Duration::from_secs(5))
                                            .await;
                                        continue;
                                    }
                                    eprintln!(
                                        "  提示：加 --wait-verify 可在验证后自动重试；或稍后重跑本命令"
                                    );
                                    break;
                                }
                                if code == -8 {
                                    eprintln!(
                                        "  服务器记录的总时长更高（code=-8），本节跳过；请用播放页真实观看该节"
                                    );
                                    break;
                                }
                                let ok = resp
                                    .pointer("/data/submitSuccess")
                                    .and_then(|x| x.as_bool())
                                    .unwrap_or(false);
                                println!("  上报 {}s/{}s -> submitSuccess={}", next, l.sec, ok);
                                if !ok {
                                    let brief: String = resp.to_string().chars().take(300).collect();
                                    eprintln!("  服务器返回: {}", brief);
                                }
                            }
                            Err(e) => {
                                eprintln!("  上报失败: {:#}", e);
                                break;
                            }
                        }
                        total = report_total;
                        pos = next;
                        i += 1;
                    }
                }
            }
        }
        other => {
            eprintln!("未知子命令: {}", other);
            println!("用法: zhihuishu <login|chapters|homeworks|submitted|paper|solve|video>");
        }
    }
    Ok(())
}


async fn find_homework(
    client: &UnipusAI::platform::zhihuishu::ZhihuishuClient,
    id: &str,
) -> Result<UnipusAI::platform::zhihuishu::ZsHomework> {
    if id.is_empty() {
        anyhow::bail!("请提供作业 id（先运行 `zhihuishu homeworks` 查看）");
    }
    for flag in [1u8, 2u8] {
        let list = client.fetch_homeworks(flag).await?;
        if let Some(hw) = list.into_iter().find(|h| h.id == id) {
            return Ok(hw);
        }
    }
    anyhow::bail!("未找到作业 {}", id)
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
                    login::mask_key(&p.key),
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
        login::mask_key(&params.key)
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
    if let Ok(conn) = UnipusAI::db::open_site(UnipusAI::db::SITE_UNIPUS) {
        let _ = UnipusAI::db::save_meta(
            &conn,
            &session.open_id(),
            &format!("course_name:{}", c.course_id),
            &c.name,
        );
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
    println!(
        "数据已保存到 {}",
        UnipusAI::db::db_path_for_site(UnipusAI::db::SITE_UNIPUS).display()
    );
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
    if let Some(un) = UnipusAI::probe::detect(group) {
        println!(
            "[未适配题型] {}；run/group 将跳过提交，可用 `probe {} --bundle` 生成适配报告",
            un.reason, group_id
        );
    }
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
        Ok(UnipusAI::core::runner::ProcessResult::Submitted { .. }) => {}
        Ok(UnipusAI::core::runner::ProcessResult::Skipped { reason }) => {
            println!("[SKIP] {} -> {}", group_id, reason);
        }
        Err(e) => println!("[FAIL] {} -> {:#}", task.tab_type, e),
    }
    Ok(())
}

/// `probe`：未适配题型探针报告（默认干跑；--submit 真实提交一次取服务端响应）。
async fn cmd_probe(session: &Session, args: &[String]) -> Result<()> {
    let (all_unknown, rest) = split_flag(args, "--all-unknown");
    let (submit, rest) = split_flag(&rest, "--submit");
    let (bundle, rest) = split_flag(&rest, "--bundle");
    let (force, rest) = split_flag(&rest, "--force");
    let (limit_arg, rest) = extract_flag_value(&rest, "--limit");
    let limit: usize = match limit_arg {
        Some(v) => v.trim().parse().context("--limit 需要正整数")?,
        None => 20,
    };
    if all_unknown {
        if submit {
            anyhow::bail!("--all-unknown 不能与 --submit 同用（避免批量真实提交）");
        }
        return probe_all_unknown(session, bundle, limit).await;
    }
    let group_id = rest.first().map(|s| s.as_str()).unwrap_or_default();
    if group_id.is_empty() {
        anyhow::bail!(
            "用法: UnipusAI probe <groupId> [--submit] [--bundle] [--force]\n  或: UnipusAI probe --all-unknown [--bundle] [--limit N]"
        );
    }
    probe_one(session, group_id, submit, bundle, force).await
}

async fn probe_one(
    session: &Session,
    group_id: &str,
    submit: bool,
    bundle: bool,
    force: bool,
) -> Result<()> {
    use UnipusAI::probe::{self, ReportMeta};

    let rt = UnipusAI::api::content::fetch_content(session, group_id).await?;
    let plain = UnipusAI::api::content::decrypt_content(&rt.content, &rt.k)?;
    let decrypted = UnipusAI::api::content::parse_decrypted(&plain).unwrap_or_else(|_| {
        serde_json::json!({
            "non_json_raw": UnipusAI::api::parser::truncate_text(&plain, 20000),
        })
    });
    let group = UnipusAI::api::parser::parse_group(&decrypted).ok();
    let detected = group.as_ref().and_then(probe::detect);

    let db_meta = UnipusAI::db::open_site(UnipusAI::db::SITE_UNIPUS)
        .ok()
        .and_then(|c| {
            UnipusAI::db::task_meta(&c, &session.open_id(), session.course_id(), group_id)
                .ok()
                .flatten()
        });
    let type_name = detected
        .as_ref()
        .map(|u| u.type_name.clone())
        .or_else(|| db_meta.as_ref().map(|m| m.group_type.clone()))
        .unwrap_or_else(|| "unknown".to_string());
    let reason = detected
        .as_ref()
        .map(|u| u.reason.clone())
        .unwrap_or_else(|| "手动触发（未检测到未适配类型，报告供对照）".to_string());

    if submit {
        let task = UnipusAI::core::runner::mock_task(session, group_id).await?;
        if task.passed && !force {
            anyhow::bail!(
                "{} 已通过：真实提交探测会覆盖作答记录，需加 --force 确认",
                group_id
            );
        }
        if group.is_none() {
            println!("[probe] 该组无题目模块，无内容可提交");
        }
    }

    let payload = match &group {
        Some(g) if submit => {
            let answers = probe::fallback_answers(session, g).await;
            probe::payload_with_answers(session, group_id, &answers)
        }
        Some(g) => probe::empty_payload(session, group_id, g),
        None => None,
    };

    let mut submit_response = None;
    if submit && let Some(p) = &payload {
        let body = serde_json::to_string(p)?;
        let (status, text) = session
            .post_raw(UnipusAI::api::session::submit_url(), &body)
            .await?;
        let resp: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| serde_json::json!({"http_status": status.as_u16(), "body": text}));
        println!(
            "[probe] 提交响应 HTTP {}: {}",
            status,
            summarize_submit_response(&resp)
        );
        submit_response = Some(resp);
    }

    let bundle_hits = if bundle {
        probe::bundle_hits(&type_name).await
    } else {
        Vec::new()
    };

    let meta = ReportMeta {
        group_id: group_id.to_string(),
        unit_id: db_meta.as_ref().map(|m| m.unit_id.clone()).unwrap_or_default(),
        tab_type: db_meta.as_ref().map(|m| m.tab_type.clone()).unwrap_or_default(),
        group_type: db_meta
            .as_ref()
            .map(|m| m.group_type.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| type_name.clone()),
        required: db_meta.as_ref().map(|m| m.required).unwrap_or(false),
        passed: db_meta.as_ref().map(|m| m.passed).unwrap_or(false),
        trigger: "手动 probe".to_string(),
        reason,
    };
    let path = probe::write_report(
        session,
        &meta,
        &decrypted,
        group.as_ref(),
        payload.as_ref(),
        submit_response.as_ref(),
        &bundle_hits,
    )?;
    println!(
        "[probe] 题型={} 模块={} 子题={} 报告={}",
        type_name,
        group.as_ref().map(|g| g.modules.len()).unwrap_or(0),
        group.as_ref().map(|g| g.child_count()).unwrap_or(0),
        path.display()
    );
    if let Some(un) = &detected {
        println!("[probe] 未适配: {}", un.reason);
    } else {
        println!("[probe] 未检测到未适配类型（报告仅供对照）");
    }
    Ok(())
}

async fn probe_all_unknown(session: &Session, bundle: bool, limit: usize) -> Result<()> {
    use UnipusAI::probe::{self, ReportMeta};
    let conn = UnipusAI::db::open_site(UnipusAI::db::SITE_UNIPUS)
        .context("打开 U 校园数据库失败")?;
    let units = UnipusAI::db::load_tree(&conn, &session.open_id(), session.course_id())?;
    if units.is_empty() {
        anyhow::bail!(
            "当前课程在本地库中无数据：请先运行 UnipusAI dump-text（库: {}）",
            UnipusAI::db::db_path_for_site(UnipusAI::db::SITE_UNIPUS).display()
        );
    }
    let mut fetched = 0usize;
    let mut reported = 0usize;
    let mut prefiltered = 0usize;
    let mut bundle_cache: std::collections::HashMap<String, Vec<(String, String)>> =
        std::collections::HashMap::new();
    'outer: for unit in &units {
        for task in &unit.tasks {
            if task.kind != "task" || task.passed {
                continue;
            }
            // 预筛：数据库题型名的每一段都已适配，则无需抓取
            if group_type_all_known(&task.group_type) {
                prefiltered += 1;
                continue;
            }
            if fetched >= limit {
                break 'outer;
            }
            fetched += 1;
            println!("[probe] ({}/{}) 检查 {} ...", fetched, limit, task.group_id);
            let rt = match UnipusAI::api::content::fetch_content(session, &task.group_id).await {
                Ok(v) => v,
                Err(e) => {
                    println!("  ! 抓取失败: {:#}", e);
                    continue;
                }
            };
            let plain = UnipusAI::api::content::decrypt_content(&rt.content, &rt.k)?;
            let decrypted = UnipusAI::api::content::parse_decrypted(&plain).unwrap_or_else(|_| {
                serde_json::json!({
                    "non_json_raw": UnipusAI::api::parser::truncate_text(&plain, 20000),
                })
            });
            let Some(group) = UnipusAI::api::parser::parse_group(&decrypted).ok() else {
                println!("  - 无题目模块，跳过");
                continue;
            };
            let Some(un) = probe::detect(&group) else {
                println!("  - 已适配，跳过");
                continue;
            };
            let hits = if bundle {
                if let Some(v) = bundle_cache.get(&un.type_name) {
                    v.clone()
                } else {
                    let v = probe::bundle_hits(&un.type_name).await;
                    bundle_cache.insert(un.type_name.clone(), v.clone());
                    v
                }
            } else {
                Vec::new()
            };
            let meta = ReportMeta {
                group_id: task.group_id.clone(),
                unit_id: unit.unit_id.clone(),
                tab_type: task.tab_type.clone(),
                group_type: task.group_type.clone(),
                required: task.required,
                passed: task.passed,
                trigger: "probe --all-unknown".to_string(),
                reason: un.reason.clone(),
            };
            let payload = probe::empty_payload(session, &task.group_id, &group);
            let path = probe::write_report(
                session,
                &meta,
                &decrypted,
                Some(&group),
                payload.as_ref(),
                None,
                &hits,
            )?;
            reported += 1;
            println!("  -> {}（{}）", path.display(), un.type_name);
        }
    }
    println!(
        "[probe] 扫描完成: 抓取检查 {} 组, 生成报告 {} 份, 预筛跳过已适配 {} 组",
        fetched, reported, prefiltered
    );
    Ok(())
}

fn group_type_all_known(group_type: &str) -> bool {
    !group_type.is_empty()
        && group_type
            .split('+')
            .all(|part| !part.is_empty() && UnipusAI::solve::is_known_reply_type(part))
}

fn summarize_submit_response(v: &serde_json::Value) -> String {
    let code = v.get("code").map(|c| c.to_string()).unwrap_or_default();
    let msg = v
        .get("message")
        .or_else(|| v.get("msg"))
        .and_then(|m| m.as_str())
        .unwrap_or("");
    match UnipusAI::api::submit::parse_submit_score(v) {
        Some(score) => format!(
            "code={} msg={} 提交后得分 {}",
            code,
            msg,
            score.display()
        ),
        None => format!("code={} msg={} 无判分数据", code, msg),
    }
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
                t.tab_type.green(),
                t.group_id,
                t.required,
                t.passed,
                if t.passed { "✔".green() } else { "✗".red() }
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
  UnipusAI                    启动交互式 TUI（推荐；F1 U校园 / F2 学习通 / F3 知到；U校园任务/预览来自 dump_text/unipusai.db，库内按账号隔离；p 预览、d 导出）
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

  probe <groupId> [--submit] [--bundle] [--force]
      生成未适配题型探针报告（reports/<题型>__<groupId>.md：解密全文/解析对照/提交载荷/适配清单）
      默认干跑不提交；--submit 用通用兜底答案真实提交一次并记录服务端响应（已通过任务需 --force）
      --bundle 额外检索 ucontent 前端 bundle 中的题型线索

  probe --all-unknown [--bundle] [--limit N]
      从当前账号数据库筛选未通过且题型未知的任务组，批量生成报告（不提交；默认最多检查 20 组）

  test-types
      每种题型抽一题测试答题链路（不提交）

  transcribe <url>
      测试媒体转写链路（下载 -> ffmpeg -> whisper）

  dump-text [--names] [--force] [unitId...]
      导出题目文本与媒体转写到平台数据库 dump_text/unipusai.db（不答题；库内按账号隔离）
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

  chaoxing <login|courses|chapters|tasks|video|quiz> ...
      学习通平台（实验性，当前不稳定、暂不可用，仅作开发留档）
      登录、课程/章节/任务点、视频补报、测验作答
      示例: UnipusAI chaoxing login / courses / chapters 1 / quiz 1 9 2 --solve --submit

  zhihuishu <login|chapters|homeworks|submitted|paper|solve|video> ...
      知到/智慧树平台（实验性，当前不稳定、暂不可用，仅作开发留档）
      章节/作业列表、整卷读取、视频学时上报；solve（作业作答/提交）已关闭
      示例: UnipusAI zhihuishu login / chapters / homeworks / paper <id> / video
            UnipusAI zhihuishu video --step 100000 --sleep 0 --gap 3

参数:
  --names       显示课程名与单元名（如 新视野大学英语(第四版)读写教程 / U1 Pre-reading activities），
                结果缓存到 .unit_labels.json
  --interval    两次提交间隔，默认 3000ms，如 --interval 5000 或 --interval=5000
  --force       dump-text 清空并全量重新生成；group/debug 忽略“已通过”跳过、强制重做/生成；login 强制重登
  --submit      probe 真实提交一次以获取服务端判分响应（仅未通过任务；已通过需配合 --force）
  --bundle      probe 额外检索 ucontent 前端 bundle（题型枚举/组件线索）
  --all-unknown probe 批量扫描未适配题型；--limit 指定最多检查组数（默认 20）
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
  UnipusAI probe 656ec937600159e --bundle
  UnipusAI probe --all-unknown --limit 10
  UnipusAI dump-text --force
  UnipusAI chaoxing login
  UnipusAI zhihuishu video --step 100000 --sleep 0 --gap 3

说明:
  config.json 推荐只填 username/password（自动登录并刷新 cookie/open_id，24h 内不重复登录）；
  也可继续使用 cookie（含 jwt=）方式。class_id/curricula_id/u_school/publish_version 每次启动
  自动从接口刷新，x_annotator_auth_token 由程序本地签发（1 年有效，剩余 <30 天自动续签），
  均无需手填；unit_id 已无需填写。

数据库: U 校园 dump_text/unipusai.db、学习通 dump_text/chaoxing.db；知到暂不落库。
库内用 account 列按账号隔离；旧版 dump-<open_id>.db 仅保留不再写入。
"#
    );
}
#[cfg(not(debug_assertions))]
fn print_hello() -> Result<()> {
    let font = FIGlet::standard().map_err(|e| anyhow::anyhow!(e))?;
    if let Some(title) = font.convert("UnipusAI") {
        println!("{}", title.to_string().cyan());
    }
    println!("{}","UnipusAI_v3.7\n\t\t\t--by Zzj\nU校园AI版自动刷题脚本".truecolor(255, 153, 255).bold());
    println!("{}",r#"v3.7更新日志
        - TUI 改为多平台标签页（F1 U校园 / F2 学习通 / F3 知到），各平台独立运行、互不干扰
        - 学习通作答结果落库并按账号隔离；视频任务搬进 TUI（v 补报、Esc 取消）
        - 新增 llm_votes 配置：同一题多次作答取多数，平票带候选再问一次
        - 新增 zhihuishu 平台：章节/作业列表、视频学时上报；支持账号密码登录（自动开浏览器过验证码）
        - 知到作业提交因服务端新增滑块验证**暂时停用**（提示“暂不可用”），避免误交 0 分
        - 修复学习通视频上报 403（dtoken 轮换需每次重新获取）
        - 数据库改为每个平台一个库，库内按账号隔离
        - 配置按平台拆分（unipusai/chaoxing 子对象 + 顶层公共配置），旧配置自动迁移
        - 新增学习通平台模块与 chaoxing 命令（登录/课程/章节/任务点/视频/测验，实验性）
        - 新增**未适配题型探针报告**：run/group/TUI 遇到未适配题型自动生成 reports/ 报告并跳过提交；新增 probe 命令（--submit/--bundle/--all-unknown），并修复无引号 script src 解析
        - 新增**提交得分展示**：CLI 每次提交后打印 `[SCORE] ... 提交后得分 100%（各题 …）`；TUI 顶栏实时显示最近一次得分（按课程持久化到数据库，重启/换课后自动恢复）
        - 修复若干 panic：cookie/open_id 等配置含换行等控制字符时启动崩溃（改为跳过并告警）、非 ASCII 日志文件名清理、中文 annotator 密钥展示、TUI 设置项索引越界"#.green()
    );
    println!("{}","提示：如果你现在用的是v3.4及以前的config.json,请按v3.6及以后版本的config.example.json补全字段（新增username/password、annotator_*等）".bright_magenta().bold());
    println!("{}","本软件完全免费，并且在https://github.com/Zzj-klwgxdz/UnipusAI上开源，作者未授权给任何人售卖。如果你是通过购买获得的，请立刻退货，并向平台举报".yellow().bold());
    println!("{}","导狗死全家".red().bold());
    Ok(())
}
#[cfg(not(debug_assertions))]
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
