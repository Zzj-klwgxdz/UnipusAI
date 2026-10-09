use crossterm::event::Event;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::api::course::GroupTask;
use crate::api::session::Session;
use crate::config::Config;
use crate::db;
use crate::dump_text::{DumpOptions, DumpSummary};
use crate::preview::Preview;
use crate::reporter::{FnReporter, ReportEvent, SharedReporter};

/// 当前界面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Settings,
    Dump,
    Preview,
    Help,
}

/// 主界面焦点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Detail,
    Logs,
}

/// 单元视图（任务懒加载）。
#[derive(Debug, Clone, Default)]
pub struct UnitUi {
    pub id: String,
    pub label: Option<String>,
    pub tasks: Vec<GroupTask>,
    pub expanded: bool,
    pub loading: bool,
    pub error: Option<String>,
}

/// 日志行。
#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: log::Level,
    pub text: String,
}

/// 运行状态。
#[derive(Debug)]
pub struct RunState {
    pub scope: String,
    pub done: u32,
    pub skipped: u32,
    pub failed: u32,
    pub current: Option<(String, String)>,
    pub cancel: CancellationToken,
    pub finished: bool,
}

/// 配置编辑字段类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Secret,
    U64,
    F32,
    Bool,
    Strategy,
    OptionStr,
}

/// 配置字段定义。
pub struct FieldDef {
    pub label: &'static str,
    pub kind: FieldKind,
}

pub const FIELDS: &[FieldDef] = &[
    FieldDef { label: "timeout(秒)", kind: FieldKind::U64 },
    FieldDef { label: "cookie", kind: FieldKind::Secret },
    FieldDef { label: "authorization", kind: FieldKind::Secret },
    FieldDef { label: "x_annotator_auth_token", kind: FieldKind::Secret },
    FieldDef { label: "u_school", kind: FieldKind::Text },
    FieldDef { label: "course_id", kind: FieldKind::Text },
    FieldDef { label: "class_id", kind: FieldKind::Text },
    FieldDef { label: "curricula_id", kind: FieldKind::Text },
    FieldDef { label: "open_id", kind: FieldKind::Text },
    FieldDef { label: "publish_version", kind: FieldKind::Text },
    FieldDef { label: "api_key", kind: FieldKind::Secret },
    FieldDef { label: "base_url", kind: FieldKind::Text },
    FieldDef { label: "model", kind: FieldKind::Text },
    FieldDef { label: "learning_strategy", kind: FieldKind::Strategy },
    FieldDef { label: "max_tokens", kind: FieldKind::U64 },
    FieldDef { label: "temperature", kind: FieldKind::F32 },
    FieldDef { label: "fallback_on_llm_failure", kind: FieldKind::Bool },
    FieldDef { label: "interval_ms", kind: FieldKind::U64 },
    FieldDef { label: "whisper_enabled", kind: FieldKind::Bool },
    FieldDef { label: "whisper_model", kind: FieldKind::OptionStr },
    FieldDef { label: "whisper_language", kind: FieldKind::OptionStr },
];

/// 设置界面状态。
#[derive(Debug)]
pub struct SettingsState {
    pub draft: Config,
    pub cursor: usize,
    pub editing: bool,
    pub buf: String,
    pub msg: Option<String>,
    pub reveal: bool,
}

impl SettingsState {
    pub fn new(cfg: &Config) -> Self {
        Self {
            draft: cfg.clone(),
            cursor: 0,
            editing: false,
            buf: String::new(),
            msg: None,
            reveal: false,
        }
    }

    pub fn value(&self, idx: usize) -> String {
        let c = &self.draft;
        match idx {
            0 => c.timeout.to_string(),
            1 => c.cookie.clone(),
            2 => c.authorization.clone(),
            3 => c.x_annotator_auth_token.clone(),
            4 => c.u_school.clone(),
            5 => c.course_id.clone(),
            6 => c.class_id.clone(),
            7 => c.curricula_id.clone(),
            8 => c.open_id.clone(),
            9 => c.publish_version.clone(),
            10 => c.api_key.clone(),
            11 => c.base_url.clone(),
            12 => c.model.clone(),
            13 => c.learning_strategy.clone(),
            14 => c.max_tokens.to_string(),
            15 => c.temperature.to_string(),
            16 => c.fallback_on_llm_failure.to_string(),
            17 => c.interval_ms.to_string(),
            18 => c.whisper_enabled.to_string(),
            19 => c.whisper_model.clone(),
            20 => c.whisper_language.clone(),
            _ => String::new(),
        }
    }

    pub fn set_value(&mut self, idx: usize, v: &str) {
        let c = &mut self.draft;
        match idx {
            0 => c.timeout = v.trim().parse().unwrap_or(c.timeout),
            1 => c.cookie = v.to_string(),
            2 => c.authorization = v.to_string(),
            3 => c.x_annotator_auth_token = v.to_string(),
            4 => c.u_school = v.to_string(),
            5 => c.course_id = v.to_string(),
            6 => c.class_id = v.to_string(),
            7 => c.curricula_id = v.to_string(),
            8 => c.open_id = v.to_string(),
            9 => c.publish_version = v.to_string(),
            10 => c.api_key = v.to_string(),
            11 => c.base_url = v.to_string(),
            12 => c.model = v.to_string(),
            13 => c.learning_strategy = v.to_string(),
            14 => c.max_tokens = v.trim().parse().unwrap_or(c.max_tokens),
            15 => c.temperature = v.trim().parse().unwrap_or(c.temperature),
            16 => c.fallback_on_llm_failure = v.trim() == "true",
            17 => c.interval_ms = v.trim().parse().unwrap_or(c.interval_ms),
            18 => c.whisper_enabled = v.trim() == "true",
            19 => c.whisper_model = v.to_string(),
            20 => c.whisper_language = v.to_string(),
            _ => {}
        }
    }
}

/// 预览界面状态。
#[derive(Debug)]
pub struct PreviewState {
    pub gid: String,
    /// 所属单元 id（重新 dump 用）
    pub unit_id: String,
    pub lines: Vec<String>,
    pub scroll: usize,
    pub draft: Option<String>,
    pub loading: bool,
}

/// 确认弹窗动作。
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    GenerateDraft(String),
    QuitRunning,
}

/// 确认弹窗。
#[derive(Debug)]
pub struct Confirm {
    pub prompt: String,
    pub action: ConfirmAction,
}

/// 鼠标命中区域动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    ToggleUnit(usize),
    SelectUnit(usize),
    SelectTask(usize, usize),
    Button(ButtonId),
    Field(usize),
}

/// 底栏按钮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonId {
    RunTask,
    RunUnit,
    RunAll,
    Preview,
    Dump,
    DumpForce,
    Settings,
    Refresh,
    Help,
    Quit,
    Save,
    Back,
    RunDump,
    GenDraft,
    Redump,
    CancelRun,
}

/// 后台 → UI 事件。
#[derive(Debug)]
pub enum AppEvent {
    Input(Event),
    Log {
        level: log::Level,
        text: String,
    },
    /// (课程名, 单元任务树) —— 全部来自数据库
    TreeLoaded(Result<(Option<String>, Vec<UnitUi>), String>),
    Report(ReportEvent),
    /// dump_text/dump.db 实时汇总内容
    DumpText(Result<String, String>),
    /// dump 数据尚不存在
    DumpMissing,
    DumpDone(Result<DumpSummary, String>),
    PreviewDone(Result<(String, Preview), String>),
    DraftDone(Result<String, String>),
    ConfigSaved(Result<(), String>),
    Status(String),
    Tick,
}

/// 应用状态。
pub struct App {
    pub screen: Screen,
    pub quit: bool,
    pub session: Session,
    pub course_name: String,
    pub units: Vec<UnitUi>,
    pub unit_sel: usize,
    pub task_sel: Option<usize>,
    pub focus: Focus,
    pub logs: VecDeque<LogLine>,
    pub log_scroll: usize,
    pub log_auto: bool,
    pub run: Option<RunState>,
    pub settings: SettingsState,
    pub dump_text: Option<String>,
    pub dump_scroll: usize,
    pub dump_missing: bool,
    pub dump_loading: bool,
    pub preview: Option<PreviewState>,
    pub confirm: Option<Confirm>,
    pub status: String,
    pub mouse_map: Vec<(ratatui::layout::Rect, MouseAction)>,
    pub tick: u64,
    pub loading_tree: bool,
}

impl App {
    pub fn new(session: Session) -> Self {
        let settings = SettingsState::new(session.cfg());
        Self {
            screen: Screen::Dashboard,
            quit: false,
            session,
            course_name: String::new(),
            units: Vec::new(),
            unit_sel: 0,
            task_sel: None,
            focus: Focus::Tree,
            logs: VecDeque::new(),
            log_scroll: 0,
            log_auto: true,
            run: None,
            settings,
            dump_text: None,
            dump_scroll: 0,
            dump_missing: false,
            dump_loading: false,
            preview: None,
            confirm: None,
            status: String::new(),
            mouse_map: Vec::new(),
            tick: 0,
            loading_tree: false,
        }
    }

    pub fn push_log(&mut self, level: log::Level, text: String) {
        self.logs.push_back(LogLine { level, text });
        while self.logs.len() > 2000 {
            self.logs.pop_front();
        }
        if self.log_auto {
            self.log_scroll = self.logs.len().saturating_sub(1);
        }
    }

    /// 扁平化行：单元行 + （展开时）任务行。
    pub fn rows(&self) -> Vec<(usize, Option<usize>)> {
        let mut rows = Vec::new();
        for (ui, u) in self.units.iter().enumerate() {
            rows.push((ui, None));
            if u.expanded {
                for ti in 0..u.tasks.len() {
                    rows.push((ui, Some(ti)));
                }
            }
        }
        rows
    }

    pub fn cursor(&self) -> usize {
        let rows = self.rows();
        rows.iter()
            .position(|(u, t)| *u == self.unit_sel && *t == self.task_sel)
            .unwrap_or(0)
    }

    pub fn set_cursor(&mut self, idx: usize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let idx = idx.min(rows.len() - 1);
        let (u, t) = rows[idx];
        self.unit_sel = u;
        self.task_sel = t;
    }

    pub fn selected_task(&self) -> Option<&GroupTask> {
        let u = self.units.get(self.unit_sel)?;
        u.tasks.get(self.task_sel?)
    }

    pub fn course_name_display(&self) -> String {
        if self.course_name.is_empty() {
            crate::api::course::course_display_name_fallback(self.session.course_id())
        } else {
            self.course_name.clone()
        }
    }

    fn reporter(tx: &UnboundedSender<AppEvent>) -> SharedReporter {
        let tx = tx.clone();
        Arc::new(FnReporter(move |ev| {
            let _ = tx.send(AppEvent::Report(ev));
        }))
    }

    /// 从数据库加载课程单元与任务树（后台）。
    pub fn spawn_load_tree(&mut self, tx: &UnboundedSender<AppEvent>) {
        if self.loading_tree {
            return;
        }
        self.loading_tree = true;
        let tx = tx.clone();
        tokio::spawn(async move {
            let res = (|| -> anyhow::Result<(Option<String>, Vec<UnitUi>)> {
                let conn = db::open()?;
                let name = db::get_meta(&conn, "course_name")?;
                let units = db::load_tree(&conn)?
                    .into_iter()
                    .map(|u| {
                        let uid = u.unit_id;
                        UnitUi {
                            id: uid.clone(),
                            label: u.unit_label,
                            tasks: u
                                .tasks
                                .into_iter()
                                .map(|t| GroupTask {
                                    group_id: t.group_id,
                                    unit_id: uid.clone(),
                                    tab_type: t.tab_type,
                                    required: t.required,
                                    passed: t.passed,
                                    min_score_pct: 0,
                                    start_time: 0,
                                    end_time: 0,
                                })
                                .collect(),
                            expanded: false,
                            loading: false,
                            error: None,
                        }
                    })
                    .collect();
                Ok((name, units))
            })();
            let msg = res.map_err(|e| format!("{:#}", e));
            let _ = tx.send(AppEvent::TreeLoaded(msg));
        });
    }

    /// 重新加载任务树（刷新通过状态等）。
    pub fn refresh(&mut self, tx: &UnboundedSender<AppEvent>) {
        self.loading_tree = false;
        self.spawn_load_tree(tx);
        self.status = "刷新中…".into();
    }

    /// 运行当前单元 / 全部 / 单个任务。
    pub fn spawn_run(&mut self, tx: &UnboundedSender<AppEvent>, scope: RunScope) {
        if self.run.as_ref().is_some_and(|r| !r.finished) {
            self.status = "已有运行中的任务".into();
            return;
        }
        let cancel = CancellationToken::new();
        let reporter = Self::reporter(tx);
        let mut session = self.session.clone();
        let tx2 = tx.clone();
        let (scope_name, unit_ids): (String, Vec<String>) = match &scope {
            RunScope::Task(task) => (
                format!("任务 {}", task.group_id),
                vec![task.unit_id.clone()],
            ),
            RunScope::Unit(uid) => (format!("单元 {}", uid), vec![uid.clone()]),
            RunScope::All => ("全课程".into(), Vec::new()),
        };
        self.run = Some(RunState {
            scope: scope_name,
            done: 0,
            skipped: 0,
            failed: 0,
            current: None,
            cancel: cancel.clone(),
            finished: false,
        });
        self.screen = Screen::Dashboard;
        match scope {
            RunScope::Task(task) => {
                tokio::spawn(async move {
                    let r = crate::core::runner::process_group(&session, &task, &*reporter, &cancel)
                        .await;
                    match r {
                        Ok(_) => {
                            let _ = tx2.send(AppEvent::Report(ReportEvent::RunFinished {
                                done: 1,
                                skipped: 0,
                                failed: 0,
                            }));
                        }
                        Err(e) => {
                            let _ = tx2.send(AppEvent::Report(ReportEvent::TaskFailed {
                                group_id: task.group_id.clone(),
                                tab_type: task.tab_type.clone(),
                                error: format!("{:#}", e),
                            }));
                            let _ = tx2.send(AppEvent::Report(ReportEvent::RunFinished {
                                done: 0,
                                skipped: 0,
                                failed: 1,
                            }));
                        }
                    }
                });
            }
            _ => {
                tokio::spawn(async move {
                    let r = if unit_ids.is_empty() {
                        crate::core::runner::run_course(&mut session, false, &*reporter, &cancel)
                            .await
                    } else {
                        crate::core::runner::run_course_units(
                            &mut session,
                            &unit_ids,
                            false,
                            &*reporter,
                            &cancel,
                        )
                        .await
                    };
                    if let Err(e) = r {
                        let _ = tx2.send(AppEvent::Status(format!("运行失败: {:#}", e)));
                        let _ = tx2.send(AppEvent::Report(ReportEvent::RunFinished {
                            done: 0,
                            skipped: 0,
                            failed: 0,
                        }));
                    }
                });
            }
        }
    }

    /// 执行 dump-text（后台）。
    pub fn spawn_dump(&mut self, tx: &UnboundedSender<AppEvent>, force: bool) {
        let reporter = Self::reporter(tx);
        let session = self.session.clone();
        let cancel = CancellationToken::new();
        let tx2 = tx.clone();
        let opts = DumpOptions {
            force,
            with_names: false,
            unit_ids: Vec::new(),
            group_ids: Vec::new(),
        };
        self.status = if force {
            "dump-text --force 运行中…".into()
        } else {
            "dump-text 运行中…".into()
        };
        tokio::spawn(async move {
            let r = crate::dump_text::run_dump_text(&session, &opts, &*reporter, &cancel).await;
            match r {
                Ok(s) => {
                    let _ = tx2.send(AppEvent::DumpDone(Ok(s)));
                }
                Err(e) => {
                    let _ = tx2.send(AppEvent::DumpDone(Err(format!("{:#}", e))));
                }
            }
        });
    }

    /// 读取 dump 汇总文本（区分"尚无数据"与读取错误）。
    pub fn spawn_load_dump(&mut self, tx: &UnboundedSender<AppEvent>) {
        self.dump_loading = true;
        let tx = tx.clone();
        tokio::spawn(async move {
            let ev = match crate::dump::summary_text() {
                Ok(Some(text)) => AppEvent::DumpText(Ok(text)),
                Ok(None) => AppEvent::DumpMissing,
                Err(e) => {
                    let msg = format!("读取 {} 失败: {:#}", crate::dump::db_path().display(), e);
                    AppEvent::DumpText(Err(msg))
                }
            };
            let _ = tx.send(ev);
        });
    }

    /// 加载选中任务的只读预览（从数据库）。
    pub fn spawn_preview(&mut self, tx: &UnboundedSender<AppEvent>) {
        let Some(task) = self.selected_task().cloned() else {
            self.status = "未选中任务".into();
            return;
        };
        self.open_preview(tx, task.unit_id, task.group_id);
    }

    /// 打开指定任务的预览界面并加载内容。
    fn open_preview(&mut self, tx: &UnboundedSender<AppEvent>, unit_id: String, gid: String) {
        self.preview = Some(PreviewState {
            gid: gid.clone(),
            unit_id,
            lines: Vec::new(),
            scroll: 0,
            draft: None,
            loading: true,
        });
        self.screen = Screen::Preview;
        self.load_preview(tx, gid);
    }

    /// 重新加载当前预览内容（数据库读取）。
    pub fn spawn_reload_preview(&mut self, tx: &UnboundedSender<AppEvent>) {
        let Some(pv) = self.preview.as_mut() else {
            return;
        };
        pv.loading = true;
        pv.scroll = 0;
        let gid = pv.gid.clone();
        self.load_preview(tx, gid);
    }

    fn load_preview(&self, tx: &UnboundedSender<AppEvent>, gid: String) {
        let tx2 = tx.clone();
        tokio::spawn(async move {
            let ev = match crate::preview::load_preview_from_db(&gid) {
                Ok(Some(p)) => AppEvent::PreviewDone(Ok((gid, p))),
                Ok(None) => AppEvent::PreviewDone(Err("库中无该任务数据（预览页 u 可重抓）".into())),
                Err(e) => AppEvent::PreviewDone(Err(format!("{:#}", e))),
            };
            let _ = tx2.send(ev);
        });
    }

    /// 重新抓取当前预览任务（单任务 dump 覆盖入库）。
    pub fn spawn_dump_task(&mut self, tx: &UnboundedSender<AppEvent>) {
        let Some(pv) = &self.preview else {
            return;
        };
        let unit_id = pv.unit_id.clone();
        let gid = pv.gid.clone();
        let reporter = Self::reporter(tx);
        let session = self.session.clone();
        let cancel = CancellationToken::new();
        let tx2 = tx.clone();
        let opts = DumpOptions {
            force: false,
            with_names: false,
            unit_ids: vec![unit_id],
            group_ids: vec![gid.clone()],
        };
        self.status = format!("重新抓取 {} …", gid);
        tokio::spawn(async move {
            let r = crate::dump_text::run_dump_text(&session, &opts, &*reporter, &cancel).await;
            match r {
                Ok(s) => {
                    let _ = tx2.send(AppEvent::DumpDone(Ok(s)));
                }
                Err(e) => {
                    let _ = tx2.send(AppEvent::DumpDone(Err(format!("{:#}", e))));
                }
            }
        });
    }

    /// 生成讨论草稿（调用 LLM，需确认；数据来自数据库）。
    pub fn spawn_draft(&mut self, tx: &UnboundedSender<AppEvent>, gid: String) {
        let session = self.session.clone();
        let tx2 = tx.clone();
        tokio::spawn(async move {
            let r = async {
                let preview = crate::preview::load_preview_from_db(&gid)?
                    .ok_or_else(|| anyhow::anyhow!("库中无该任务数据，请先重新 dump"))?;
                let group = preview
                    .group
                    .ok_or_else(|| anyhow::anyhow!("浏览类页面，无题目模块"))?;
                let mut drafts = Vec::new();
                for m in group.modules.iter().filter(|m| m.reply_type == "discussion") {
                    let vals = crate::solve::solve_module(&session, m).await?;
                    if let Some(v) = vals.first() {
                        drafts.push(v.trim().to_string());
                    }
                }
                if drafts.is_empty() {
                    anyhow::bail!("该任务组不含 discussion 模块");
                }
                Ok::<_, anyhow::Error>(drafts.join("\n\n"))
            }
            .await;
            let _ = tx2.send(AppEvent::DraftDone(r.map_err(|e| format!("{:#}", e))));
        });
    }

    /// 保存设置（写盘并重建 HTTP 客户端）。
    pub fn save_config(&mut self, tx: &UnboundedSender<AppEvent>) {
        let mut session = self.session.clone();
        match session.update_config(self.settings.draft.clone()) {
            Ok(()) => {
                self.session = session;
                self.settings.msg = Some("已保存并重建会话".into());
                self.status = "配置已保存".into();
                let _ = tx.send(AppEvent::ConfigSaved(Ok(())));
            }
            Err(e) => {
                let msg = format!("保存失败: {:#}", e);
                self.settings.msg = Some(msg.clone());
                let _ = tx.send(AppEvent::ConfigSaved(Err(msg)));
            }
        }
    }
}

/// 运行范围。
#[derive(Debug, Clone)]
pub enum RunScope {
    Task(Box<GroupTask>),
    Unit(String),
    All,
}
