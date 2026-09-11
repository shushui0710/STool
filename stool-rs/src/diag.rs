//! 诊断支持：全局 panic 钩子 + 日志落盘 + 后台任务 panic 捕获。
//!
//! 解决的问题：
//! - 之前 GUI 后台线程 panic 后只剩一句「任务线程异常退出」，既无位置也无原因，
//!   用户和开发者都无法定位（release 版还看不到控制台）。
//! - 现在任何线程的 panic 都会写入 `~/.stool/logs/stool-YYYY-MM-DD.log`，
//!   含时间、线程名、panic 消息与 `file:line:col` 位置。
//! - 后台任务线程用 `catch_unwind` 把 panic 转成可展示的 `String` 错误，
//!   而不是让整个任务静默消失。

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// 串行化日志写入，避免多线程交错破坏行结构。
static LOG_LOCK: Mutex<()> = Mutex::new(());

/// 日志目录：`<家目录>/.stool/logs`。
pub fn log_dir() -> PathBuf {
    let base = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(".stool").join("logs")
}

/// 当天日志文件路径。
pub fn log_path() -> PathBuf {
    log_dir().join(format!("stool-{}.log", today()))
}

/// 向日志追加一行：`[YYYY-MM-DD HH:MM:SS] [LEVEL] msg`。
/// 失败静默（日志本身不应成为新的故障源）。
pub fn log(level: &str, msg: &str) {
    let line = format!("[{}] [{}] {}\n", now_stamp(), level, msg);
    let _g = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = log_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = f.write_all(line.as_bytes());
    }
    // debug 构建同时打到 stderr，开发期可直接看到
    #[cfg(debug_assertions)]
    eprint!("{line}");
}

/// 安装全局 panic 钩子，并记录一次启动日志。应在程序入口最早调用。
pub fn init() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<未知位置>".to_string());
        let thread = std::thread::current()
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| "<未命名线程>".to_string());
        let reason = panic_message(info);
        log("PANIC", &format!("线程={thread} 位置={loc} 原因={reason}"));
        default_hook(info);
    }));
    log("INFO", &format!("STool {} 启动（日志: {}）", crate::VERSION, log_path().display()));
}

/// 把 `catch_unwind` 捕获到的 panic 载荷转成人类可读字符串。
/// 后台任务线程用它把 panic 变成可展示的错误信息。
pub fn panic_to_string(e: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        "未知 panic 载荷".to_string()
    }
}

/// 在后台任务中执行 `f`，把 panic 折成 `Result<_, String>`：
/// - `Ok(v)` 正常返回；
/// - `Err(msg)` 表示发生 panic，`msg` 含原因与位置，可直接展示给用户。
pub fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    let hook_installed = std::panic::take_hook();
    // 捕panic时避免默认钩子重复打印（我们已在日志中记录）
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::panic::set_hook(hook_installed);
    match r {
        Ok(v) => Ok(v),
        Err(e) => {
            let msg = format!("[{what}] 内部错误（已记录日志）: {}", panic_to_string(e));
            log("ERROR", &msg);
            Err(msg)
        }
    }
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "未知 panic 载荷".to_string()
    }
}

// ---------- 无依赖的日期/时间格式化 ----------

fn now_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

pub fn today() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 当前时间的紧凑时间戳（用于导出文件名）：`YYYYMMDD-HHMMSS`。
pub fn stamp_compact() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Howard Hinnant 的 days → (year, month, day) 算法（本地时区近似为 UTC）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1)); // 2024-01-01
    }

    #[test]
    fn log_writes_line() {
        log("TEST", "hello diag");
        let p = log_path();
        assert!(p.exists());
        let content = std::fs::read_to_string(&p).unwrap_or_default();
        assert!(content.contains("hello diag"));
    }

    #[test]
    fn guard_converts_panic() {
        let r: Result<(), String> = guard("unit", || panic!("boom"));
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("boom"));
    }
}
