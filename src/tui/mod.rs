pub mod app;
pub mod events;
pub mod logger;
pub mod ui;
pub mod update;

use anyhow::Result;
use app::{App, AppEvent};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::api::session::Session;
use crate::logging::RunLog;

/// 启动交互式 TUI（无参数或 `--tui`）。
pub async fn run_tui(session: Session, log: Option<std::sync::Arc<RunLog>>) -> Result<()> {
    // 标记 TUI 模式：第三方库（如 whisper-candle）会直接打印 stdout/stderr，需静默
    crate::logging::set_tui_active();
    let mut stdout = std::io::stdout();
    enable_raw_mode()?;
    if let Err(e) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 异常退出时恢复终端
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        hook(info);
    }));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();
    logger::init(tx.clone(), log.clone());
    if let Some(l) = &log {
        log::info!("日志文件: {}", l.path.display());
    }
    events::spawn_input_thread(tx.clone());

    let need_course = session.course_id().is_empty();
    let mut app = App::new(session);
    if need_course {
        // 未选择课程：直接打开课程选择界面
        app.open_courses(&tx);
    } else {
        app.spawn_load_tree(&tx);
    }

    let mut tick = tokio::time::interval(std::time::Duration::from_millis(200));
    let result: Result<()> = loop {
        if let Err(e) = terminal.draw(|f| ui::render(f, &mut app)) {
            break Err(anyhow::anyhow!("TUI 绘制失败: {:#}", e));
        }
        tokio::select! {
            maybe = rx.recv() => {
                match maybe {
                    Some(ev) => app.handle(ev, &tx),
                    None => break Ok(()),
                }
            }
            _ = tick.tick() => app.handle(AppEvent::Tick, &tx),
        }
        if app.quit {
            break Ok(());
        }
    };

    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();
    result
}
