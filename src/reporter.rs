use std::sync::Arc;

/// dump-text 生成事件类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpKind {
    /// 新生成（含媒体转写）
    Generated,
    /// 浏览类页面（内容为空/非 JSON/无题目模块）
    ViewOnly,
    /// 已存在文件，仅刷新首行状态
    Refreshed,
}

/// 核心流程上报事件：CLI 由 PrintReporter 打印，TUI 转为界面事件。
#[derive(Debug, Clone)]
pub enum ReportEvent {
    /// 开始处理一个单元
    UnitStart {
        unit_id: String,
        label: Option<String>,
        total: usize,
        required_only: bool,
    },
    /// 开始作答某任务
    TaskStart { group_id: String, tab_type: String },
    /// 任务提交成功
    TaskDone {
        group_id: String,
        tab_type: String,
        detail: String,
    },
    /// 任务跳过（已通过等）
    TaskSkipped { group_id: String, reason: String },
    /// 任务失败
    TaskFailed {
        group_id: String,
        tab_type: String,
        error: String,
    },
    /// 提交命中限频，等待冷却
    RateLimited {
        attempt: u32,
        max: u32,
        wait_secs: u64,
        message: String,
    },
    /// dump-text 生成/刷新事件
    Dump {
        unit_id: String,
        group_id: String,
        kind: DumpKind,
        path: String,
        modules: usize,
        questions: usize,
        media: usize,
    },
    /// 运行结束
    RunFinished {
        done: u32,
        skipped: u32,
        failed: u32,
    },
    /// 纯文本输出行（保持 CLI 原样）
    Plain(String),
    Info(String),
    Warn(String),
    Error(String),
}

/// 事件接收方（必须 Send + Sync，便于在 tokio 任务间共享）。
pub trait Reporter: Send + Sync {
    fn report(&self, ev: ReportEvent);
}

/// 什么都不做（测试/内部用）。
pub struct SilentReporter;

impl Reporter for SilentReporter {
    fn report(&self, _ev: ReportEvent) {}
}

/// CLI 实现：保持原有输出格式（日志走 log 宏，dump 行走 println）。
pub struct PrintReporter;

impl Reporter for PrintReporter {
    fn report(&self, ev: ReportEvent) {
        match ev {
            ReportEvent::UnitStart {
                unit_id,
                label,
                total,
                required_only,
            } => {
                let suffix = if required_only { " (仅必修)" } else { "" };
                match label {
                    Some(l) => log::info!("单元 {} ({}) ：任务 {} 个{}", unit_id, l, total, suffix),
                    None => log::info!("单元 {} ：任务 {} 个{}", unit_id, total, suffix),
                }
            }
            ReportEvent::TaskDone {
                group_id,
                tab_type,
                detail,
            } => log::info!("[OK] {} {} -> {}", tab_type, group_id, detail),
            ReportEvent::TaskFailed {
                group_id,
                tab_type,
                error,
            } => log::error!("[FAIL] {} {} -> {}", tab_type, group_id, error),
            ReportEvent::RateLimited {
                attempt,
                max,
                wait_secs,
                message,
            } => log::warn!(
                "触发限频，等待 {} 秒后重试 ({}/{}): {}",
                wait_secs,
                attempt,
                max,
                message
            ),
            ReportEvent::Dump {
                group_id,
                kind,
                path,
                modules,
                questions,
                media,
                ..
            } => match kind {
                DumpKind::Generated => println!(
                    "任务组 {} -> {} (模块{} 题{} 媒体{})",
                    group_id, path, modules, questions, media
                ),
                DumpKind::ViewOnly => println!("任务组 {} -> {} (浏览类页面)", group_id, path),
                DumpKind::Refreshed => {}
            },
            ReportEvent::Plain(line) => println!("{}", line),
            ReportEvent::Info(msg) => log::info!("{}", msg),
            ReportEvent::Warn(msg) => log::warn!("{}", msg),
            ReportEvent::Error(msg) => log::error!("{}", msg),
            // TaskStart / TaskSkipped / RunFinished 静默（CLI 由调用方汇总输出）
            ReportEvent::TaskStart { .. }
            | ReportEvent::TaskSkipped { .. }
            | ReportEvent::RunFinished { .. } => {}
        }
    }
}

/// 转发给闭包的实现（TUI 用：把事件塞进 mpsc）。
pub struct FnReporter<F>(pub F);

impl<F: Fn(ReportEvent) + Send + Sync> Reporter for FnReporter<F> {
    fn report(&self, ev: ReportEvent) {
        (self.0)(ev)
    }
}

/// 可在多处共享的 reporter。
pub type SharedReporter = Arc<dyn Reporter>;
