//! TUI 的「学习通」标签页：独立状态、后台任务与渲染。
//!
//! 设计目标：与 U 校园标签页互不干扰——各自的请求、进度、日志、取消互不影响。

use crate::config::Config;
use crate::platform::chaoxing::{ChaoxingClient, video};
use crate::platform::types::{Chapter, Course, TaskKind, TaskPoint};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use std::collections::HashMap;
use std::collections::VecDeque;
use tokio_util::sync::CancellationToken;
use std::path::PathBuf;
use tokio::sync::mpsc::UnboundedSender;

use super::app::AppEvent;

/// 学习通账号标识（用于数据库按账号隔离）。
///
/// 用用户名哈希而不是明文手机号，避免把敏感信息写进库；哈希稳定，
/// 换设备/重启后仍是同一个账号键。
pub fn account(cfg: &Config) -> String {
    use md5::{Digest, Md5};
    let c = &cfg.chaoxing;
    if !c.username.is_empty() {
        let mut h = Md5::new();
        h.update(c.username.as_bytes());
        let hex = format!("{:x}", h.finalize());
        format!("u{}", &hex[..16])
    } else if !c.user_id.is_empty() {
        format!("uid{}", c.user_id)
    } else {
        "chaoxing".to_string()
    }
}

/// 学习通标签页状态。
pub struct CxState {
    pub loaded: bool,
    pub loading: bool,
    pub courses: Vec<Course>,
    pub course_idx: usize,
    pub chapters: Vec<Chapter>,
    pub chapter_idx: usize,
    pub tasks: Vec<TaskPoint>,
    /// tasks 对应的章节 id（切换章节后清空）。
    pub tasks_chapter: String,
    pub stage: CxStage,
    pub status: String,
    pub running: bool,
    pub logs: VecDeque<String>,
    pub last_score: Option<f64>,
    /// 测验结果（task_id -> 结果），用于任务列表显示分数。
    pub results: HashMap<String, crate::db::QuizResult>,
    /// 当前任务的取消令牌。
    pub cancel: Option<CancellationToken>,
    /// 鼠标命中区（每帧重建）。
    pub hot: Vec<(Rect, CxHit)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CxStage {
    Courses,
    Chapters,
    Tasks,
}

impl Default for CxState {
    fn default() -> Self {
        Self {
            loaded: false,
            loading: false,
            courses: Vec::new(),
            course_idx: 0,
            chapters: Vec::new(),
            chapter_idx: 0,
            tasks: Vec::new(),
            tasks_chapter: String::new(),
            stage: CxStage::Courses,
            status: "按 r 加载课程".to_string(),
            running: false,
            logs: VecDeque::new(),
            last_score: None,
            results: HashMap::new(),
            cancel: None,
            hot: Vec::new(),
        }
    }
}

impl CxState {
    pub fn current_course(&self) -> Option<&Course> {
        self.courses.get(self.course_idx)
    }

    pub fn current_chapter(&self) -> Option<&Chapter> {
        self.chapters.get(self.chapter_idx)
    }

    /// 从未完成任务里挑出测验数量与视频数量。
    pub fn unfinished_counts(&self) -> (usize, usize) {
        let quizzes = self
            .tasks
            .iter()
            .filter(|t| t.kind == TaskKind::Quiz && !t.finished)
            .count();
        let videos = self
            .tasks
            .iter()
            .filter(|t| t.kind == TaskKind::Video && !t.finished)
            .count();
        (quizzes, videos)
    }

    pub fn push_log(&mut self, s: impl Into<String>) {
        let s = s.into();
        self.logs.push_back(s);
        while self.logs.len() > 200 {
            self.logs.pop_front();
        }
    }

    /// 切到本标签页时自动加载课程（只加载一次）。
    pub fn ensure_loaded(&mut self, cfg: &Config, path: &std::path::Path, tx: &UnboundedSender<AppEvent>) {
        if self.loaded || self.loading {
            return;
        }
        self.loading = true;
        let (cfg, path, tx) = (cfg.clone(), path.to_path_buf(), tx.clone());
        tokio::spawn(async move {
            let res = load_courses_inner(cfg, path).await;
            let _ = tx.send(AppEvent::Cx(CxEvent::Courses(res.map_err(|e| format!("{e:#}")))));
        });
    }
}

/// 后台 → 学习通标签页事件。
#[derive(Debug)]
pub enum CxEvent {
    Courses(Result<Vec<Course>, String>),
    Chapters(Result<Vec<Chapter>, String>),
    Tasks(Result<Vec<TaskPoint>, String>),
    Log(Level, String),
    Progress { done: usize, total: usize, text: String },
    RunDone(String),
}

/// 鼠标命中项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CxHit {
    Stage(CxStage),
    Course(usize),
    Chapter(usize),
    Task(usize),
    Action(char),
}

#[derive(Debug, Clone, Copy)]
pub enum Level {
    Info,
    Warn,
    Error,
}

async fn load_courses_inner(cfg: Config, path: PathBuf) -> anyhow::Result<Vec<Course>> {
    let mut client = ChaoxingClient::new(cfg, path)?;
    client.ensure_login().await?;
    client.fetch_courses().await
}

pub fn spawn_load_courses(cfg: Config, path: PathBuf, tx: UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let res = load_courses_inner(cfg, path).await;
        let _ = tx.send(AppEvent::Cx(CxEvent::Courses(
            res.map_err(|e| format!("{e:#}")),
        )));
    });
}

pub fn spawn_load_chapters(
    cfg: Config,
    path: PathBuf,
    course: Course,
    tx: UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let res = async {
            let mut client = ChaoxingClient::new(cfg, path)?;
            client.ensure_login().await?;
            client.set_course(&course)?;
            client.fetch_chapters(&course).await
        }
        .await;
        let event = CxEvent::Chapters(res.map_err(|e| format!("{e:#}")));
        let _ = tx.send(AppEvent::Cx(event));
    });
}

pub fn spawn_load_tasks(
    cfg: Config,
    path: PathBuf,
    course: Course,
    chapter: Chapter,
    tx: UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let res = async {
            let mut client = ChaoxingClient::new(cfg, path)?;
            client.ensure_login().await?;
            client.set_course(&course)?;
            client.fetch_tasks(&course, &chapter).await
        }
        .await;
        let event = CxEvent::Tasks(res.map_err(|e| format!("{e:#}")));
        let _ = tx.send(AppEvent::Cx(event));
    });
}

/// 运行当前章节：逐题作答并提交（视频任务跳过，由 CLI 的 video 命令处理）。
/// 发送学习通事件。
fn cx_send(tx: impl std::borrow::Borrow<UnboundedSender<AppEvent>>, ev: CxEvent) {
    let _ = tx.borrow().send(AppEvent::Cx(ev));
}

/// 取消感知的 sleep：返回 true 表示已取消。
async fn sleep_or_cancel(cancel: &CancellationToken, secs: u64) -> bool {
    if secs == 0 {
        return cancel.is_cancelled();
    }
    tokio::select! {
        _ = cancel.cancelled() => true,
        _ = tokio::time::sleep(std::time::Duration::from_secs(secs)) => false,
    }
}

/// 完成一个视频任务（按真实节奏补报）。
async fn finish_one_video(
    client: &ChaoxingClient,
    course: &Course,
    task: &TaskPoint,
    defaults: &crate::platform::chaoxing::course::MargDefaults,
    step: u64,
    cancel: &CancellationToken,
    tx: &UnboundedSender<AppEvent>,
) -> anyhow::Result<bool> {
    let object_id = task.extra_str("objectId");
    let status = video::fetch_status(&client.session, &object_id, &defaults.cfid).await?;
    let duration = if status.duration > 0 {
        status.duration
    } else {
        task.extra_i64("attDuration").unwrap_or(0).max(0) as u64
    };
    if duration == 0 {
        cx_send(tx, CxEvent::Log(
            Level::Warn,
            format!("{}：无法确定时长，跳过", task.title),
        ));
        return Ok(false);
    }
    let report = video::Report {
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
        playing_time: 0,
        clip_time: format!("0_{}", duration),
        isdrag: 0,
    };
    let mut playing = step.min(duration);
    loop {
        if cancel.is_cancelled() {
            return Ok(false);
        }
        let mut r = report.clone();
        r.playing_time = playing;
        // dtoken 轮换，必须每次重新取，否则会被判重放（403）
        let resp = video::report(&client.session, &r).await?;
        let ok = resp.get("isPassed").and_then(|x| x.as_bool()).unwrap_or(false);
        cx_send(tx, CxEvent::Log(
            Level::Info,
            format!("{}：上报 {}s / {}s", task.title, playing, duration),
        ));
        if ok {
            return Ok(true);
        }
        if playing >= duration {
            return Ok(false);
        }
        if sleep_or_cancel(cancel, step).await {
            return Ok(false);
        }
        playing = (playing + step).min(duration);
    }
}

/// 作答并提交一个测验任务；返回得分。
async fn submit_one_quiz(
    client: &mut ChaoxingClient,
    course: &Course,
    chapter: &Chapter,
    task: &TaskPoint,
    account: &str,
    cancel: &CancellationToken,
    tx: &UnboundedSender<AppEvent>,
) -> anyhow::Result<Option<f64>> {
    if cancel.is_cancelled() {
        anyhow::bail!("已取消");
    }
    cx_send(tx, CxEvent::Log(
        Level::Info,
        format!("抓取测验「{}」", task.title),
    ));
    let page = client.fetch_quiz_page(course, chapter, &task.id).await?;
    cx_send(tx, CxEvent::Log(
        Level::Info,
        format!(
            "{}：{} 题（{}）",
            task.title,
            page.questions.len(),
            if page.font_map.is_empty() {
                "明文"
            } else {
                "已解密"
            }
        ),
    ));
    let answers = client.solve_quiz(&page).await?;
    let resp = client.submit_quiz(&page, &answers).await?;
    let score = client.quiz_score(&resp).await.ok().flatten();
    if let Ok(d) = crate::db::Db::open(crate::db::SITE_CHAOXING, account) {
        let _ = d.save_quiz_result(
            &course.course_id,
            &chapter.id,
            &task.id,
            &task.title,
            score,
            page.questions.len(),
            answers.len(),
        );
    }
    cx_send(tx, CxEvent::Log(
        Level::Info,
        match score {
            Some(s) => format!("提交成功：{} -> {} 分", task.title, s),
            None => format!("提交成功但未取到分数：{}", task.title),
        },
    ));
    Ok(score)
}

/// 运行当前章节：逐题作答并提交（视频任务跳过，由 v 处理）。
pub fn spawn_run_chapter(
    cfg: Config,
    path: PathBuf,
    course: Course,
    chapter: Chapter,
    account: String,
    cancel: CancellationToken,
    tx: UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let send = |ev: CxEvent| {
            let _ = tx.send(AppEvent::Cx(ev));
        };
        let run = async {
            let mut client = ChaoxingClient::new(cfg, path)?;
            client.ensure_login().await?;
            client.set_course(&course)?;
            let tasks = client.fetch_tasks(&course, &chapter).await?;
            let quizzes: Vec<TaskPoint> = tasks
                .iter()
                .filter(|t| t.kind == TaskKind::Quiz && !t.finished)
                .cloned()
                .collect();
            if quizzes.is_empty() {
                let total_quiz = tasks.iter().filter(|t| t.kind == TaskKind::Quiz).count();
                if total_quiz > 0 {
                    send(CxEvent::RunDone(format!(
                        "本章 {} 个测验均已完成，跳过（避免重复提交）",
                        total_quiz
                    )));
                } else {
                    send(CxEvent::RunDone("本任务点没有测验（视频请按 v）".into()));
                }
                return anyhow::Ok(());
            }
            let total = quizzes.len();
            let mut ok = 0usize;
            let mut sum = 0.0f64;
            for (i, task) in quizzes.iter().enumerate() {
                if cancel.is_cancelled() {
                    send(CxEvent::RunDone("已取消".into()));
                    return anyhow::Ok(());
                }
                cx_send(&tx, CxEvent::Progress {
                    done: i,
                    total,
                    text: format!("测验「{}」", task.title),
                });
                match submit_one_quiz(&mut client, &course, &chapter, task, &account, &cancel, &tx)
                    .await
                {
                    Ok(score) => {
                        if let Some(s) = score {
                            sum += s;
                            ok += 1;
                        }
                    }
                    Err(e) => cx_send(&tx, CxEvent::Log(
                        Level::Error,
                        format!("{} 提交失败：{:#}", task.title, e),
                    )),
                }
                if i + 1 < total && sleep_or_cancel(&cancel, 4).await {
                    send(CxEvent::RunDone("已取消".into()));
                    return anyhow::Ok(());
                }
            }
            let avg = if ok > 0 { sum / ok as f64 } else { 0.0 };
            send(CxEvent::RunDone(format!(
                "完成：{}/{} 个测验，平均 {} 分",
                ok, total, avg
            )));
            anyhow::Ok(())
        }
        .await;
        if let Err(e) = run {
            send(CxEvent::RunDone(format!("运行失败：{e:#}")));
        }
    });
}

/// 运行当前章节里未完成的视频任务。
pub fn spawn_run_videos(
    cfg: Config,
    path: PathBuf,
    course: Course,
    chapter: Chapter,
    step: u64,
    cancel: CancellationToken,
    tx: UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let send = |ev: CxEvent| {
            let _ = tx.send(AppEvent::Cx(ev));
        };
        let run = async {
            let mut client = ChaoxingClient::new(cfg, path)?;
            client.ensure_login().await?;
            client.set_course(&course)?;
            let tasks = client.fetch_tasks(&course, &chapter).await?;
            let videos: Vec<TaskPoint> = tasks
                .iter()
                .filter(|t| t.kind == TaskKind::Video && !t.finished)
                .cloned()
                .collect();
            if videos.is_empty() {
                send(CxEvent::RunDone("本章没有待完成的视频".into()));
                return anyhow::Ok(());
            }
            let cards = client.fetch_cards(&course, &chapter).await?;
            let defaults = cards.first().map(|p| p.defaults.clone()).unwrap_or_default();
            let total = videos.len();
            let mut done = 0usize;
            for (i, task) in videos.iter().enumerate() {
                if cancel.is_cancelled() {
                    send(CxEvent::RunDone("已取消".into()));
                    return anyhow::Ok(());
                }
                cx_send(&tx, CxEvent::Progress {
                    done: i,
                    total,
                    text: format!("视频「{}」", task.title),
                });
                match finish_one_video(&client, &course, task, &defaults, step, &cancel, &tx).await {
                    Ok(true) => {
                        done += 1;
                        cx_send(&tx, CxEvent::Log(Level::Info, format!("{}：已完成", task.title)));
                    }
                    Ok(false) => cx_send(&tx, CxEvent::Log(
                        Level::Warn,
                        format!("{}：未通过（可能仍需真实观看时长）", task.title),
                    )),
                    Err(e) => cx_send(&tx, CxEvent::Log(
                        Level::Error,
                        format!("{} 上报失败：{:#}", task.title, e),
                    )),
                }
            }
            send(CxEvent::RunDone(format!("视频完成 {}/{}", done, total)));
            anyhow::Ok(())
        }
        .await;
        if let Err(e) = run {
            send(CxEvent::RunDone(format!("运行失败：{e:#}")));
        }
    });
}

/// 全自动：按章节顺序跑完整个课程（视频 + 测验）。
pub fn spawn_run_course(
    cfg: Config,
    path: PathBuf,
    course: Course,
    step: u64,
    account: String,
    cancel: CancellationToken,
    tx: UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let send = |ev: CxEvent| {
            let _ = tx.send(AppEvent::Cx(ev));
        };
        let run = async {
            let mut client = ChaoxingClient::new(cfg, path)?;
            client.ensure_login().await?;
            client.set_course(&course)?;
            let chapters = client.fetch_chapters(&course).await?;
            let total = chapters.len();
            if total == 0 {
                send(CxEvent::RunDone("没有章节".into()));
                return anyhow::Ok(());
            }
            cx_send(&tx, CxEvent::Log(
                Level::Info,
                format!("全自动开始：{} 个章节", total),
            ));
            let mut quiz_ok = 0usize;
            let mut video_ok = 0usize;
            let mut failed = 0usize;
            for (ci, chapter) in chapters.iter().enumerate() {
                if cancel.is_cancelled() {
                    send(CxEvent::RunDone("已取消".into()));
                    return anyhow::Ok(());
                }
                let label = format!("{}{}", chapter.number, chapter.title);
                cx_send(&tx, CxEvent::Progress {
                    done: ci,
                    total,
                    text: format!("{}/{} 章节 {}", ci + 1, total, label),
                });
                cx_send(&tx, CxEvent::Log(
                    Level::Info,
                    format!("── 章节 {}/{}：{} ──", ci + 1, total, label),
                ));
                let tasks = match client.fetch_tasks(&course, chapter).await {
                    Ok(t) => t,
                    Err(e) => {
                        failed += 1;
                        cx_send(&tx, CxEvent::Log(
                            Level::Error,
                            format!("{} 任务点获取失败：{:#}", label, e),
                        ));
                        continue;
                    }
                };
                let videos: Vec<TaskPoint> = tasks
                    .iter()
                    .filter(|t| t.kind == TaskKind::Video && !t.finished)
                    .cloned()
                    .collect();
                let quizzes: Vec<TaskPoint> = tasks
                    .iter()
                    .filter(|t| t.kind == TaskKind::Quiz && !t.finished)
                    .cloned()
                    .collect();
                if videos.is_empty() && quizzes.is_empty() {
                    cx_send(&tx, CxEvent::Log(
                        Level::Info,
                        format!("{}：无待完成任务", label),
                    ));
                    continue;
                }
                // 视频
                if !videos.is_empty() {
                    let cards = match client.fetch_cards(&course, chapter).await {
                        Ok(c) => c,
                        Err(e) => {
                            failed += 1;
                            cx_send(&tx, CxEvent::Log(
                                Level::Error,
                                format!("{} 视频参数获取失败：{:#}", label, e),
                            ));
                            Vec::new()
                        }
                    };
                    let defaults = cards.first().map(|p| p.defaults.clone()).unwrap_or_default();
                    for task in &videos {
                        if cancel.is_cancelled() {
                            send(CxEvent::RunDone("已取消".into()));
                            return anyhow::Ok(());
                        }
                        cx_send(&tx, CxEvent::Log(
                            Level::Info,
                            format!("视频：{}", task.title),
                        ));
                        match finish_one_video(&client, &course, task, &defaults, step, &cancel, &tx)
                            .await
                        {
                            Ok(true) => {
                                video_ok += 1;
                                cx_send(&tx, CxEvent::Log(
                                    Level::Info,
                                    format!("{}：已完成", task.title),
                                ));
                            }
                            Ok(false) => cx_send(&tx, CxEvent::Log(
                                Level::Warn,
                                format!("{}：未通过（可能仍需真实观看时长）", task.title),
                            )),
                            Err(e) => {
                                failed += 1;
                                cx_send(&tx, CxEvent::Log(
                                    Level::Error,
                                    format!("{} 上报失败：{:#}", task.title, e),
                                ));
                            }
                        }
                    }
                }
                // 测验
                for task in &quizzes {
                    if cancel.is_cancelled() {
                        send(CxEvent::RunDone("已取消".into()));
                        return anyhow::Ok(());
                    }
                    match submit_one_quiz(
                        &mut client,
                        &course,
                        chapter,
                        task,
                        &account,
                        &cancel,
                        &tx,
                    )
                    .await
                    {
                        Ok(score) => {
                            if score.is_some() {
                                quiz_ok += 1;
                            }
                        }
                        Err(e) => {
                            failed += 1;
                            cx_send(&tx, CxEvent::Log(
                                Level::Error,
                                format!("{} 提交失败：{:#}", task.title, e),
                            ));
                        }
                    }
                    if sleep_or_cancel(&cancel, 4).await {
                        send(CxEvent::RunDone("已取消".into()));
                        return anyhow::Ok(());
                    }
                }
            }
            send(CxEvent::RunDone(format!(
                "全自动完成：测验 {} 个、视频 {} 个、失败 {} 项",
                quiz_ok, video_ok, failed
            )));
            anyhow::Ok(())
        }
        .await;
        if let Err(e) = run {
            send(CxEvent::RunDone(format!("运行失败：{e:#}")));
        }
    });
}

/// 渲染学习通标签页。
pub fn render(f: &mut Frame, area: Rect, cx: &mut CxState) {
    cx.hot.clear();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(8),
            Constraint::Length(1),
        ])
        .split(area);
    // 顶部状态
    let course = cx
        .current_course()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "（未加载）".into());
    let chapter = cx
        .current_chapter()
        .map(|c| format!("{}{}", c.number, c.title))
        .unwrap_or_else(|| "（未选择）".into());
    let head = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                "学习通 ",
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "课程 {}/{}  {}",
                cx.course_idx + 1,
                cx.courses.len(),
                course
            )),
            Span::raw("   "),
            Span::raw(format!(
                "章节 {}/{}  {}",
                cx.chapter_idx + 1,
                cx.chapters.len(),
                chapter
            )),
        ]),
        Line::from(vec![
            Span::styled("状态: ", Style::default().fg(Color::Yellow)),
            Span::raw(cx.status.clone()),
            if cx.running {
                Span::styled("  [运行中]", Style::default().fg(Color::Green))
            } else {
                Span::raw("")
            },
        ]),
    ])
    .block(Block::default().borders(Borders::ALL).title("学习通"));
    f.render_widget(head, chunks[0]);

    // 中部：课程 / 章节 双列
    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[1]);
    let stage = cx.stage;
    let courses: Vec<ListItem> = cx
        .courses
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let sel = i == cx.course_idx && matches!(stage, CxStage::Courses);
            ListItem::new(format!("{}{}", if sel { "▶ " } else { "  " }, c.name))
        })
        .collect();
    f.render_widget(
        List::new(courses).block(
            Block::default()
                .borders(Borders::ALL)
                .title("课程 (j/k 选择, Enter)"),
        ),
        mid[0],
    );
    register_rows(&mut cx.hot, mid[0], cx.courses.len(), CxHit::Course);

    let chapters: Vec<ListItem> = cx
        .chapters
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let sel = i == cx.chapter_idx && matches!(stage, CxStage::Chapters);
            ListItem::new(format!(
                "{}{}{} {}",
                if sel { "▶ " } else { "  " },
                c.number,
                c.title,
                if c.completed { " ✓" } else { "" }
            ))
        })
        .collect();
    f.render_widget(
        List::new(chapters).block(
            Block::default()
                .borders(Borders::ALL)
                .title("章节 (j/k 选择, Enter 加载, a 测验, v 视频, A 全自动, Esc 取消)"),
        ),
        mid[1],
    );
    register_rows(&mut cx.hot, mid[1], cx.chapters.len(), CxHit::Chapter);

    // 底部：任务 + 日志
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(chunks[2]);
    let tasks: Vec<ListItem> = cx
        .tasks
        .iter()
        .map(|t| {
            let score = cx
                .results
                .get(&t.id)
                .and_then(|r| r.score)
                .map(|s| format!("  {s}分"))
                .unwrap_or_default();
            ListItem::new(format!(
                "[{}] {}{}{}",
                t.kind.as_str(),
                t.title,
                score,
                if t.finished { " ✓" } else { "" }
            ))
        })
        .collect();
    f.render_widget(
        List::new(tasks).block(Block::default().borders(Borders::ALL).title("任务点 (t 刷新)")),
        bottom[0],
    );
    register_rows(&mut cx.hot, bottom[0], cx.tasks.len(), CxHit::Task);
    let logs: Vec<Line> = cx
        .logs
        .iter()
        .rev()
        .take(7)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|s| Line::from(s.clone()))
        .collect();
    f.render_widget(
        Paragraph::new(logs)
            .block(Block::default().borders(Borders::ALL).title("日志"))
            .wrap(Wrap { trim: false }),
        bottom[1],
    );

    // 底栏：按键提示（可点击）
    render_chips(
        f,
        chunks[3],
        &mut cx.hot,
        &[
            ('a', "测验"),
            ('v', "视频"),
            ('A', "全自动"),
            ('t', "刷新"),
            ('x', "取消"),
        ],
    );
}

fn register_rows(hot: &mut Vec<(Rect, CxHit)>, area: Rect, n: usize, f: impl Fn(usize) -> CxHit) {
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    for i in 0..n {
        if i as u16 >= inner.height {
            break;
        }
        hot.push((
            Rect {
                x: inner.x,
                y: inner.y + i as u16,
                width: inner.width,
                height: 1,
            },
            f(i),
        ));
    }
}

fn char_width(c: char) -> u16 {
    if c.is_ascii() { 1 } else { 2 }
}

fn text_width(s: &str) -> u16 {
    s.chars().map(char_width).sum()
}

/// 渲染底栏按键提示，并把每段注册为可点击区域。
fn render_chips(f: &mut Frame, area: Rect, hot: &mut Vec<(Rect, CxHit)>, chips: &[(char, &str)]) {
    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;
    for (key, label) in chips {
        let text = format!("[{}] {}", key, label);
        let w = text_width(&text);
        if x + w > area.x + area.width {
            break;
        }
        spans.push(Span::styled(
            text,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        hot.push((
            Rect {
                x,
                y: area.y,
                width: w,
                height: 1,
            },
            CxHit::Action(*key),
        ));
        x += w + 1;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn hit(cx: &CxState, x: u16, y: u16) -> Option<CxHit> {
    cx.hot
        .iter()
        .rev()
        .find(|(r, _)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
        .map(|(_, h)| *h)
}

/// 处理学习通标签页的鼠标事件；返回是否已消费。
pub fn handle_mouse(
    cx: &mut CxState,
    m: &crossterm::event::MouseEvent,
    cfg: &Config,
    path: &std::path::Path,
    tx: &UnboundedSender<AppEvent>,
) -> bool {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let Some(h) = hit(cx, m.column, m.row) else {
                return false;
            };
            match h {
                CxHit::Course(i) => {
                    cx.course_idx = i;
                    cx.stage = CxStage::Courses;
                    cx.tasks.clear();
                    cx.status = "已选择课程，Enter 加载章节".into();
                }
                CxHit::Chapter(i) => {
                    cx.chapter_idx = i;
                    cx.stage = CxStage::Chapters;
                }
                CxHit::Stage(st) => {
                    cx.stage = st;
                }
                CxHit::Task(_) => {}
                CxHit::Action(c) => {
                    let code = if c == 'x' {
                        KeyCode::Esc
                    } else {
                        KeyCode::Char(c)
                    };
                    let k = KeyEvent::new(code, KeyModifiers::NONE);
                    handle_key(cx, k, cfg, path, tx);
                }
            }
            true
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = matches!(m.kind, MouseEventKind::ScrollUp);
            match cx.stage {
                CxStage::Courses => {
                    if up {
                        cx.course_idx = cx.course_idx.saturating_sub(1);
                    } else if cx.course_idx + 1 < cx.courses.len() {
                        cx.course_idx += 1;
                    }
                }
                CxStage::Chapters => {
                    if up {
                        cx.chapter_idx = cx.chapter_idx.saturating_sub(1);
                    } else if cx.chapter_idx + 1 < cx.chapters.len() {
                        cx.chapter_idx += 1;
                    }
                }
                CxStage::Tasks => {}
            }
            true
        }
        _ => false,
    }
}

fn load_results(
    account: Option<&str>,
    course: Option<&Course>,
) -> HashMap<String, crate::db::QuizResult> {
    let (Some(account), Some(course)) = (account, course) else {
        return HashMap::new();
    };
    let Ok(d) = crate::db::Db::open(crate::db::SITE_CHAOXING, account) else {
        return HashMap::new();
    };
    d.load_quiz_results(&course.course_id)
        .map(|v| v.into_iter().map(|r| (r.task_id.clone(), r)).collect())
        .unwrap_or_default()
}

/// 处理学习通标签页的按键；返回是否已消费。
pub fn handle_key(
    cx: &mut CxState,
    k: crossterm::event::KeyEvent,
    cfg: &Config,
    path: &std::path::Path,
    tx: &UnboundedSender<AppEvent>,
) -> bool {
    use crossterm::event::KeyCode;
    if cx.running {
        if k.code == crossterm::event::KeyCode::Esc {
            if let Some(token) = &cx.cancel {
                token.cancel();
            }
            cx.status = "正在取消…".into();
        }
        return true;
    }
    match k.code {
        KeyCode::Char('r') => {
            cx.loaded = false;
            cx.loading = false;
            cx.status = "重新加载课程…".into();
            cx.ensure_loaded(cfg, path, tx);
        }
        KeyCode::Char('j') | KeyCode::Down => match cx.stage {
            CxStage::Courses => {
                if !cx.courses.is_empty() {
                    cx.course_idx = (cx.course_idx + 1) % cx.courses.len();
                }
            }
            _ => {
                if !cx.chapters.is_empty() {
                    cx.chapter_idx = (cx.chapter_idx + 1) % cx.chapters.len();
                    cx.tasks.clear();
                }
            }
        },
        KeyCode::Char('k') | KeyCode::Up => match cx.stage {
            CxStage::Courses => {
                if !cx.courses.is_empty() {
                    cx.course_idx = cx.course_idx.saturating_sub(1);
                }
            }
            _ => {
                if !cx.chapters.is_empty() {
                    cx.chapter_idx = cx.chapter_idx.saturating_sub(1);
                    cx.tasks.clear();
                }
            }
        },
        KeyCode::Enter => match cx.stage {
            CxStage::Courses => {
                if let Some(course) = cx.current_course().cloned() {
                    cx.status = format!("加载「{}」章节…", course.name);
                    spawn_load_chapters(cfg.clone(), path.to_path_buf(), course, tx.clone());
                }
            }
            CxStage::Chapters => {
                if let (Some(course), Some(chapter)) =
                    (cx.current_course().cloned(), cx.current_chapter().cloned())
                {
                    cx.status = format!("加载 {}{} 任务点…", chapter.number, chapter.title);
                    let key = chapter.id.clone();
                    spawn_load_tasks(cfg.clone(), path.to_path_buf(), course, chapter, tx.clone());
                    cx.tasks_chapter = key;
                }
            }
            CxStage::Tasks => {}
        },
        KeyCode::Char('A') => {
            if let Some(course) = cx.current_course().cloned() {
                cx.running = true;
                cx.status = "全自动运行中（按章节测验+视频）…".into();
                cx.push_log("开始全自动：按章节跑完整个课程（Esc 取消）");
                let token = CancellationToken::new();
                cx.cancel = Some(token.clone());
                spawn_run_course(
                    cfg.clone(),
                    path.to_path_buf(),
                    course,
                    60,
                    account(cfg),
                    token,
                    tx.clone(),
                );
            }
        }
        KeyCode::Char('a') => {
            if let (Some(course), Some(chapter)) =
                (cx.current_course().cloned(), cx.current_chapter().cloned())
            {
                cx.running = true;
                cx.status = "作答测验中…".into();
                cx.push_log("开始作答当前章节测验");
                let token = CancellationToken::new();
                cx.cancel = Some(token.clone());
                spawn_run_chapter(
                    cfg.clone(),
                    path.to_path_buf(),
                    course,
                    chapter,
                    account(cfg),
                    token,
                    tx.clone(),
                );
            }
        }
        KeyCode::Char('v') => {
            if let (Some(course), Some(chapter)) =
                (cx.current_course().cloned(), cx.current_chapter().cloned())
            {
                cx.running = true;
                cx.status = "视频补报中（每 60s 一次）…".into();
                cx.push_log("开始补报当前章节视频（Esc 取消）");
                let token = CancellationToken::new();
                cx.cancel = Some(token.clone());
                spawn_run_videos(
                    cfg.clone(),
                    path.to_path_buf(),
                    course,
                    chapter,
                    60,
                    token,
                    tx.clone(),
                );
            }
        }
        KeyCode::Char('t') => {
            if let (Some(course), Some(chapter)) =
                (cx.current_course().cloned(), cx.current_chapter().cloned())
            {
                cx.status = "刷新任务点…".into();
                spawn_load_tasks(cfg.clone(), path.to_path_buf(), course, chapter, tx.clone());
            }
        }
        KeyCode::Esc => {
            cx.stage = CxStage::Courses;
            cx.chapters.clear();
            cx.tasks.clear();
        }
        _ => return false,
    }
    true
}

/// 应用后台事件；返回是否需要重绘。
pub fn apply(cx: &mut CxState, ev: CxEvent, account: Option<&str>) {
    match ev {
        CxEvent::Courses(Ok(list)) => {
            cx.loading = false;
            cx.loaded = true;
            cx.courses = list;
            cx.course_idx = 0;
            cx.stage = CxStage::Courses;
            cx.status = if cx.courses.is_empty() {
                "没有课程".into()
            } else {
                format!("已加载 {} 门课程", cx.courses.len())
            };
        }
        CxEvent::Courses(Err(e)) => {
            cx.loading = false;
            cx.status = if e.contains("401") || e.contains("登录") {
                format!("登录已失效：{e}（运行 `chaoxing login` 重新登录）")
            } else {
                format!("加载课程失败：{e}")
            };
            log::error!("[学习通] {}", cx.status);
        }
        CxEvent::Chapters(Ok(list)) => {
            cx.chapters = list;
            cx.chapter_idx = 0;
            cx.tasks.clear();
            cx.stage = CxStage::Chapters;
            cx.status = format!("已加载 {} 个章节", cx.chapters.len());
        }
        CxEvent::Chapters(Err(e)) => {
            cx.status = format!("加载章节失败：{e}");
            log::error!("[学习通] {}", cx.status);
        }
        CxEvent::Tasks(Ok(list)) => {
            cx.tasks = list;
            cx.stage = CxStage::Tasks;
            cx.results = load_results(account, cx.current_course());
            cx.status = format!("已加载 {} 个任务点", cx.tasks.len());
        }
        CxEvent::Tasks(Err(e)) => {
            cx.status = format!("加载任务点失败：{e}");
            log::error!("[学习通] {}", cx.status);
        }
        CxEvent::Log(level, msg) => {
            match level {
                Level::Info => log::info!("[学习通] {}", msg),
                Level::Warn => log::warn!("[学习通] {}", msg),
                Level::Error => log::error!("[学习通] {}", msg),
            }
            let tag = match level {
                Level::Info => "INFO",
                Level::Warn => "WARN",
                Level::Error => "ERR ",
            };
            cx.push_log(format!("[{tag}] {msg}"));
        }
        CxEvent::Progress { done, total, text } => {
            cx.status = format!("{}/{} {}", done, total, text);
        }
        CxEvent::RunDone(msg) => {
            log::info!("[学习通] {}", msg);
            cx.running = false;
            cx.cancel = None;
            cx.status = msg.clone();
            cx.push_log(msg);
            // 运行结束后刷新结果
            if let Some(account) = account {
                cx.results = load_results(Some(account), cx.current_course());
            }
        }
    }
}

/// 供 UI 计算得分（预留）。
#[allow(dead_code)]
fn _unused() -> HashMap<String, String> {
    HashMap::new()
}
