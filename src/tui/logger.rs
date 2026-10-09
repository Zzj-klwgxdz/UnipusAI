use std::sync::Arc;

use log::{LevelFilter, Log, Metadata, Record};
use tokio::sync::mpsc::UnboundedSender;

use super::app::AppEvent;
use crate::logging::RunLog;

/// 把 `log` 记录转发到 TUI 日志面板，同时写入本次运行日志文件（若有）。
pub struct TuiLogger {
    tx: UnboundedSender<AppEvent>,
    log: Option<Arc<RunLog>>,
}

impl Log for TuiLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if let Some(log) = &self.log {
            let (t, _) = crate::logging::now();
            log.write_line(&format!(
                "[{} {} {}] {}",
                crate::logging::stamp_human(t),
                record.level(),
                record.target(),
                record.args()
            ));
        }
        let _ = self.tx.send(AppEvent::Log {
            level: record.level(),
            text: format!("{}", record.args()),
        });
    }

    fn flush(&self) {}
}

/// 安装全局 logger（TUI 模式替代 env_logger）。
pub fn init(tx: UnboundedSender<AppEvent>, log: Option<Arc<RunLog>>) {
    let logger = TuiLogger { tx, log };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(LevelFilter::Info);
    }
}
