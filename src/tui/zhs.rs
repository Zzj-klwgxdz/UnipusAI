//! TUI 的「知到 / 智慧树」标签页：章节、作业与视频学时。
//!
//! 与其它标签页互不干扰；作业作答复用 `platform::zhihuishu::solve`。

use super::app::AppEvent;
use crate::config::Config;
use crate::platform::zhihuishu::solve;
use crate::platform::zhihuishu::{ZsAnswer, ZsChapter, ZsHomework, ZhihuishuClient};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use std::collections::VecDeque;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

/// 知到账号标识（库内按账号隔离）。
pub fn account(cfg: &Config) -> String {
    let z = &cfg.zhihuishu;
    if !z.uuid.is_empty() {
        format!("z{}", z.uuid)
    } else {
        format!("z{}-{}", z.recruit_id, z.course_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZsFocus {
    Chapters,
    Homeworks,
}

pub struct ZsState {
    pub loaded: bool,
    pub loading: bool,
    pub chapters: Vec<ZsChapter>,
    pub homeworks: Vec<ZsHomework>,
    pub submitted: Vec<ZsHomework>,
    pub show_submitted: bool,
    pub chapter_idx: usize,
    pub hw_idx: usize,
    pub focus: ZsFocus,
    pub status: String,
    pub running: bool,
    pub logs: VecDeque<String>,
    pub last_score: Option<String>,
    pub cancel: Option<CancellationToken>,
    /// 鼠标命中区（每帧重建）。
    pub hot: Vec<(Rect, ZsHit)>,
}

/// 鼠标命中项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZsHit {
    Focus(ZsFocus),
    Chapter(usize),
    Hw(usize),
    Action(char),
}

impl Default for ZsState {
    fn default() -> Self {
        Self {
            loaded: false,
            loading: false,
            chapters: Vec::new(),
            homeworks: Vec::new(),
            submitted: Vec::new(),
            show_submitted: false,
            chapter_idx: 0,
            hw_idx: 0,
            focus: ZsFocus::Chapters,
            status: "按 r 加载知到数据".to_string(),
            running: false,
            logs: VecDeque::new(),
            last_score: None,
            cancel: None,
            hot: Vec::new(),
        }
    }
}

impl ZsState {
    pub fn push_log(&mut self, s: impl Into<String>) {
        self.logs.push_back(s.into());
        while self.logs.len() > 300 {
            self.logs.pop_front();
        }
    }

    pub fn current_chapter(&self) -> Option<&ZsChapter> {
        self.chapters.get(self.chapter_idx)
    }

    pub fn current_homework(&self) -> Option<&ZsHomework> {
        if self.show_submitted {
            self.submitted.get(self.hw_idx)
        } else {
            self.homeworks.get(self.hw_idx)
        }
    }
}

/// (章节, 未提交作业, 已提交作业)
pub type ZsData = (Vec<ZsChapter>, Vec<ZsHomework>, Vec<ZsHomework>);

#[derive(Debug)]
pub enum ZsEvent {
    Data(Result<ZsData, String>),
    Log(Level, String),
    Progress { done: usize, total: usize, text: String },
    Score(String),
    Done(String),
}

fn cx_send(tx: &UnboundedSender<AppEvent>, ev: ZsEvent) {
    let _ = tx.send(AppEvent::Zs(ev));
}

pub fn spawn_load(cfg: Config, tx: UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let res: anyhow::Result<ZsData> = async {
            let client = ZhihuishuClient::new(&cfg.zhihuishu, cfg.timeout)?;
            let chapters = client.fetch_chapters().await?;
            let hw = client.fetch_homeworks(1).await?;
            let sub = client.fetch_homeworks(2).await?;
            Ok::<_, anyhow::Error>((chapters, hw, sub))
        }
        .await;
        cx_send(&tx, ZsEvent::Data(res.map_err(|e| format!("{e:#}"))));
    });
}

/// 作答一份作业：LLM 逐题 -> 保存 -> 交卷。
pub fn spawn_answer(
    cfg: Config,
    hw: ZsHomework,
    cancel: CancellationToken,
    tx: UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        if !crate::platform::zhihuishu::HOMEWORK_SUBMIT_ENABLED {
            cx_send(
                &tx,
                ZsEvent::Done(crate::platform::zhihuishu::HOMEWORK_SUBMIT_DISABLED_MSG.into()),
            );
            return;
        }
        let run = async {
            let client = ZhihuishuClient::new(&cfg.zhihuishu, cfg.timeout)?;
            cx_send(
                &tx,
                ZsEvent::Log(Level::Info, format!("取卷：{}", hw.name)),
            );
            let paper = client.fetch_paper(&hw).await?;
            cx_send(
                &tx,
                ZsEvent::Log(
                    Level::Info,
                    format!("{}：{} 题", paper.name, paper.questions.len()),
                ),
            );
            let total = paper.questions.len();
            let mut answers: Vec<ZsAnswer> = Vec::new();
            for (i, q) in paper.questions.iter().enumerate() {
                if cancel.is_cancelled() {
                    cx_send(&tx, ZsEvent::Done("已取消".into()));
                    return anyhow::Ok(());
                }
                cx_send(
                    &tx,
                    ZsEvent::Progress {
                        done: i,
                        total,
                        text: format!("作答 {}/{}", i + 1, total),
                    },
                );
                let ids = match solve::solve_question(&cfg, q).await {
                    Ok(v) => v,
                    Err(e) => {
                        cx_send(
                            &tx,
                            ZsEvent::Log(
                                Level::Error,
                                format!("第 {} 题作答失败：{:#}", i + 1, e),
                            ),
                        );
                        continue;
                    }
                };
                let text = solve::answer_text(q, &ids);
                cx_send(
                    &tx,
                    ZsEvent::Log(Level::Info, format!("{}. {} -> {}", i + 1, q.text, text)),
                );
                answers.push(ZsAnswer {
                    eid: q.eid.clone(),
                    type_id: q.type_id,
                    option_ids: ids,
                });
            }
            if cancel.is_cancelled() {
                cx_send(&tx, ZsEvent::Done("已取消".into()));
                return anyhow::Ok(());
            }
            let mut saved = 0usize;
            for a in &answers {
                match client.save_answer(&hw, a).await {
                    Ok(()) => saved += 1,
                    Err(e) => {
                        let msg = format!("{:#}", e);
                        cx_send(&tx, ZsEvent::Log(Level::Error, format!("保存失败：{msg}")));
                        if msg.contains("滑块") {
                            cx_send(
                                &tx,
                                ZsEvent::Log(
                                    Level::Warn,
                                    "服务端要求滑块验证：请打开考试页手动完成验证后再重试".into(),
                                ),
                            );
                        }
                    }
                }
            }
            cx_send(
                &tx,
                ZsEvent::Log(Level::Info, format!("已保存 {}/{} 题", saved, answers.len())),
            );
            if saved == 0 && !answers.is_empty() {
                cx_send(
                    &tx,
                    ZsEvent::Done(
                        "保存全部失败（服务端要求滑块验证），已阻止 0 分交卷；请先在考试页完成滑块验证"
                            .into(),
                    ),
                );
                return anyhow::Ok(());
            }
            let score = client.submit(&hw, answers.len()).await?;
            if let Some(s) = &score {
                cx_send(&tx, ZsEvent::Score(s.clone()));
            }
            cx_send(
                &tx,
                ZsEvent::Done(match score {
                    Some(s) => format!("交卷完成：{} 分", s),
                    None => "交卷完成（未返回分数）".into(),
                }),
            );
            anyhow::Ok(())
        }
        .await;
        if let Err(e) = run {
            cx_send(&tx, ZsEvent::Done(format!("运行失败：{e:#}")));
        }
    });
}

/// 顺序补报章节内视频（一次报完，节间 2 秒）。
pub fn spawn_videos(
    cfg: Config,
    chapters: Vec<ZsChapter>,
    only: Option<usize>,
    cancel: CancellationToken,
    tx: UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        if !crate::platform::zhihuishu::HOMEWORK_SUBMIT_ENABLED {
            cx_send(
                &tx,
                ZsEvent::Done(crate::platform::zhihuishu::HOMEWORK_SUBMIT_DISABLED_MSG.into()),
            );
            return;
        }
        let run = async {
            let client = ZhihuishuClient::new(&cfg.zhihuishu, cfg.timeout)?;
            let mut lessons = 0usize;
            for (ci, ch) in chapters.iter().enumerate() {
                if let Some(w) = only
                    && ci != w
                {
                    continue;
                }
                for l in &ch.lessons {
                    if cancel.is_cancelled() {
                        cx_send(&tx, ZsEvent::Done("已取消".into()));
                        return anyhow::Ok(());
                    }
                    lessons += 1;
                    cx_send(
                        &tx,
                        ZsEvent::Log(Level::Info, format!("视频：{}（{}s）", l.name, l.sec)),
                    );
                    let v = client.lesson_progress(l).await?;
                    let prelearn = v
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
                    if learned >= l.sec && l.sec > 0 {
                        cx_send(&tx, ZsEvent::Log(Level::Info, "  已完成，跳过".into()));
                        continue;
                    }
                    if prelearn == 0 {
                        cx_send(&tx, ZsEvent::Log(Level::Warn, "  缺少 prelearnId，跳过".into()));
                        continue;
                    }
                    let delta = l.sec.saturating_sub(learned);
                    let total = study_total.max(learned) + delta;
                    match client
                        .report_video(l, prelearn, delta, total, l.sec)
                        .await
                    {
                        Ok(resp) => {
                            let code = resp.get("code").and_then(|x| x.as_i64()).unwrap_or(0);
                            if code == -12 || code == -13 {
                                cx_send(
                                    &tx,
                                    ZsEvent::Log(
                                        Level::Error,
                                        "  触发风控：请打开播放页完成验证后重试".into(),
                                    ),
                                );
                                continue;
                            }
                            let ok = resp
                                .pointer("/data/submitSuccess")
                                .and_then(|x| x.as_bool())
                                .unwrap_or(false);
                            cx_send(
                                &tx,
                                ZsEvent::Log(
                                    Level::Info,
                                    format!("  上报 {}s/{}s -> {}", l.sec, l.sec, ok),
                                ),
                            );
                        }
                        Err(e) => {
                            cx_send(
                                &tx,
                                ZsEvent::Log(Level::Error, format!("  上报失败：{:#}", e)),
                            );
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
            cx_send(
                &tx,
                ZsEvent::Done(format!("视频批量结束，共处理 {} 节", lessons)),
            );
            anyhow::Ok(())
        }
        .await;
        if let Err(e) = run {
            cx_send(&tx, ZsEvent::Done(format!("运行失败：{e:#}")));
        }
    });
}

pub fn apply(zs: &mut ZsState, ev: ZsEvent) {
    match ev {
        ZsEvent::Data(Ok((chapters, hw, sub))) => {
            zs.loading = false;
            zs.loaded = true;
            zs.chapters = chapters;
            zs.homeworks = hw;
            zs.submitted = sub;
            zs.chapter_idx = 0;
            zs.hw_idx = 0;
            zs.status = format!(
                "已加载 {} 章 / 未提交 {} 个 / 已提交 {} 个",
                zs.chapters.len(),
                zs.homeworks.len(),
                zs.submitted.len()
            );
        }
        ZsEvent::Data(Err(e)) => {
            zs.loading = false;
            let hint = if e.contains("401") || e.contains("Forbidden") || e.contains("登录") {
                "（登录已失效：运行 `zhihuishu login`，会自动打开浏览器让你过验证码）"
            } else {
                ""
            };
            zs.status = format!("加载失败：{e}{hint}");
            zs.push_log(format!("加载失败：{e}{hint}"));
            log::error!("[知到] {}", zs.status);
        }
        ZsEvent::Log(level, msg) => {
            match level {
                Level::Info => log::info!("[知到] {}", msg),
                Level::Warn => log::warn!("[知到] {}", msg),
                Level::Error => log::error!("[知到] {}", msg),
            }
            let tag = match level {
                Level::Info => "INFO",
                Level::Warn => "WARN",
                Level::Error => "ERR ",
            };
            zs.push_log(format!("[{}] {}", tag, msg));
            zs.status = msg;
        }
        ZsEvent::Progress { done, total, text } => {
            zs.status = format!("{text}（{done}/{total}）");
        }
        ZsEvent::Score(s) => {
            zs.last_score = Some(s.clone());
            zs.push_log(format!("得分：{}", s));
        }
        ZsEvent::Done(msg) => {
            log::info!("[知到] {}", msg);
            zs.running = false;
            zs.cancel = None;
            zs.status = msg.clone();
            zs.push_log(msg);
        }
    }
}

pub fn handle_key(
    zs: &mut ZsState,
    k: crossterm::event::KeyEvent,
    cfg: &Config,
    tx: &UnboundedSender<AppEvent>,
) -> bool {
    use crossterm::event::KeyCode;
    if zs.running {
        if k.code == KeyCode::Esc {
            if let Some(token) = &zs.cancel {
                token.cancel();
            }
            zs.status = "正在取消…".into();
        }
        return true;
    }
    match k.code {
        KeyCode::Char('r') => {
            zs.loading = true;
            zs.status = "加载中…".into();
            spawn_load(cfg.clone(), tx.clone());
        }
        KeyCode::Char('s') => {
            zs.show_submitted = !zs.show_submitted;
            zs.hw_idx = 0;
            zs.status = if zs.show_submitted {
                "已提交作业（只读）".into()
            } else {
                "未提交作业".into()
            };
        }
        KeyCode::Tab => {
            zs.focus = match zs.focus {
                ZsFocus::Chapters => ZsFocus::Homeworks,
                ZsFocus::Homeworks => ZsFocus::Chapters,
            };
        }
        KeyCode::Char('j') | KeyCode::Down => match zs.focus {
            ZsFocus::Chapters => {
                if zs.chapter_idx + 1 < zs.chapters.len() {
                    zs.chapter_idx += 1;
                }
            }
            ZsFocus::Homeworks => {
                let n = if zs.show_submitted {
                    zs.submitted.len()
                } else {
                    zs.homeworks.len()
                };
                if zs.hw_idx + 1 < n {
                    zs.hw_idx += 1;
                }
            }
        },
        KeyCode::Char('k') | KeyCode::Up => match zs.focus {
            ZsFocus::Chapters => zs.chapter_idx = zs.chapter_idx.saturating_sub(1),
            ZsFocus::Homeworks => zs.hw_idx = zs.hw_idx.saturating_sub(1),
        },
        KeyCode::Char('a') | KeyCode::Enter => {
            if !crate::platform::zhihuishu::HOMEWORK_SUBMIT_ENABLED {
                zs.status = crate::platform::zhihuishu::HOMEWORK_SUBMIT_DISABLED_MSG.into();
                zs.push_log(zs.status.clone());
            } else if zs.show_submitted {
                zs.status = "已提交作业不可再作答".into();
            } else if let Some(hw) = zs.homeworks.get(zs.hw_idx).cloned() {
                zs.running = true;
                zs.status = format!("作答：{}", hw.name);
                let token = CancellationToken::new();
                zs.cancel = Some(token.clone());
                spawn_answer(cfg.clone(), hw, token, tx.clone());
            }
        }
        KeyCode::Char('v') => {
            zs.running = true;
            zs.status = "视频批量（当前章节）".into();
            let token = CancellationToken::new();
            zs.cancel = Some(token.clone());
            spawn_videos(
                cfg.clone(),
                zs.chapters.clone(),
                Some(zs.chapter_idx),
                token,
                tx.clone(),
            );
        }
        KeyCode::Char('V') => {
            zs.running = true;
            zs.status = "视频批量（全部章节）".into();
            let token = CancellationToken::new();
            zs.cancel = Some(token.clone());
            spawn_videos(cfg.clone(), zs.chapters.clone(), None, token, tx.clone());
        }
        _ => {}
    }
    true
}

pub fn render(f: &mut Frame, area: Rect, zs: &mut ZsState) {
    zs.hot.clear();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(9),
            Constraint::Length(1),
        ])
        .split(area);
    let title = format!(
        "知到 / 智慧树  章节 {}  作业 {}  {}",
        zs.chapters.len(),
        if zs.show_submitted {
            zs.submitted.len()
        } else {
            zs.homeworks.len()
        },
        if zs.running { "[运行中 Esc 取消]" } else { "" }
    );
    let status = Paragraph::new(format!(
        "{}\n{}",
        title,
        zs.last_score
            .as_ref()
            .map(|s| format!("最近得分：{}", s))
            .unwrap_or_else(|| zs.status.clone())
    ))
    .block(Block::default().borders(Borders::ALL).title("状态"))
    .wrap(Wrap { trim: true });
    f.render_widget(status, chunks[0]);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    let chapter_items: Vec<ListItem> = zs
        .chapters
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mark = if i == zs.chapter_idx && zs.focus == ZsFocus::Chapters {
                "▶"
            } else {
                " "
            };
            ListItem::new(Line::from(vec![Span::raw(format!(
                "{} {}. {}（{} 节）",
                mark,
                i + 1,
                c.name,
                c.lessons.len()
            ))]))
        })
        .collect();
    let chapter_block = Block::default()
        .borders(Borders::ALL)
        .title("章节（j/k 选择，v 本章视频，V 全部）");
    f.render_widget(List::new(chapter_items).block(chapter_block), cols[0]);
    register_rows(&mut zs.hot, cols[0], zs.chapters.len(), ZsHit::Chapter);

    let list = if zs.show_submitted {
        &zs.submitted
    } else {
        &zs.homeworks
    };
    let hw_items: Vec<ListItem> = list
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let mark = if i == zs.hw_idx && zs.focus == ZsFocus::Homeworks {
                "▶"
            } else {
                " "
            };
            let score = h
                .score
                .as_ref()
                .map(|s| format!("  得分 {}", s))
                .unwrap_or_default();
            ListItem::new(Line::from(vec![Span::raw(format!(
                "{} {}  {}{}",
                mark, h.name, h.total_score, score
            ))]))
        })
        .collect();
    let hw_block = Block::default().borders(Borders::ALL).title(if zs.show_submitted {
        "已提交作业（s 切回，只读）"
    } else {
        "未提交作业（a 暂不可用，s 看已提交）"
    });
    f.render_widget(List::new(hw_items).block(hw_block), cols[1]);
    register_rows(&mut zs.hot, cols[1], list.len(), ZsHit::Hw);

    let log_text: Vec<Line> = zs
        .logs
        .iter()
        .rev()
        .take(8)
        .rev()
        .map(|l| Line::from(l.clone()))
        .collect();
    let log = Paragraph::new(log_text).block(Block::default().borders(Borders::ALL).title("日志"));
    f.render_widget(log, chunks[2]);

    render_chips(
        f,
        chunks[3],
        &mut zs.hot,
        &[
            ('a', "作业暂不可用"),
            ('s', "已提交"),
            ('v', "本章视频"),
            ('V', "全部视频"),
            ('r', "刷新"),
            ('x', "取消"),
        ],
    );
}

fn register_rows(hot: &mut Vec<(Rect, ZsHit)>, area: Rect, n: usize, f: impl Fn(usize) -> ZsHit) {
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

fn text_width(s: &str) -> u16 {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

fn render_chips(f: &mut Frame, area: Rect, hot: &mut Vec<(Rect, ZsHit)>, chips: &[(char, &str)]) {
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
            ZsHit::Action(*key),
        ));
        x += w + 1;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn hit(zs: &ZsState, x: u16, y: u16) -> Option<ZsHit> {
    zs.hot
        .iter()
        .rev()
        .find(|(r, _)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
        .map(|(_, h)| *h)
}

/// 处理知到标签页的鼠标事件；返回是否已消费。
pub fn handle_mouse(
    zs: &mut ZsState,
    m: &crossterm::event::MouseEvent,
    cfg: &Config,
    tx: &UnboundedSender<AppEvent>,
) -> bool {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let Some(h) = hit(zs, m.column, m.row) else {
                return false;
            };
            match h {
                ZsHit::Chapter(i) => {
                    zs.chapter_idx = i;
                    zs.focus = ZsFocus::Chapters;
                }
                ZsHit::Hw(i) => {
                    zs.hw_idx = i;
                    zs.focus = ZsFocus::Homeworks;
                }
                ZsHit::Focus(f) => zs.focus = f,
                ZsHit::Action(c) => {
                    let code = if c == 'x' {
                        KeyCode::Esc
                    } else {
                        KeyCode::Char(c)
                    };
                    let k = KeyEvent::new(code, KeyModifiers::NONE);
                    handle_key(zs, k, cfg, tx);
                }
            }
            true
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = matches!(m.kind, MouseEventKind::ScrollUp);
            match zs.focus {
                ZsFocus::Chapters => {
                    if up {
                        zs.chapter_idx = zs.chapter_idx.saturating_sub(1);
                    } else if zs.chapter_idx + 1 < zs.chapters.len() {
                        zs.chapter_idx += 1;
                    }
                }
                ZsFocus::Homeworks => {
                    let n = if zs.show_submitted {
                        zs.submitted.len()
                    } else {
                        zs.homeworks.len()
                    };
                    if up {
                        zs.hw_idx = zs.hw_idx.saturating_sub(1);
                    } else if zs.hw_idx + 1 < n {
                        zs.hw_idx += 1;
                    }
                }
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(uuid: &str, recruit: i64, course: i64) -> Config {
        let mut c = Config::default();
        c.zhihuishu.uuid = uuid.into();
        c.zhihuishu.recruit_id = recruit;
        c.zhihuishu.course_id = course;
        c
    }

    #[test]
    fn account_prefers_uuid() {
        assert_eq!(account(&cfg_with("Xe6OpKN2", 1, 2)), "zXe6OpKN2");
        assert_eq!(account(&cfg_with("", 467029, 1000009484)), "z467029-1000009484");
    }

    #[test]
    fn apply_data_sets_counts() {
        let mut zs = ZsState::default();
        apply(
            &mut zs,
            ZsEvent::Data(Ok((vec![], vec![], vec![]))),
        );
        assert!(zs.loaded);
        assert!(zs.status.contains("已加载"));
    }
}
