//! 学习通命令行入口：`UnipusAI chaoxing <子命令>`。
//!
//! 目前提供：login / courses / chapters / tasks / video / quiz。
//! 这是移植阶段的手工验证入口，后续会与统一计划器/TUI 打通。

use crate::config::Config;
use crate::platform::chaoxing::quiz::QKind;
use crate::platform::chaoxing::{ChaoxingClient, course, quiz, video};
use crate::platform::types::{Course, TaskKind, TaskPoint};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::PathBuf;

pub async fn run(config: Config, config_path: PathBuf, args: &[String]) -> Result<()> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("help");
    if matches!(sub, "help" | "-h" | "--help") {
        print_help();
        return Ok(());
    }
    let mut client = ChaoxingClient::new(config, config_path)?;
    match sub {
        "login" => cmd_login(&mut client).await,
        "courses" => cmd_courses(&mut client).await,
        "chapters" => cmd_chapters(&mut client, &args[1..]).await,
        "tasks" => cmd_tasks(&mut client, &args[1..]).await,
        "video" => cmd_video(&mut client, &args[1..]).await,
        "quiz" => cmd_quiz(&mut client, &args[1..]).await,
        other => bail!("未知 chaoxing 子命令: {}（可用: login/courses/chapters/tasks/video/quiz）", other),
    }
}

fn print_help() {
    println!("学习通命令：");
    println!("  chaoxing login                          # 登录并缓存 cookie");
    println!("  chaoxing courses                        # 课程列表");
    println!("  chaoxing chapters <课程序号|course_id>   # 章节列表");
    println!("  chaoxing tasks <课程> <章节序号|章节id>   # 任务点");
    println!("  chaoxing video <课程> <章节> <任务> [--dry|--pace 秒 [--speed 倍]]  # 默认直接上报播放完");
    println!("  chaoxing quiz  <课程> <章节> <任务> [--answers \"1:C,2:对\"] [--submit]  # 测验");
}

async fn cmd_login(client: &mut ChaoxingClient) -> Result<()> {
    client.ensure_login().await?;
    let courses = client.fetch_courses().await?;
    println!("登录成功，共 {} 门课程", courses.len());
    Ok(())
}

async fn cmd_courses(client: &mut ChaoxingClient) -> Result<()> {
    client.ensure_login().await?;
    let courses = client.fetch_courses().await?;
    for (i, c) in courses.iter().enumerate() {
        println!(
            "{:>2}. {}  [course={} clazz={} cpi={}]",
            i + 1,
            c.name,
            c.course_id,
            c.extra_str("clazz_id"),
            c.extra_str("cpi")
        );
    }
    Ok(())
}

async fn cmd_chapters(client: &mut ChaoxingClient, args: &[String]) -> Result<()> {
    client.ensure_login().await?;
    let course = resolve_course(client, args.first().map(|s| s.as_str())).await?;
    client.set_course(&course)?;
    let chapters = client.fetch_chapters(&course).await?;
    let mut last_parent = String::new();
    for (i, ch) in chapters.iter().enumerate() {
        if ch.parent_title != last_parent {
            println!("\n{}", ch.parent_title);
            last_parent = ch.parent_title.clone();
        }
        println!(
            "{:>3}. {}{} {}",
            i + 1,
            ch.number,
            ch.title,
            if ch.completed { "✓" } else { "" }
        );
    }
    println!("\n共 {} 个章节", chapters.len());
    Ok(())
}

async fn cmd_tasks(client: &mut ChaoxingClient, args: &[String]) -> Result<()> {
    client.ensure_login().await?;
    let course = resolve_course(client, args.first().map(|s| s.as_str())).await?;
    client.set_course(&course)?;
    let chapter = resolve_chapter(client, &course, args.get(1).map(|s| s.as_str())).await?;
    let tasks = client.fetch_tasks(&course, &chapter).await?;
    println!("{} / {}", chapter.number, chapter.title);
    for (i, t) in tasks.iter().enumerate() {
        println!(
            "{:>2}. [{}] {}{}  id={}",
            i + 1,
            t.kind.as_str(),
            t.title,
            if t.finished { " ✓" } else { "" },
            t.id
        );
    }
    Ok(())
}

async fn cmd_video(client: &mut ChaoxingClient, args: &[String]) -> Result<()> {
    client.ensure_login().await?;
    let course = resolve_course(client, args.first().map(|s| s.as_str())).await?;
    client.set_course(&course)?;
    let chapter = resolve_chapter(client, &course, args.get(1).map(|s| s.as_str())).await?;
    let tasks = client.fetch_tasks(&course, &chapter).await?;
    let task = resolve_task(&tasks, args.get(2).map(|s| s.as_str()))?;
    if task.kind != TaskKind::Video {
        bail!("任务 {} 不是视频（类型 {}）", task.id, task.kind.as_str());
    }
    let cards = client.fetch_cards(&course, &chapter).await?;
    let defaults = cards
        .first()
        .map(|p| p.defaults.clone())
        .unwrap_or_default();
    let object_id = task.extra_str("objectId");
    let status = video::fetch_status(&client.session, &object_id, &defaults.cfid).await?;
    println!(
        "视频「{}」时长 {}s，dtoken={}",
        if status.filename.is_empty() { &task.title } else { &status.filename },
        status.duration,
        status.dtoken
    );
    // --dry：只看视频信息，不上报
    if args.iter().any(|s| s == "--dry") {
        println!("( --dry：未上报 )");
        return Ok(());
    }
    let duration = if status.duration > 0 {
        status.duration
    } else {
        task.extra_i64("attDuration").unwrap_or(0).max(0) as u64
    };
    if duration == 0 {
        bail!("无法确定视频时长，不能上报（status.duration 与 attDuration 都为空）");
    }
    // 组装一次上报所需的固定参数
    let make_report = |playing: u64, isdrag: u32| video::Report {
        clazz_id: course.extra_str("clazz_id"),
        userid: if defaults.userid.is_empty() {
            client.chaoxing().user_id.clone()
        } else {
            defaults.userid.clone()
        },
        jobid: task.id.clone(),
        object_id: object_id.clone(),
        cfid: defaults.cfid.clone(),
        course_id: course.course_id.clone(),
        cpi: if defaults.cpi.is_empty() {
            course.extra_str("cpi")
        } else {
            defaults.cpi.clone()
        },
        other_info: task.extra_str("otherInfo"),
        duration,
        att_duration: task.extra_i64("attDuration").unwrap_or(0),
        att_duration_enc: task.extra_str("attDurationEnc"),
        video_face_capture_enc: task.extra_str("videoFaceCaptureEnc"),
        playing_time: playing,
        clip_time: format!("0_{}", duration),
        isdrag,
    };

    // --pace [秒]：按真实时间间隔上报（服务端校验累计观看时长时用）
    if let Some(pos) = args.iter().position(|s| s == "--pace") {
        let step: u64 = args
            .get(pos + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);
        let step = step.max(5);
        // --speed：每次上报把进度前进 step*speed 秒（服务端一般只允许 ~2 倍速）
        let speed: f64 = args
            .iter()
            .position(|s| s == "--speed")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(1.0)
            .clamp(1.0, 4.0);
        let advance = ((step as f64) * speed).round() as u64;
        // --from：从指定播放位置开始（默认从 0 起步）；断点续传/避免倒退上报时用
        let start_from: u64 = args
            .iter()
            .position(|s| s == "--from")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
            .min(duration);
        println!(
            "按真实时间上报：每 {} 秒一次、每次前进 {} 秒（{}x），共约 {} 次（{}s）",
            step,
            advance,
            speed,
            duration.div_ceil(advance) + 1,
            duration
        );
        let mut playing = (start_from.saturating_add(advance)).min(duration);
        loop {
            // 每次上报都重新取 dtoken：dtoken 会轮换，复用会被服务端判重放（403）
            let resp = video::report(&client.session, &make_report(playing, 0)).await?;
            let passed = resp
                .get("isPassed")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            println!("  playingTime={} -> isPassed={}", playing, passed);
            if passed {
                println!("✓ 已标记为完成");
                return Ok(());
            }
            if playing >= duration {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(step)).await;
            playing = playing.saturating_add(advance).min(duration);
        }
        println!("已达到视频结尾，但服务端仍未标记完成（可能还有其它风控）");
        return Ok(());
    }

    // 默认行为：直接上报"已播放完"（playingTime = 总时长）。
    // 注意：实测服务端按真实观看时长累计，单次跳变通常不会通过（isPassed=false），
    // 需要真正计入进度时请用 --pace。
    let resp = video::report(&client.session, &make_report(duration, 0)).await?;
    let passed = resp
        .get("isPassed")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    println!("上报结果: {}", resp);
    if passed {
        println!("✓ 已标记为完成");
    } else {
        println!(
            "未通过（isPassed=false）：该视频按真实观看时长校验，单次上报不会计入；"
        );
        println!("可改用 `--pace 60` 按真实时间补报。");
    }

    Ok(())
}

async fn cmd_quiz(client: &mut ChaoxingClient, args: &[String]) -> Result<()> {
    client.ensure_login().await?;
    let course = resolve_course(client, args.first().map(|s| s.as_str())).await?;
    client.set_course(&course)?;
    let chapter = resolve_chapter(client, &course, args.get(1).map(|s| s.as_str())).await?;
    let tasks = client.fetch_tasks(&course, &chapter).await?;
    let task = resolve_task(&tasks, args.get(2).map(|s| s.as_str()))?;
    if task.kind != TaskKind::Quiz {
        bail!("任务 {} 不是测验（类型 {}）", task.id, task.kind.as_str());
    }
    let page = client.fetch_quiz_page(&course, &chapter, &task.id).await?;
    println!(
        "测验 {}：{} 题（{}）",
        task.title,
        page.questions.len(),
        if page.font_map.is_empty() {
            "无字体加密"
        } else {
            "已解密字体"
        }
    );
    for (i, q) in page.questions.iter().enumerate() {
        println!("\n{}. [{}] {}", i + 1, q.kind.as_str(), q.stem);
        for o in &q.options {
            println!("   {}. {}", o.label, o.text);
        }
    }

    // 答案来源：--answers 手工指定，或 --solve 由 LLM 作答
    let submit = args.iter().any(|s| s == "--submit");
    let solve = args.iter().any(|s| s == "--solve");
    let answers: Option<HashMap<String, String>> =
        if let Some(pos) = args.iter().position(|s| s == "--answers") {
            let raw = args
                .get(pos + 1)
                .context("--answers 需要内容，如 \"1:C,2:对\"")?;
            Some(parse_answers(&page, raw)?)
        } else if solve {
            println!("\n正在请求 LLM 作答…");
            Some(client.solve_quiz(&page).await?)
        } else {
            None
        };

    if let Some(answers) = answers {
        println!("\n答案：");
        for (i, q) in page.questions.iter().enumerate() {
            let v = answers.get(&q.qid).cloned().unwrap_or_default();
            let shown = match q.kind {
                QKind::Judge => {
                    if v == "true" {
                        "对".to_string()
                    } else {
                        "错".to_string()
                    }
                }
                _ => v,
            };
            println!("  {}. [{}] -> {}", i + 1, q.kind.as_str(), shown);
        }
        if !submit {
            println!("\n(未加 --submit，不提交)");
            return Ok(());
        }
        let resp = client.submit_quiz(&page, &answers).await?;
        println!("\n提交响应: {}", resp);
        match client.quiz_score(&resp).await {
            Ok(Some(score)) => println!("本次成绩: {} 分", score),
            Ok(None) => println!("未能从批阅页解析到分数"),
            Err(e) => println!("分数获取失败: {:#}", e),
        }
    }
    Ok(())
}

fn parse_answers(page: &quiz::QuizPage, raw: &str) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for item in raw.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let (idx, choice) = item
            .split_once(':')
            .or_else(|| item.split_once('：'))
            .context("答案格式应为 题号:选项，如 1:C")?;
        let idx: usize = idx.trim().parse().context("题号不合法")?;
        let q = page
            .questions
            .get(idx.saturating_sub(1))
            .context("题号超出范围")?;
        map.insert(q.qid.clone(), quiz::answer_value(q, choice));
    }
    Ok(map)
}

async fn resolve_course(client: &ChaoxingClient, arg: Option<&str>) -> Result<Course> {
    let list = client.fetch_courses().await?;
    match arg {
        None => {
            // 无参数：优先用 config 里记住的课程
            let saved = client.chaoxing().course_id.clone();
            list.iter()
                .find(|c| c.course_id == saved)
                .cloned()
                .or_else(|| list.first().cloned())
                .context("没有课程")
        }
        Some(a) => {
            if let Ok(idx) = a.parse::<usize>() {
                return list
                    .get(idx.saturating_sub(1))
                    .cloned()
                    .with_context(|| format!("课程序号超出范围（共 {} 门）", list.len()));
            }
            list.iter()
                .find(|c| c.course_id == a || c.name.contains(a))
                .cloned()
                .with_context(|| format!("未找到课程: {}", a))
        }
    }
}

async fn resolve_chapter(
    client: &ChaoxingClient,
    course: &Course,
    arg: Option<&str>,
) -> Result<crate::platform::types::Chapter> {
    let list = client.fetch_chapters(course).await?;
    match arg {
        None => list.first().cloned().context("没有章节"),
        Some(a) => {
            if let Ok(idx) = a.parse::<usize>() {
                return list
                    .get(idx.saturating_sub(1))
                    .cloned()
                    .with_context(|| format!("章节序号超出范围（共 {} 个）", list.len()));
            }
            list.iter()
                .find(|c| c.id == a)
                .cloned()
                .with_context(|| format!("未找到章节: {}", a))
        }
    }
}

fn resolve_task<'a>(tasks: &'a [TaskPoint], arg: Option<&str>) -> Result<&'a TaskPoint> {
    match arg {
        None => bail!("请指定任务点（用 tasks 子命令查看）"),
        Some(a) => {
            if let Ok(idx) = a.parse::<usize>() {
                return tasks
                    .get(idx.saturating_sub(1))
                    .with_context(|| format!("任务序号超出范围（共 {} 个）", tasks.len()));
            }
            tasks
                .iter()
                .find(|t| t.id == a)
                .with_context(|| format!("未找到任务: {}", a))
        }
    }
}

/// 供 `course::CardPage` 之外的打印使用（避免 dead_code 警告）。
#[allow(dead_code)]
fn _fmt_defaults(d: &course::MargDefaults) -> String {
    format!(
        "fid={} cfid={} userid={} cpi={} clazz={}",
        d.fid, d.cfid, d.userid, d.cpi, d.clazzid
    )
}
