use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Gauge, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};

use super::app::{App, ButtonId, FieldKind, FIELDS, MouseAction, Screen};

pub fn render(f: &mut Frame, app: &mut App) {
    app.mouse_map.clear();
    match app.screen {
        Screen::Dashboard => render_dashboard(f, app),
        Screen::Settings => render_settings(f, app),
        Screen::Dump => render_dump(f, app),
        Screen::Preview => render_preview(f, app),
        Screen::Help => {
            render_dashboard(f, app);
            render_help(f);
        }
    }
    if let Some(confirm) = &app.confirm {
        render_confirm(f, &confirm.prompt);
    }
}

fn render_dashboard(f: &mut Frame, app: &mut App) {
    let running = app.run.as_ref().is_some_and(|r| !r.finished);
    let top_h = if running { 4 } else { 3 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top_h),
            Constraint::Percentage(80),
            Constraint::Percentage(20),
            Constraint::Length(2),
        ])
        .split(f.area());

    render_top(f, app, chunks[0], running);

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(chunks[1]);
    render_tree(f, app, main[0]);
    render_detail(f, app, main[1]);
    render_logs(f, app, chunks[2]);

    // 底栏按钮
    let buttons: &[(ButtonId, &str)] = if running {
        &[
            (ButtonId::CancelRun, "Esc 取消"),
            (ButtonId::Dump, "d 导出"),
            (ButtonId::Quit, "q 退出"),
        ]
    } else {
        &[
            (ButtonId::RunTask, "Enter 运行"),
            (ButtonId::RunUnit, "R 本单元"),
            (ButtonId::RunAll, "A 全课程"),
            (ButtonId::Preview, "p 预览"),
            (ButtonId::Dump, "d 导出"),
            (ButtonId::Settings, "s 设置"),
            (ButtonId::Refresh, "r 刷新"),
            (ButtonId::Help, "? 帮助"),
            (ButtonId::Quit, "q 退出"),
        ]
    };
    render_footer(f, app, chunks[3], buttons, 14);
}

fn render_top(f: &mut Frame, app: &App, area: Rect, running: bool) {
    let mut total = 0usize;
    let mut done = 0usize;
    for u in &app.units {
        for t in &u.tasks {
            if t.required {
                total += 1;
                if t.passed {
                    done += 1;
                }
            }
        }
    }
    let strategy = if app.session.cfg().compulsory_only() {
        "仅必修"
    } else {
        "全部"
    };
    let loaded = app.units.iter().filter(|u| !u.tasks.is_empty()).count();
    let loading = if !app.units.is_empty() && loaded < app.units.len() {
        format!(" · 进度加载中 {}/{}", loaded, app.units.len())
    } else {
        String::new()
    };
    let title = format!(
        " UnipusAI · {} · 策略:{} · 必修进度 {}/{}{} ",
        app.course_name_display(),
        strategy,
        done,
        total,
        loading
    );
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(title))
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(if total == 0 {
            0.0
        } else {
            done as f64 / total as f64
        })
        .label(format!("{}/{}", done, total));
    f.render_widget(gauge, area);

    if running && let Some(run) = &app.run {
        let line = Line::from(vec![
            Span::styled(" 运行中: ", Style::default().fg(Color::Yellow)),
            Span::raw(run.scope.clone()),
            Span::raw(format!(
                "  done={} skipped={} failed={}",
                run.done, run.skipped, run.failed
            )),
            Span::raw(
                run.current
                    .as_ref()
                    .map(|(gid, t)| format!("  当前: {} {}", t, gid))
                    .unwrap_or_default(),
            ),
            Span::styled("   Esc 取消", Style::default().fg(Color::DarkGray)),
        ]);
        let para = Paragraph::new(line);
        let line_area = Rect {
            x: area.x + 1,
            y: area.y + area.height.saturating_sub(1),
            width: area.width.saturating_sub(2),
            height: 1,
        };
        f.render_widget(para, line_area);
    }
}

fn render_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let rows = app.rows();
    let sel = app.cursor();
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let inner_h = inner.height as usize;
    // 选中项尽量居中（顶部/底部贴边）
    let offset = sel
        .saturating_sub(inner_h / 2)
        .min(rows.len().saturating_sub(inner_h));
    let mut lines: Vec<Line> = Vec::new();
    for (i, (ui, ti)) in rows.iter().enumerate() {
        if i < offset || i >= offset + inner_h {
            continue;
        }
        let unit = app.units.get(*ui);
        let Some(unit) = unit else { continue };
        let text = match ti {
            None => {
                let label = unit
                    .label
                    .clone()
                    .unwrap_or_else(|| format!("Unit {}", *ui + 1));
                let (mut done, mut total) = (0, 0);
                for t in &unit.tasks {
                    if t.required {
                        total += 1;
                        if t.passed {
                            done += 1;
                        }
                    }
                }
                let mark = if unit.expanded { "▼" } else { "▶" };
                let loading = if unit.loading { " ⏳" } else { "" };
                format!("{} {} ({}/{}){}", mark, label, done, total, loading)
            }
            Some(ti) => {
                let t = match unit.tasks.get(*ti) {
                    Some(t) => t,
                    None => continue,
                };
                format!(
                    "   {} {} {} {}",
                    if t.passed { "✔" } else { "✘" },
                    pad(&t.tab_type, 6),
                    t.group_id,
                    if t.required { "必修" } else { "选修" }
                )
            }
        };
        let style = if i == sel {
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else if ti.is_some() {
            Style::default().fg(Color::Gray)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(text, style)));
        let rect = Rect {
            x: inner.x,
            y: inner.y + (i - offset) as u16,
            width: inner.width.saturating_sub(1),
            height: 1,
        };
        let action = match ti {
            Some(t) => MouseAction::SelectTask(*ui, *t),
            None => MouseAction::SelectUnit(*ui),
        };
        app.mouse_map.push((rect, action));
    }
    let block = Block::default().borders(Borders::ALL).title(" 课程结构 ");
    f.render_widget(Paragraph::new(lines).block(block), area);
    // 右侧滚动条（内容超出可视区时显示）
    if rows.len() > inner_h {
        let sb_area = Rect {
            x: inner.x + inner.width.saturating_sub(1),
            y: inner.y,
            width: 1,
            height: inner.height,
        };
        let mut sb_state = ScrollbarState::new(rows.len()).position(sel);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(Color::DarkGray)),
            sb_area,
            &mut sb_state,
        );
    }
}

fn render_detail(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some(task) = app.selected_task() {
        lines.push(Line::from(vec![
            Span::styled("任务: ", Style::default().fg(Color::DarkGray)),
            Span::raw(task.group_id.clone()),
        ]));
        lines.push(Line::from(format!(
            "类型: {}    必修: {}    状态: {}",
            task.tab_type,
            if task.required { "是" } else { "否" },
            if task.passed { "已完成" } else { "未完成" }
        )));
        lines.push(Line::from(format!("单元: {}", task.unit_id)));
        if task.end_time > 0 {
            lines.push(Line::from(format!(
                "截止: {}",
                fmt_ts(task.end_time)
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "按 Enter 运行 / f 强制 / p 预览",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from("选择左侧单元/任务查看详情"));
        lines.push(Line::from("Enter 展开单元，任务行 Enter 运行"));
    }
    if let Some(pv) = &app.preview
        && app.screen == Screen::Dashboard
    {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("最近预览: {}（p 再次预览）", pv.gid),
            Style::default().fg(Color::DarkGray),
        )));
    }
    let block = Block::default().borders(Borders::ALL).title(" 详情 ");
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn render_logs(f: &mut Frame, app: &App, area: Rect) {
    let inner_h = area.height.saturating_sub(2) as usize;
    let total = app.logs.len();
    let skip = app.log_scroll.min(total.saturating_sub(1)).min(
        total.saturating_sub(inner_h),
    );
    let mut lines: Vec<Line> = Vec::new();
    for log in app.logs.iter().skip(skip).take(inner_h) {
        let color = match log.level {
            log::Level::Error => Color::Red,
            log::Level::Warn => Color::Yellow,
            log::Level::Info => Color::Gray,
            _ => Color::DarkGray,
        };
        lines.push(Line::from(Span::styled(
            log.text.clone(),
            Style::default().fg(color),
        )));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" 日志 ({}/{}) ", skip + lines.len(), total));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// 底栏：上行为状态提示，下行为按钮。
fn render_footer(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    buttons: &[(ButtonId, &str)],
    each: u16,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    let status = if app.status.is_empty() {
        String::new()
    } else {
        format!(" {}", truncate_display(&app.status, area.width.saturating_sub(1) as usize))
    };
    f.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Yellow)),
        rows[0],
    );
    render_buttons(f, app, rows[1], buttons, each);
}

fn truncate_display(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut r: String = s.chars().take(max.saturating_sub(1)).collect();
        r.push('…');
        r
    }
}

fn render_buttons(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    buttons: &[(ButtonId, &str)],
    each: u16,
) {
    let mut constraints: Vec<Constraint> = buttons
        .iter()
        .map(|_| Constraint::Length(each))
        .collect();
    constraints.push(Constraint::Min(1));
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);
    for (i, (id, label)) in buttons.iter().enumerate() {
        let style = Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        let text = Line::from(Span::styled(format!(" {} ", label), style));
        let seg = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(each), Constraint::Min(0)])
            .split(cols[i]);
        let rect = Rect {
            x: seg[0].x,
            y: seg[0].y,
            width: each.saturating_sub(1),
            height: 1,
        };
        app.mouse_map.push((rect, MouseAction::Button(*id)));
        f.render_widget(Paragraph::new(text), seg[0]);
    }
}

fn render_settings(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(2)])
        .split(f.area());
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!(
            "配置文件: {}",
            app.session.config_path().display()
        ),
        Style::default().fg(Color::DarkGray),
    )));
    for (i, def) in FIELDS.iter().enumerate() {
        let selected = i == app.settings.cursor;
        let value = app.settings.value(i);
        let shown = match def.kind {
            FieldKind::Secret if !app.settings.reveal => mask(&value),
            _ => value.clone(),
        };
        let editing = selected && app.settings.editing;
        let text = if editing {
            format!("  {:<26} [{}]", def.label, app.settings.buf)
        } else {
            format!("  {:<26} {}", def.label, shown)
        };
        let style = if selected {
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(text, style)));
        let rect = Rect {
            x: chunks[0].x,
            y: chunks[0].y + 1 + i as u16,
            width: chunks[0].width,
            height: 1,
        };
        app.mouse_map.push((rect, MouseAction::Field(i)));
    }
    if let Some(msg) = &app.settings.msg {
        lines.push(Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Green),
        )));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 设置 config.json (Enter 编辑/切换, w 保存, v 显示敏感, Esc 返回) ");
    f.render_widget(Paragraph::new(lines).block(block), chunks[0]);
    render_footer(
        f,
        app,
        chunks[1],
        &[
            (ButtonId::Save, "w 保存"),
            (ButtonId::Refresh, "v 敏感"),
            (ButtonId::Back, "Esc 返回"),
            (ButtonId::Quit, "q 退出"),
        ],
        12,
    );
}

fn render_dump(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(2)])
        .split(f.area());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" dump-text 状态汇总 (Enter 执行导出, F 全量重建, r 刷新, Esc 返回) ");
    if app.dump_missing || app.dump_loading {
        // 提示态：水平居中 + 垂直居中
        let message = if app.dump_missing {
            "尚无 dump 数据：按 Enter 导出（F 全量重建）"
        } else {
            "正在读取 dump_text/dump.db …"
        };
        let inner = Rect {
            x: chunks[0].x + 1,
            y: chunks[0].y + 1,
            width: chunks[0].width.saturating_sub(2),
            height: chunks[0].height.saturating_sub(2),
        };
        f.render_widget(block, chunks[0]);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(45),
                Constraint::Length(1),
                Constraint::Percentage(45),
            ])
            .split(inner);
        f.render_widget(
            Paragraph::new(message)
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray)),
            rows[1],
        );
    } else {
        let text = app.dump_text.clone().unwrap_or_default();
        let lines: Vec<Line> = text
            .lines()
            .skip(app.dump_scroll)
            .map(|l| Line::from(l.to_string()))
            .collect();
        f.render_widget(Paragraph::new(lines).block(block), chunks[0]);
    }
    render_footer(
        f,
        app,
        chunks[1],
        &[
            (ButtonId::RunDump, "Enter 导出"),
            (ButtonId::DumpForce, "F 全量"),
            (ButtonId::Back, "Esc 返回"),
            (ButtonId::Quit, "q 退出"),
        ],
        14,
    );
}

fn render_preview(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(2)])
        .split(f.area());
    let mut lines: Vec<Line> = Vec::new();
    if let Some(pv) = &app.preview {
        if pv.loading {
            lines.push(Line::from("加载中…"));
        }
        lines.extend(
            pv.lines
                .iter()
                .skip(pv.scroll)
                .map(|l| Line::from(l.clone())),
        );
        if let Some(draft) = &pv.draft {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "── 讨论发言草稿 ──",
                Style::default().fg(Color::Yellow),
            )));
            lines.extend(draft.lines().map(|l| Line::from(l.to_string())));
        }
    }
    let title = app
        .preview
        .as_ref()
        .map(|p| format!(" 预览 {} (g 生成草稿, u 重抓, Esc 返回) ", p.gid))
        .unwrap_or_else(|| " 预览 ".to_string());
    let block = Block::default().borders(Borders::ALL).title(title);
    f.render_widget(Paragraph::new(lines).block(block), chunks[0]);
    render_footer(
        f,
        app,
        chunks[1],
        &[
            (ButtonId::GenDraft, "g 生成草稿"),
            (ButtonId::Redump, "u 重抓"),
            (ButtonId::Back, "Esc 返回"),
            (ButtonId::Quit, "q 退出"),
        ],
        14,
    );
}

fn render_help(f: &mut Frame) {
    let area = centered_rect(f.area(), 70, 70);
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from("UnipusAI TUI 帮助（任意键返回）"),
        Line::from(""),
        Line::from("↑/↓ j/k      移动选择"),
        Line::from("←/→ h/l      折叠/展开单元"),
        Line::from("Tab          切换焦点（树/详情/日志）"),
        Line::from("Enter        单元行=展开；任务行=运行（已通过需 f 强制）"),
        Line::from("f            强制运行选中任务（忽略已通过）"),
        Line::from("R / A        运行本单元 / 全课程"),
        Line::from("p / g        预览选中任务（库内数据） / 生成讨论草稿（需确认）"),
        Line::from("u            预览页：重新抓取当前任务并更新入库"),
        Line::from("d            dump-text 总览与导出（导出后自动刷新）"),
        Line::from("s / w       设置 / 保存配置"),
        Line::from("r / q        刷新 / 退出（运行中 Esc 取消）"),
        Line::from("鼠标：点击选择/按钮，滚轮滚动列表与日志"),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 帮助 ")
        .style(Style::default().bg(Color::Black));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_confirm(f: &mut Frame, prompt: &str) {
    let area = centered_rect(f.area(), 50, 5);
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 确认 ")
        .style(Style::default().bg(Color::Black).fg(Color::Yellow));
    f.render_widget(Paragraph::new(prompt.to_string()).block(block), area);
}

fn centered_rect(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn pad(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - count))
    }
}

fn mask(s: &str) -> String {
    if s.is_empty() {
        "(空)".to_string()
    } else {
        format!("●●●● (len={})", s.chars().count())
    }
}

fn fmt_ts(ts: i64) -> String {
    // 仅做粗略展示（本地时间不处理时区）
    let days = ts.div_euclid(86400);
    let rem = ts.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::course::GroupTask;
    use crate::api::session::Session;
    use crate::config::Config;
    use crate::tui::app::{App, PreviewState, UnitUi};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;

    fn test_app() -> App {
        let session = Session::new(Config::default(), PathBuf::from("config.json")).unwrap();
        App::new(session)
    }

    fn task(gid: &str, unit: &str, required: bool, passed: bool) -> GroupTask {
        GroupTask {
            group_id: gid.into(),
            unit_id: unit.into(),
            tab_type: "task".into(),
            required,
            passed,
            min_score_pct: 0,
            start_time: 0,
            end_time: 0,
        }
    }

    fn draw(app: &mut App) {
        let backend = TestBackend::new(110, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
    }

    /// 渲染一次并返回缓冲区文本（用于断言）。
    fn render_text(app: &mut App) -> String {
        let backend = TestBackend::new(110, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        buffer_text(terminal.backend())
    }

    /// 去掉全部空白（宽字符在缓冲区中会插入占位空格）。
    fn compact(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    fn buffer_text(backend: &TestBackend) -> String {
        let buf = backend.buffer();
        let area = buf.area;
        let mut text = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                text.push_str(buf[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn renders_all_screens_without_panic() {
        let mut app = test_app();
        draw(&mut app);

        app.screen = Screen::Settings;
        draw(&mut app);

        app.screen = Screen::Dump;
        app.dump_text = Some("dump-text 状态汇总\n任务组文件: 1".into());
        draw(&mut app);

        app.screen = Screen::Preview;
        app.preview = Some(PreviewState {
            gid: "g1".into(),
            unit_id: "u1".into(),
            lines: vec!["[浏览类页面] 无题目".into()],
            scroll: 0,
            draft: Some("draft text".into()),
            loading: false,
        });
        draw(&mut app);

        app.screen = Screen::Help;
        draw(&mut app);

        app.confirm = Some(crate::tui::app::Confirm {
            prompt: "确认？(y/n)".into(),
            action: crate::tui::app::ConfirmAction::QuitRunning,
        });
        app.screen = Screen::Dashboard;
        draw(&mut app);
    }

    #[test]
    fn dashboard_shows_loaded_unit_progress() {
        let mut app = test_app();
        app.units = vec![
            UnitUi {
                id: "u1".into(),
                label: Some("Unit A".into()),
                tasks: vec![
                    task("g1", "u1", true, true),
                    task("g2", "u1", true, false),
                    task("g3", "u1", false, false),
                ],
                expanded: true,
                loading: false,
                error: None,
            },
            UnitUi {
                id: "u2".into(),
                label: Some("Unit B".into()),
                tasks: vec![task("g4", "u2", true, false)],
                expanded: false,
                loading: false,
                error: None,
            },
        ];
        let text = render_text(&mut app);
        // 顶部课程进度统计所有已加载单元（必修 1/3；宽字符占位可能插入空单元格，按片段断言）
        assert!(text.contains("1/3"), "text:\n{}", text);
        // 树显示单元与任务状态
        assert!(text.contains("Unit A (1/2)"), "text:\n{}", text);
        assert!(text.contains("✔") && text.contains("✘"));
    }

    #[test]
    fn dump_screen_shows_missing_and_loading_states() {
        let mut app = test_app();
        app.screen = Screen::Dump;

        app.dump_missing = true;
        let text = compact(&render_text(&mut app));
        assert!(text.contains("尚无dump数据"), "text:\n{}", text);

        app.dump_missing = false;
        app.dump_loading = true;
        let text = compact(&render_text(&mut app));
        assert!(text.contains("正在读取"), "text:\n{}", text);

        app.dump_loading = false;
        app.dump_text = Some("dump-text 状态汇总".into());
        let text = compact(&render_text(&mut app));
        assert!(text.contains("dump-text状态汇总"), "text:\n{}", text);
    }

    #[test]
    fn footer_renders_status_line() {
        let mut app = test_app();
        app.status = "测试状态：已请求取消".into();
        let text = compact(&render_text(&mut app));
        assert!(text.contains("测试状态"), "text:\n{}", text);
    }

    #[test]
    fn tree_shows_scrollbar_when_overflow() {
        let mut app = test_app();
        let tasks = (0..40)
            .map(|i| task(&format!("g{}", i), "u1", true, false))
            .collect();
        app.units = vec![UnitUi {
            id: "u1".into(),
            label: Some("Unit A".into()),
            tasks,
            expanded: true,
            loading: false,
            error: None,
        }];
        let backend = TestBackend::new(110, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        // 左栏宽 42%（110 → 46），内区右缘一列（x=44）应绘制滚动条（█ 或 │）
        let x = 44u16;
        let found = (1..30u16).any(|y| {
            let s = buf[(x, y)].symbol();
            s == "█" || s == "│"
        });
        assert!(found, "未在 x={} 列发现滚动条", x);
    }
}
