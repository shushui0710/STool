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
// 只在非 Windows 的 UTC 回退路径里用到（Windows 走 GetLocalTime）。
#[cfg(not(windows))]
use std::time::{SystemTime, UNIX_EPOCH};

/// 串行化日志写入，避免多线程交错破坏行结构。
static LOG_LOCK: Mutex<()> = Mutex::new(());

/// 日志目录：`<家目录>/.stool/logs`。
///
/// 两条覆盖规则：
/// - 环境变量 `STOOL_LOG_DIR` 优先 —— 想把日志挪到别处不用改代码；
/// - **单测构建下恒定落到进程专属临时目录**。否则 `cargo test` 里那些
///   `boom` / `hello diag` / `读取失败 f3.bin` / `解包未完成…断点 200 条` /
///   `机翻第 1/3 次失败：HTTP 503` 的**用例数据**会混进界面的「运行日志」，
///   用户看着像自己干过这些操作（2026-09 实际踩到，用户报「莫名其妙的记录」）。
pub fn log_dir() -> PathBuf {
    log_dir_of(
        std::env::var_os("STOOL_LOG_DIR").map(PathBuf::from),
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from),
    )
}

/// [`log_dir`] 的纯逻辑：把环境变量当参数喂进来，才能在单测里钉死覆盖规则。
fn log_dir_of(override_dir: Option<PathBuf>, profile: Option<PathBuf>) -> PathBuf {
    if let Some(d) = override_dir {
        return d;
    }
    #[cfg(test)]
    {
        let _ = profile;
        std::env::temp_dir().join(format!("stool-test-logs-{}", std::process::id()))
    }
    #[cfg(not(test))]
    {
        profile.unwrap_or_else(std::env::temp_dir).join(".stool").join("logs")
    }
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
//
// ⚠️ **一律用本地时区**。这里曾经直接拿 `UNIX_EPOCH` 的秒数除以 86400 换算日期，
// 等于记 **UTC** —— 在东八区比用户墙钟慢整 8 小时：用户 20:31 点的「刷新」，
// 日志里写成 12:31，直觉上就是「时间对不上、操作也对不上」（2026-09 用户反馈）。
// Windows 下改走 `GetLocalTime`（含夏令时与半时区），非 Windows 回退 UTC。

/// 本地墙钟表示的「类 Unix 秒」= UTC 秒 + 时区偏移。
/// 日志里所有时间口径都从这一个函数出 —— 别再各自 `SystemTime::now()`。
#[cfg(windows)]
fn local_secs() -> i64 {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: SYSTEMTIME 全是 POD 字段；GetLocalTime 只填这一个出参。
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    days_from_civil(st.wYear as i64, st.wMonth as u32, st.wDay as u32) * 86_400
        + st.wHour as i64 * 3_600
        + st.wMinute as i64 * 60
        + st.wSecond as i64
}

/// 非 Windows 没有时区换算，退回 UTC（STool 目前只发 Windows 版）。
#[cfg(not(windows))]
fn local_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_stamp() -> String {
    stamp_from_secs(local_secs())
}

/// 把「类 Unix 秒」格式化成 `YYYY-MM-DD HH:MM:SS`。
/// 纯函数（不读时钟）：跨日 / 跨月 / 闰年 / 负秒这些边界才能钉死在单测里。
fn stamp_from_secs(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
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
    let (y, m, d) = civil_from_days(local_secs().div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// 当前时间的紧凑时间戳（用于导出文件名）：`YYYYMMDD-HHMMSS`。
pub fn stamp_compact() -> String {
    let secs = local_secs();
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Howard Hinnant 的 days → (year, month, day) 算法。
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

/// [`civil_from_days`] 的逆函数：(year, month, day) → days。
/// Windows 下把 `GetLocalTime` 的墙钟折成秒要用；非 Windows 只有单测用得到。
#[cfg_attr(not(windows), allow(dead_code))]
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
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
    fn civil_days_round_trip() {
        // 逆函数必须与正函数互为反函数（含 1970 之前、闰日 2024-02-29、跨世纪 2000-03-01）
        for days in [-31_000i64, -1, 0, 1, 11_016, 19_723, 19_782, 29_999] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "回环失败：{days} → {y}-{m:02}-{d:02}");
        }
    }

    #[test]
    fn stamp_from_secs_boundaries() {
        assert_eq!(stamp_from_secs(0), "1970-01-01 00:00:00");
        assert_eq!(stamp_from_secs(1_700_000_000), "2023-11-14 22:13:20");
        // 跨日：2024-03-01 的前一秒必须是 2024-02-29（闰年）
        assert_eq!(stamp_from_secs(1_709_251_199), "2024-02-29 23:59:59");
        assert_eq!(stamp_from_secs(1_709_251_200), "2024-03-01 00:00:00");
        // 负秒不 panic、不借位错（`div_euclid` / `rem_euclid` 语义）
        assert_eq!(stamp_from_secs(-1), "1969-12-31 23:59:59");
    }

    /// 独立再问一次 Windows 的本地墙钟（不复用 `local_secs` 的实现），
    /// 对不上就说明 `local_secs()` 没真的用本地时间 —— 例如退回成 `utc_secs()`，
    /// 在东八区会整整差 28800 秒；也能兜住 `SYSTEMTIME` 布局写错。
    #[cfg(windows)]
    #[test]
    fn local_secs_matches_win32_local_clock() {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut st) };
        let os_local = days_from_civil(st.wYear as i64, st.wMonth as u32, st.wDay as u32) * 86_400
            + st.wHour as i64 * 3_600
            + st.wMinute as i64 * 60
            + st.wSecond as i64;
        let diff = (os_local - local_secs()).abs();
        assert!(diff <= 2, "local_secs 与 Win32 本地时钟相差 {diff} 秒（应 ≤2）");
    }

    #[test]
    fn log_dir_honors_env_override() {
        let tmp = std::env::temp_dir();
        let ovr = tmp.join("stool-logs-override");
        let prof = tmp.join("fake-profile");
        assert_eq!(log_dir_of(Some(ovr.clone()), Some(prof)), ovr, "STOOL_LOG_DIR 应优先");
    }

    /// 回归：单测绝不能写到用户真实的日志目录 ——
    /// 否则 `cargo test` 的用例数据会出现在界面的「运行日志」里。
    #[test]
    fn tests_never_write_to_real_log_dir() {
        let tmp = std::env::temp_dir();
        let prof = tmp.join("fake-profile");
        let d = log_dir_of(None, Some(prof.clone()));
        assert!(!d.starts_with(&prof), "单测日志跑进了真实目录: {}", d.display());
        assert!(
            d.to_string_lossy().contains("stool-test-logs-"),
            "单测日志应在进程专属临时目录，实际: {}",
            d.display()
        );
    }

    #[test]
    fn log_writes_line() {
        log("TEST", "hello diag");
        let p = log_path();
        assert!(p.exists());
        let content = std::fs::read_to_string(&p).unwrap_or_default();
        assert!(content.contains("hello diag"));
        assert!(p.starts_with(std::env::temp_dir()), "日志落到不该去的地方: {}", p.display());
    }

    #[test]
    fn guard_converts_panic() {
        let r: Result<(), String> = guard("unit", || panic!("boom"));
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("boom"));
    }
}
