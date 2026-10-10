use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use time::PrimitiveDateTime;
use time::macros::format_description;

/// 日志目录（相对工作目录）。
pub const LOG_DIR: &str = "logs";
const FILE_PREFIX: &str = "unipus-";
const RETENTION_HOURS: i64 = 72;

/// TUI 是否正在运行（用于屏蔽第三方库的 stdout/stderr 直接输出，避免破坏界面）。
static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 标记 TUI 已启动。
pub fn set_tui_active() {
    TUI_ACTIVE.store(true, Ordering::Relaxed);
}

/// 取消 TUI 标记（测试用）。
pub fn reset_tui_active() {
    TUI_ACTIVE.store(false, Ordering::Relaxed);
}

/// 当前是否运行在 TUI 中。
pub fn tui_active() -> bool {
    TUI_ACTIVE.load(Ordering::Relaxed)
}

/// 当前时间：本地时间；受限环境（如多线程 Linux 无法安全取本地偏移）回退 UTC。
/// 返回 (时间, 是否为本地时间)。
pub fn now() -> (time::OffsetDateTime, bool) {
    match time::OffsetDateTime::now_local() {
        Ok(t) => (t, true),
        Err(_) => (time::OffsetDateTime::now_utc(), false),
    }
}

/// `YYYYMMDD-HHMMSS`
pub fn stamp_compact(t: time::OffsetDateTime) -> String {
    let fmt = format_description!("[year][month][day]-[hour][minute][second]");
    t.format(&fmt).unwrap_or_default()
}

/// `YYYY-MM-DD HH:MM:SS`
pub fn stamp_human(t: time::OffsetDateTime) -> String {
    let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    t.format(&fmt).unwrap_or_default()
}

/// 本次运行日志文件名（本地时间）。
pub fn base_log_name(t: time::OffsetDateTime) -> String {
    format!("{}{}.log", FILE_PREFIX, stamp_compact(t))
}

/// 单个运行日志文件（多线程写入用 Mutex 保护）。
pub struct RunLog {
    pub path: PathBuf,
    file: Mutex<File>,
}

impl RunLog {
    /// 追加一行（自动补换行）。
    pub fn write_line(&self, line: &str) {
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "{}", line);
        }
    }

    /// 追加原始字节（env_logger tee 用，行格式由 env_logger 负责）。
    pub fn write_bytes(&self, buf: &[u8]) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(buf);
        }
    }

    pub fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

/// 唯一日志路径：同秒冲突时追加 `-2/-3…`。
fn unique_log_path(dir: &Path, t: time::OffsetDateTime) -> PathBuf {
    let base = base_log_name(t);
    let mut path = dir.join(&base);
    let mut n = 2;
    while path.exists() {
        let stem = base.trim_end_matches(".log");
        path = dir.join(format!("{}-{}.log", stem, n));
        n += 1;
    }
    path
}

/// 创建日志目录、清理过期日志、创建本次运行日志文件。
pub fn prepare() -> Result<RunLog> {
    let dir = Path::new(LOG_DIR);
    fs::create_dir_all(dir).with_context(|| format!("创建日志目录失败: {}", dir.display()))?;
    let (t, _) = now();
    let _ = cleanup_expired(dir, t);
    let path = unique_log_path(dir, t);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("创建日志文件失败: {}", path.display()))?;
    Ok(RunLog {
        path,
        file: Mutex::new(file),
    })
}

/// 删除 `logs/` 下 `unipus-*.log` 中超过 72 小时的文件；返回删除数量。
/// 文件名无法解析或时间在未来的文件保留。
pub fn cleanup_expired(dir: &Path, now_t: time::OffsetDateTime) -> usize {
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    let fmt = format_description!("[year][month][day]-[hour][minute][second]");
    let now_prim = PrimitiveDateTime::new(now_t.date(), now_t.time());
    let mut removed = 0;
    for entry in rd.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(FILE_PREFIX) || !name.ends_with(".log") {
            continue;
        }
        let rest = &name[FILE_PREFIX.len()..];
        if rest.len() < 15 {
            continue;
        }
        let Ok(ts) = PrimitiveDateTime::parse(&rest[..15], &fmt) else {
            continue;
        };
        if now_prim < ts {
            continue; // 未来时间，保留
        }
        if (now_prim - ts).whole_hours() > RETENTION_HOURS && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "unipus_log_test_{}_{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn log_file_name_uses_local_format() {
        let t = datetime!(2026-09-23 19:05:12).assume_utc();
        assert_eq!(base_log_name(t), "unipus-20260923-190512.log");
    }

    #[test]
    fn cleanup_removes_only_expired_unipus_logs() {
        let dir = temp_dir("cleanup");
        let old = dir.join("unipus-20260918-120000.log");
        let recent = dir.join("unipus-20260923-100000.log");
        let other = dir.join("foo.log");
        let bad = dir.join("unipus-badname.log");
        for f in [&old, &recent, &other, &bad] {
            fs::write(f, "x").unwrap();
        }
        let now = datetime!(2026-09-23 19:05:12).assume_utc();
        let removed = cleanup_expired(&dir, now);
        assert_eq!(removed, 1);
        assert!(!old.exists(), "5 天前的日志应被删除");
        assert!(recent.exists(), "当天日志应保留");
        assert!(other.exists(), "非 unipus-*.log 应保留");
        assert!(bad.exists(), "无法解析的文件名应保留");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_path_adds_suffix_on_collision() {
        let dir = temp_dir("collision");
        let t = datetime!(2026-09-23 19:05:12).assume_utc();
        let p1 = unique_log_path(&dir, t);
        fs::write(&p1, "x").unwrap();
        let p2 = unique_log_path(&dir, t);
        assert_ne!(p1, p2);
        assert!(p2.to_string_lossy().ends_with("-2.log"));
        let _ = fs::remove_dir_all(&dir);
    }
}
