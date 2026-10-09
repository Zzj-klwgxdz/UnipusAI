use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use log::Level;
use tokio::sync::mpsc::UnboundedSender;

use crate::preview::Preview;
use crate::reporter::ReportEvent;

use super::app::{App, AppEvent, ButtonId, Confirm, ConfirmAction, Focus, MouseAction, RunScope, Screen};

impl App {
    pub fn handle(&mut self, ev: AppEvent, tx: &UnboundedSender<AppEvent>) {
        match ev {
            AppEvent::Tick => self.tick = self.tick.wrapping_add(1),
            AppEvent::Log { level, text } => self.push_log(level, text),
            AppEvent::Status(s) => self.status = s,
            AppEvent::Input(e) => self.handle_input(e, tx),
            AppEvent::TreeLoaded(res) => {
                self.loading_tree = false;
                match res {
                    Ok((name, units)) => {
                        if let Some(n) = name
                            && !n.is_empty()
                        {
                            self.course_name = n;
                        }
                        // 保留展开状态与选中项（按 unit_id / group_id 恢复）
                        let old_expanded: std::collections::HashMap<String, bool> = self
                            .units
                            .iter()
                            .map(|u| (u.id.clone(), u.expanded))
                            .collect();
                        let sel_unit = self.units.get(self.unit_sel).map(|u| u.id.clone());
                        let sel_task = self.selected_task().map(|t| t.group_id.clone());
                        let had_units = !self.units.is_empty();
                        self.units = units;
                        for u in self.units.iter_mut() {
                            u.expanded = old_expanded.get(&u.id).copied().unwrap_or(false);
                        }
                        if !had_units
                            && let Some(u) = self.units.first_mut()
                        {
                            u.expanded = true;
                        }
                        if let Some(uid) = sel_unit
                            && let Some(ui) = self.units.iter().position(|u| u.id == uid)
                        {
                            self.unit_sel = ui;
                            self.task_sel = sel_task
                                .and_then(|g| self.units[ui].tasks.iter().position(|t| t.group_id == g));
                        } else {
                            self.unit_sel = 0;
                            self.task_sel = None;
                        }
                        self.status = if self.units.is_empty() {
                            "尚无 dump 数据：按 D 导出".into()
                        } else {
                            format!("已从数据库加载 {} 个单元", self.units.len())
                        };
                    }
                    Err(e) => {
                        self.status = format!("加载任务树失败: {}", e);
                        self.push_log(Level::Error, format!("加载任务树失败: {}", e));
                    }
                }
            }
            AppEvent::Report(re) => self.handle_report(re, tx),
            AppEvent::DumpText(res) => match res {
                Ok(text) => {
                    self.dump_text = Some(text);
                    self.dump_scroll = 0;
                    self.dump_missing = false;
                    self.dump_loading = false;
                }
                Err(e) => {
                    self.status = e.clone();
                    self.dump_loading = false;
                    self.push_log(Level::Error, e);
                }
            },
            AppEvent::DumpMissing => {
                self.dump_text = None;
                self.dump_scroll = 0;
                self.dump_missing = true;
                self.dump_loading = false;
                self.status = "尚无 dump 数据（Enter 导出 / F 全量重建）".into();
            }
            AppEvent::DumpDone(res) => match res {
                Ok(s) => {
                    self.status = format!(
                        "dump-text 完成: 新生成 {}，已存在 {}(刷新 {})，累计任务组 {}",
                        s.generated, s.skipped, s.updated, s.total_files
                    );
                    // 导出后刷新汇总、任务树与当前预览（数据全部来自数据库）
                    self.spawn_load_dump(tx);
                    self.spawn_load_tree(tx);
                    if self.preview.is_some() {
                        self.spawn_reload_preview(tx);
                    }
                }
                Err(e) => {
                    self.status = format!("dump-text 失败: {}", e);
                    self.push_log(Level::Error, format!("dump-text 失败: {}", e));
                }
            },
            AppEvent::PreviewDone(res) => match res {
                Ok((gid, p)) => {
                    if let Some(pv) = self.preview.as_mut() {
                        pv.gid = gid;
                        pv.lines = preview_lines(&p);
                        pv.loading = false;
                    }
                }
                Err(e) => {
                    if let Some(pv) = self.preview.as_mut() {
                        pv.loading = false;
                        pv.lines = vec![format!("预览失败: {}", e)];
                    }
                }
            },
            AppEvent::DraftDone(res) => match res {
                Ok(text) => {
                    if let Some(pv) = self.preview.as_mut() {
                        pv.draft = Some(text);
                    }
                    self.status = "讨论草稿已生成".into();
                }
                Err(e) => {
                    self.status = format!("生成草稿失败: {}", e);
                    self.push_log(Level::Error, format!("生成草稿失败: {}", e));
                }
            },
            AppEvent::ConfigSaved(res) => {
                if let Err(e) = res {
                    self.push_log(Level::Error, format!("保存配置失败: {}", e));
                }
            }
        }
    }

    fn handle_report(&mut self, re: ReportEvent, tx: &UnboundedSender<AppEvent>) {
        if let Some(run) = self.run.as_mut() {
            match &re {
                ReportEvent::TaskStart { group_id, tab_type } => {
                    run.current = Some((group_id.clone(), tab_type.clone()));
                }
                ReportEvent::TaskDone { .. } => {
                    run.done += 1;
                    run.current = None;
                }
                ReportEvent::TaskSkipped { .. } => run.skipped += 1,
                ReportEvent::TaskFailed { .. } => {
                    run.failed += 1;
                    run.current = None;
                }
                ReportEvent::RunFinished {
                    done,
                    skipped,
                    failed,
                } => {
                    run.done = *done;
                    run.skipped = *skipped;
                    run.failed = *failed;
                    run.current = None;
                    run.finished = true;
                }
                _ => {}
            }
        }
        match re {
            ReportEvent::TaskDone { group_id, .. } => {
                self.push_log(Level::Info, format!("[OK] {}", group_id))
            }
            ReportEvent::TaskFailed { group_id, error, .. } => {
                self.push_log(Level::Error, format!("[FAIL] {} -> {}", group_id, error))
            }
            ReportEvent::RateLimited {
                attempt,
                max,
                wait_secs,
                ..
            } => self.push_log(
                Level::Warn,
                format!("触发限频，等待 {} 秒后重试 ({}/{})", wait_secs, attempt, max),
            ),
            ReportEvent::Dump {
                group_id,
                kind,
                path,
                ..
            } => {
                let label = match kind {
                    crate::reporter::DumpKind::Generated => "已生成",
                    crate::reporter::DumpKind::ViewOnly => "浏览类页面",
                    crate::reporter::DumpKind::Refreshed => "状态刷新",
                };
                self.push_log(Level::Info, format!("[dump] {} {} {}", label, group_id, path));
            }
            ReportEvent::RunFinished { done, skipped, failed } => {
                self.push_log(
                    Level::Info,
                    format!("运行结束: done={} skipped={} failed={}", done, skipped, failed),
                );
                // 结束后刷新单元/任务通过状态
                self.refresh(tx);
            }
            ReportEvent::Info(m) => self.push_log(Level::Info, m),
            ReportEvent::Warn(m) => self.push_log(Level::Warn, m),
            ReportEvent::Error(m) => self.push_log(Level::Error, m),
            ReportEvent::Plain(m) => self.push_log(Level::Info, m),
            _ => {}
        }
    }

    fn handle_input(&mut self, ev: Event, tx: &UnboundedSender<AppEvent>) {
        match ev {
            Event::Key(k) => {
                if k.kind != KeyEventKind::Press && k.kind != KeyEventKind::Repeat {
                    return;
                }
                self.handle_key(k, tx);
            }
            Event::Mouse(m) => self.handle_mouse(m, tx),
            Event::Resize(_, _) => {}
            _ => {}
        }
    }

    fn handle_key(&mut self, k: KeyEvent, tx: &UnboundedSender<AppEvent>) {
        // 确认弹窗优先
        if let Some(confirm) = self.confirm.take() {
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    match confirm.action {
                        ConfirmAction::GenerateDraft(gid) => self.spawn_draft(tx, gid),
                        ConfirmAction::QuitRunning => {
                            if let Some(run) = &self.run {
                                run.cancel.cancel();
                            }
                            self.quit = true;
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        // 全局退出
        if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c')) {
            self.quit = true;
            return;
        }
        match self.screen {
            Screen::Help => {
                self.screen = Screen::Dashboard;
                return;
            }
            Screen::Settings => {
                self.handle_settings_key(k, tx);
                return;
            }
            _ => {}
        }
        // 运行中 Esc 取消
        if k.code == KeyCode::Esc
            && let Some(run) = &self.run
            && !run.finished
        {
            run.cancel.cancel();
            self.status = "已请求取消…".into();
            return;
        }
        match self.screen {
            Screen::Dump => self.handle_dump_key(k, tx),
            Screen::Preview => self.handle_preview_key(k, tx),
            _ => self.handle_dashboard_key(k, tx),
        }
    }

    fn handle_dashboard_key(&mut self, k: KeyEvent, tx: &UnboundedSender<AppEvent>) {
        match k.code {
            KeyCode::Char('q') => {
                if self.run.as_ref().is_some_and(|r| !r.finished) {
                    self.confirm = Some(Confirm {
                        prompt: "正在运行，确定退出？(y/n)".into(),
                        action: ConfirmAction::QuitRunning,
                    });
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('?') | KeyCode::Char('H') => self.screen = Screen::Help,
            KeyCode::Char('s') => {
                self.settings = super::app::SettingsState::new(self.session.cfg());
                self.screen = Screen::Settings;
            }
            KeyCode::Char('d') => {
                self.screen = Screen::Dump;
                self.spawn_load_dump(tx);
            }
            KeyCode::Char('p') => self.spawn_preview(tx),
            KeyCode::Char('r') => self.refresh(tx),
            KeyCode::Char('R') => self.run_unit(tx),
            KeyCode::Char('A') => self.run_all(tx),
            KeyCode::Char('f') => self.run_task_force(tx),
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Tree => Focus::Detail,
                    Focus::Detail => Focus::Logs,
                    Focus::Logs => Focus::Tree,
                };
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1, tx),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1, tx),
            KeyCode::Left | KeyCode::Char('h') => self.collapse_or_up(tx),
            KeyCode::Right | KeyCode::Char('l') => self.expand_or_down(tx),
            KeyCode::Enter => self.enter_row(tx),
            KeyCode::PageUp => self.scroll_logs(-10),
            KeyCode::PageDown => self.scroll_logs(10),
            _ => {}
        }
    }

    fn handle_dump_key(&mut self, k: KeyEvent, tx: &UnboundedSender<AppEvent>) {
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Dashboard,
            KeyCode::Up | KeyCode::Char('k') => {
                self.dump_scroll = self.dump_scroll.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => self.dump_scroll += 1,
            KeyCode::PageUp => self.dump_scroll = self.dump_scroll.saturating_sub(20),
            KeyCode::PageDown => self.dump_scroll += 20,
            KeyCode::Char('r') => self.spawn_load_dump(tx),
            KeyCode::Enter => self.spawn_dump(tx, false),
            KeyCode::Char('F') => self.spawn_dump(tx, true),
            _ => {}
        }
    }

    fn handle_preview_key(&mut self, k: KeyEvent, tx: &UnboundedSender<AppEvent>) {
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Dashboard,
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(pv) = self.preview.as_mut() {
                    pv.scroll = pv.scroll.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(pv) = self.preview.as_mut() {
                    pv.scroll += 1;
                }
            }
            KeyCode::PageUp => {
                if let Some(pv) = self.preview.as_mut() {
                    pv.scroll = pv.scroll.saturating_sub(20);
                }
            }
            KeyCode::PageDown => {
                if let Some(pv) = self.preview.as_mut() {
                    pv.scroll += 20;
                }
            }
            KeyCode::Char('g') => {
                if let Some(pv) = &self.preview
                    && !pv.loading
                {
                    let gid = pv.gid.clone();
                    self.confirm = Some(Confirm {
                        prompt: "调用 LLM 生成讨论草稿？可能产生费用 (y/n)".into(),
                        action: ConfirmAction::GenerateDraft(gid),
                    });
                }
            }
            KeyCode::Char('u') => self.spawn_dump_task(tx),
            _ => {}
        }
    }

    fn handle_settings_key(&mut self, k: KeyEvent, tx: &UnboundedSender<AppEvent>) {
        if self.settings.editing {
            match k.code {
                KeyCode::Esc => {
                    self.settings.editing = false;
                    self.settings.buf.clear();
                }
                KeyCode::Enter => {
                    let idx = self.settings.cursor;
                    let buf = self.settings.buf.clone();
                    self.settings.set_value(idx, &buf);
                    self.settings.editing = false;
                }
                KeyCode::Backspace => {
                    self.settings.buf.pop();
                }
                KeyCode::Char(c) => self.settings.buf.push(c),
                KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End => {}
                _ => {}
            }
            return;
        }
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Dashboard,
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings.cursor = self.settings.cursor.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings.cursor =
                    (self.settings.cursor + 1).min(super::app::FIELDS.len() - 1)
            }
            KeyCode::Char('v') => self.settings.reveal = !self.settings.reveal,
            KeyCode::Char('w') => self.save_config(tx),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let idx = self.settings.cursor;
                match super::app::FIELDS[idx].kind {
                    super::app::FieldKind::Bool => {
                        let v = self.settings.value(idx) == "true";
                        self.settings.set_value(idx, if v { "false" } else { "true" });
                    }
                    super::app::FieldKind::Strategy => {
                        let v = self.settings.value(idx);
                        let next = if v.contains("compusory") || v.contains("compulsory") {
                            "learn_all"
                        } else {
                            "learn_all_compusory_course"
                        };
                        self.settings.set_value(idx, next);
                    }
                    _ => {
                        self.settings.buf = self.settings.value(idx);
                        self.settings.editing = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn move_cursor(&mut self, delta: i32, _tx: &UnboundedSender<AppEvent>) {
        let cur = self.cursor() as i32 + delta;
        if cur < 0 {
            return;
        }
        self.set_cursor(cur as usize);
    }

    fn enter_row(&mut self, tx: &UnboundedSender<AppEvent>) {
        if self.task_sel.is_some() {
            self.run_task(tx);
        } else {
            self.toggle_unit();
        }
    }

    fn toggle_unit(&mut self) {
        let idx = self.unit_sel;
        let Some(u) = self.units.get_mut(idx) else {
            return;
        };
        u.expanded = !u.expanded;
    }

    fn collapse_or_up(&mut self, tx: &UnboundedSender<AppEvent>) {
        if self.task_sel.is_none() {
            if let Some(u) = self.units.get_mut(self.unit_sel) {
                u.expanded = false;
            }
        } else {
            self.move_cursor(-1, tx);
        }
    }

    fn expand_or_down(&mut self, tx: &UnboundedSender<AppEvent>) {
        if self.task_sel.is_none() {
            if let Some(u) = self.units.get_mut(self.unit_sel) {
                u.expanded = true;
            }
        } else {
            self.move_cursor(1, tx);
        }
    }

    fn run_task(&mut self, tx: &UnboundedSender<AppEvent>) {
        let Some(task) = self.selected_task().cloned() else {
            self.status = "未选中任务".into();
            return;
        };
        if task.passed {
            self.status = format!("{} 已通过（f 可强制重做）", task.group_id);
            return;
        }
        self.spawn_run(tx, RunScope::Task(Box::new(task)));
    }

    /// 强制运行选中任务（含已通过）。
    fn run_task_force(&mut self, tx: &UnboundedSender<AppEvent>) {
        let Some(task) = self.selected_task().cloned() else {
            self.status = "未选中任务".into();
            return;
        };
        self.spawn_run(tx, RunScope::Task(Box::new(task)));
    }

    fn run_unit(&mut self, tx: &UnboundedSender<AppEvent>) {
        let Some(u) = self.units.get(self.unit_sel) else {
            return;
        };
        let id = u.id.clone();
        self.spawn_run(tx, RunScope::Unit(id));
    }

    fn run_all(&mut self, tx: &UnboundedSender<AppEvent>) {
        self.spawn_run(tx, RunScope::All);
    }

    fn scroll_logs(&mut self, delta: i32) {
        let cur = self.log_scroll as i32 + delta;
        self.log_scroll = cur.clamp(0, self.logs.len().saturating_sub(1) as i32) as usize;
        self.log_auto = self.log_scroll + 1 >= self.logs.len();
    }

    /// 按当前界面滚动对应面板（Dump/Preview/日志）。
    fn scroll_current(&mut self, delta: i32) {
        if self.screen == Screen::Dump {
            self.dump_scroll = (self.dump_scroll as i32 + delta).max(0) as usize;
        } else if self.screen == Screen::Preview {
            if let Some(pv) = self.preview.as_mut() {
                pv.scroll = (pv.scroll as i32 + delta).max(0) as usize;
            }
        } else {
            self.scroll_logs(delta);
        }
    }

    fn handle_mouse(&mut self, m: MouseEvent, tx: &UnboundedSender<AppEvent>) {
        match m.kind {
            MouseEventKind::ScrollUp => {
                if let Some(action) = self.hit(m.column, m.row) {
                    match action {
                        MouseAction::SelectUnit(_) | MouseAction::SelectTask(..) => {
                            self.move_cursor(-1, tx)
                        }
                        MouseAction::Field(_) => {
                            self.settings.cursor = self.settings.cursor.saturating_sub(1)
                        }
                        _ => {
                            if self.screen == Screen::Dump {
                                self.dump_scroll = self.dump_scroll.saturating_sub(3);
                            } else if self.screen == Screen::Preview {
                                if let Some(pv) = self.preview.as_mut() {
                                    pv.scroll = pv.scroll.saturating_sub(3);
                                }
                            } else {
                                self.scroll_logs(-3);
                            }
                        }
                    }
                } else {
                    self.scroll_current(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if let Some(action) = self.hit(m.column, m.row) {
                    match action {
                        MouseAction::SelectUnit(_) | MouseAction::SelectTask(..) => {
                            self.move_cursor(1, tx)
                        }
                        MouseAction::Field(_) => {
                            self.settings.cursor =
                                (self.settings.cursor + 1).min(super::app::FIELDS.len() - 1)
                        }
                        _ => {
                            if self.screen == Screen::Dump {
                                self.dump_scroll += 3;
                            } else if self.screen == Screen::Preview {
                                if let Some(pv) = self.preview.as_mut() {
                                    pv.scroll += 3;
                                }
                            } else {
                                self.scroll_logs(3);
                            }
                        }
                    }
                } else {
                    self.scroll_current(3);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(action) = self.hit(m.column, m.row) else {
                    return;
                };
                match action {
                    MouseAction::SelectUnit(ui) => {
                        self.unit_sel = ui;
                        self.task_sel = None;
                        self.toggle_unit();
                    }
                    MouseAction::SelectTask(ui, ti) => {
                        self.unit_sel = ui;
                        self.task_sel = Some(ti);
                    }
                    MouseAction::ToggleUnit(ui) => {
                        self.unit_sel = ui;
                        self.task_sel = None;
                        self.toggle_unit();
                    }
                    MouseAction::Field(idx) => {
                        if self.screen == Screen::Settings {
                            self.settings.cursor = idx;
                        }
                    }
                    MouseAction::Button(id) => self.on_button(id, tx),
                }
            }
            _ => {}
        }
    }

    fn hit(&self, x: u16, y: u16) -> Option<MouseAction> {
        self.mouse_map
            .iter()
            .rev()
            .find(|(r, _)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            .map(|(_, a)| *a)
    }

    fn on_button(&mut self, id: ButtonId, tx: &UnboundedSender<AppEvent>) {
        match id {
            ButtonId::RunTask => self.run_task(tx),
            ButtonId::RunUnit => self.run_unit(tx),
            ButtonId::RunAll => self.run_all(tx),
            ButtonId::Preview => self.spawn_preview(tx),
            ButtonId::Dump => {
                self.screen = Screen::Dump;
                self.spawn_load_dump(tx);
            }
            ButtonId::DumpForce => self.spawn_dump(tx, true),
            ButtonId::CancelRun => {
                if let Some(run) = &self.run {
                    run.cancel.cancel();
                }
                self.status = "已请求取消…".into();
            }
            ButtonId::Settings => {
                self.settings = super::app::SettingsState::new(self.session.cfg());
                self.screen = Screen::Settings;
            }
            ButtonId::Refresh => self.refresh(tx),
            ButtonId::Help => self.screen = Screen::Help,
            ButtonId::Quit => self.quit = true,
            ButtonId::Save => self.save_config(tx),
            ButtonId::Back => self.screen = Screen::Dashboard,
            ButtonId::RunDump => self.spawn_dump(tx, false),
            ButtonId::Redump => self.spawn_dump_task(tx),
            ButtonId::GenDraft => {
                if let Some(pv) = &self.preview {
                    let gid = pv.gid.clone();
                    self.confirm = Some(Confirm {
                        prompt: "调用 LLM 生成讨论草稿？可能产生费用 (y/n)".into(),
                        action: ConfirmAction::GenerateDraft(gid),
                    });
                }
            }
        }
    }
}

fn preview_lines(p: &Preview) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(pretty) = &p.json_pretty {
        lines.push("=== 解密后完整 JSON ===".into());
        lines.extend(pretty.lines().map(|l| l.to_string()));
    }
    match &p.group {
        Some(group) => {
            for m in &group.modules {
                lines.push(String::new());
                lines.push(format!(
                    "[模块] type={} reply_type={} instance_id={}",
                    m.module_type, m.reply_type, m.instance_id
                ));
                if !m.direction.is_empty() {
                    lines.push(format!("【答题说明】{}", m.direction));
                }
                if !m.material.is_empty() {
                    lines.push(format!("【材料文本】({}字)", m.material.chars().count()));
                    lines.extend(m.material.lines().map(|l| l.to_string()));
                }
                if !m.transcript.is_empty() {
                    lines.push(format!("【内嵌字幕】({}字)", m.transcript.chars().count()));
                    lines.extend(m.transcript.lines().map(|l| l.to_string()));
                }
                if m.module_type == "vocabulary" {
                    lines.push(format!("【单词卡】共 {} 个单词", p.vocab.len()));
                    for w in &p.vocab {
                        lines.push(format!("  {} | {}", w.name, w.sound));
                    }
                }
                for c in &m.children {
                    lines.push(format!(
                        "  [{}] 题干: {}",
                        c.reply_type,
                        crate::api::parser::truncate_text(&c.question_text, 200)
                    ));
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
                                format!("{}: {}", label, crate::api::parser::truncate_text(&txt, 100))
                            })
                            .collect();
                        lines.push(format!("       选项: {}", opts.join(" ; ")));
                    }
                }
            }
        }
        None => {
            lines.push("[浏览类页面] 内容为空/非 JSON/无题目模块，run/group 将直接标记已看".into());
            let t = p.plain.trim();
            if !t.is_empty() {
                lines.push(format!("【原始内容】({}字)", p.plain.chars().count()));
                lines.extend(t.lines().take(500).map(|l| l.to_string()));
            }
        }
    }
    lines
}
