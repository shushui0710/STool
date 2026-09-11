//! 环境预检（P1-2）：执行写操作**之前**检查 目录可写 / 文件被占用 / 磁盘空间，
//! 失败时给出「原因 + 修法」，避免任务跑到一半才失败（几 GB 解包尤其致命）。
//!
//! 设计要点：
//! - 纯函数式、无副作用地「探测」而不是「假设」：可写性用真实建删临时文件确认，
//!   文件占用用带写权限的 open 探测（只认共享冲突 32/33，不误报权限拒绝 5）。
//! - 分级 `Ok/Warn/Fail`：只有 `Fail` 会中止操作；`Warn` 仅提示（例如游戏还开着）。
//! - 平台无关编译：磁盘空间在 Windows 走 `GetDiskFreeSpaceExW`，其他平台跳过（不误报）。

use std::path::Path;

/// 预检结论级别。序关系 `Ok < Warn < Fail`，便于取最严重项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    fn mark(self) -> &'static str {
        match self {
            Level::Ok => "✔",
            Level::Warn => "⚠",
            Level::Fail => "✘",
        }
    }

    /// 会中止操作的级别。
    pub fn blocks(self) -> bool {
        self == Level::Fail
    }
}

/// 单项预检结果。
#[derive(Debug, Clone)]
pub struct Item {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
    /// 出问题时的修复建议（`Ok` 项为 None）。
    pub fix: Option<String>,
}

/// 预检范围：本次操作要往哪里写。
#[derive(Debug, Clone, Copy)]
pub struct Scope {
    /// 会写回游戏目录本身（注入 / 存档 / 解锁 / 封包）。
    pub write_root: bool,
    /// 会往输出目录写（解包 / 反编译 / 文本导出）。
    pub write_out: bool,
}

impl Scope {
    pub fn out_only() -> Self {
        Scope { write_root: false, write_out: true }
    }
    pub fn root_only() -> Self {
        Scope { write_root: true, write_out: false }
    }
    pub fn both() -> Self {
        Scope { write_root: true, write_out: true }
    }
    /// 不落盘（仅联网/只读操作）：只做目录有效性检查。
    pub fn read_only() -> Self {
        Scope { write_root: false, write_out: false }
    }
}

/// 预检报告。
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub items: Vec<Item>,
}

impl Report {
    /// 最严重项的级别（空报告视为 `Ok`）。
    pub fn worst(&self) -> Level {
        self.items.iter().map(|i| i.level).max().unwrap_or(Level::Ok)
    }

    /// 是否可以继续执行（无 `Fail`）。
    pub fn ok(&self) -> bool {
        !self.worst().blocks()
    }

    /// 第一处会中止的问题。
    pub fn first_fail(&self) -> Option<&Item> {
        self.items.iter().find(|i| i.level.blocks())
    }

    /// 人类可读的多行文本（CLI 打印 / GUI 日志）。
    pub fn lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        for it in &self.items {
            v.push(format!("{} {}: {}", it.level.mark(), it.name, it.detail));
            if let Some(fix) = &it.fix {
                v.push(format!("    修法: {fix}"));
            }
        }
        v
    }

    /// 一句话结论（用于 toast / `OpOutcome::fail`）——只取失败项。
    pub fn fail_summary(&self) -> String {
        match self.first_fail() {
            Some(it) => match &it.fix {
                Some(fix) => format!("环境预检未通过：{}（{}）；修法：{fix}", it.name, it.detail),
                None => format!("环境预检未通过：{}（{}）", it.name, it.detail),
            },
            None => "环境预检通过".to_string(),
        }
    }
}

/// 执行预检。`out_dir` 为 None 时跳过输出目录相关检查。
pub fn run(root: &Path, out_dir: Option<&Path>, scope: Scope) -> Report {
    let mut items = Vec::new();

    // 1) 游戏目录必须存在且为目录——否则后面一切都免谈。
    if !root.exists() {
        items.push(Item {
            name: "游戏目录",
            level: Level::Fail,
            detail: format!("不存在：{}", root.display()),
            fix: Some("在首页重新选择有效的游戏根目录".into()),
        });
        return Report { items };
    }
    if !root.is_dir() {
        items.push(Item {
            name: "游戏目录",
            level: Level::Fail,
            detail: format!("不是目录：{}", root.display()),
            fix: Some("请选择「文件夹」而不是单个文件".into()),
        });
        return Report { items };
    }
    items.push(Item { name: "游戏目录", level: Level::Ok, detail: "存在".into(), fix: None });

    // 2) 输出目录：能建、能写。
    if scope.write_out {
        if let Some(out) = out_dir {
            if let Err(e) = std::fs::create_dir_all(out) {
                items.push(Item {
                    name: "输出目录",
                    level: Level::Fail,
                    detail: format!("无法创建：{}（{}）", out.display(), io_reason(&e)),
                    fix: Some(io_fix(&e, "输出目录")),
                });
            } else {
                match probe_writable(out) {
                    Ok(()) => items.push(Item {
                        name: "输出目录可写",
                        level: Level::Ok,
                        detail: format!("可写：{}", out.display()),
                        fix: None,
                    }),
                    Err(e) => items.push(Item {
                        name: "输出目录可写",
                        level: Level::Fail,
                        detail: format!("不可写：{}（{}）", out.display(), io_reason(&e)),
                        fix: Some(io_fix(&e, "输出目录")),
                    }),
                }
            }
        }
    }

    // 3) 游戏目录可写（仅当本次操作要写回游戏目录）。
    if scope.write_root {
        match probe_writable(root) {
            Ok(()) => items.push(Item { name: "游戏目录可写", level: Level::Ok, detail: "可写".into(), fix: None }),
            Err(e) => items.push(Item {
                name: "游戏目录可写",
                level: Level::Fail,
                detail: format!("不可写（{}）", io_reason(&e)),
                fix: Some(io_fix(&e, "游戏目录")),
            }),
        }
    }

    // 4) 磁盘空间：以「要写入的位置」所在卷为准，阈值给足解包余量。
    let vol = if scope.write_out { out_dir.unwrap_or(root) } else { root };
    if let Some(free) = free_space_bytes(vol) {
        let level = classify_space(free);
        items.push(Item {
            name: "磁盘空间",
            level,
            detail: format!("{} 可用 {}", vol.display(), human_bytes(free)),
            fix: if level.blocks() {
                Some("清理该磁盘，或把输出目录改到空间充足的盘".into())
            } else if level == Level::Warn {
                Some("大型封包解包建议预留数倍原包体积的空间".into())
            } else {
                None
            },
        });
    }

    // 5) 文件占用：写回游戏目录时，探测可能被占用的资源/程序文件。
    if scope.write_root {
        let locked = probe_locks(root, 8);
        if locked.is_empty() {
            items.push(Item { name: "文件占用", level: Level::Ok, detail: "未见占用".into(), fix: None });
        } else {
            items.push(Item {
                name: "文件占用",
                level: Level::Warn,
                detail: format!("{} 个文件疑似被占用：{}", locked.len(), locked.join("、")),
                fix: Some("关闭正在运行的游戏 / 占用这些文件的程序（含杀软扫描）后重试".into()),
            });
        }
    }

    // 6) 路径过长（Windows MAX_PATH 260）——打包资源常见坑，仅提示。
    let len = root.as_os_str().len();
    if len > 240 {
        items.push(Item {
            name: "路径长度",
            level: Level::Warn,
            detail: format!("游戏目录路径较长（{len} 字符），接近 Windows 上限 260"),
            fix: Some("把游戏移动到更短的路径（如 D:\\games\\xxx）以避免写盘失败".into()),
        });
    }

    Report { items }
}

/// 快速判定：返回第一处会中止的问题摘要；全部通过返回 None。
/// 供 GUI 在启动任务前一行调用。
pub fn blocking(root: &Path, out_dir: Option<&Path>, scope: Scope) -> Option<String> {
    let r = run(root, out_dir, scope);
    if r.ok() { None } else { Some(r.fail_summary()) }
}

/// 依据操作类型推断预检范围：导出类只写输出目录；回填/注入类写回游戏目录。
pub fn scope_for_op(op: crate::engines::Op, opts: &std::collections::HashMap<String, String>) -> Scope {
    match op {
        crate::engines::Op::Extract
        | crate::engines::Op::Decompile
        | crate::engines::Op::TextExtract
        | crate::engines::Op::TextImport => Scope::out_only(),
        crate::engines::Op::TextInject | crate::engines::Op::Save | crate::engines::Op::Repack => Scope::root_only(),
        // 解锁默认只读预览，只有 apply=1 才写回
        crate::engines::Op::Unlock => {
            if opts.get("apply").map(|v| v == "1").unwrap_or(false) {
                Scope::root_only()
            } else {
                Scope::read_only()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 探测原语
// ---------------------------------------------------------------------------

/// 真实建删一个临时探针文件，确认目录可写。
pub(crate) fn probe_writable(dir: &Path) -> std::io::Result<()> {
    let probe = dir.join(format!(".stool_write_probe_{}", std::process::id()));
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&probe)?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// 列出被其他进程占用的「热门」资源/程序文件（上限 `max` 个）。
fn probe_locks(root: &Path, max: usize) -> Vec<String> {
    const HOT: &[&str] = &[
        "exe", "dll", "pak", "xp3", "pck", "arc", "dat", "rpa", "rvdata2", "ald", "nsa", "bsa", "pfs", "bin", "int",
    ];
    let mut locked = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else { return locked };
    for e in rd.flatten() {
        if locked.len() >= max {
            break;
        }
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let hot = p
            .extension()
            .and_then(|s| s.to_str())
            .map(|x| HOT.contains(&x.to_ascii_lowercase().as_str()))
            .unwrap_or(false);
        if hot && is_locked(&p) {
            let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            locked.push(name);
        }
    }
    locked
}

/// 带写权限 open 探测：只把 Windows 共享冲突（32）视为「被占用」，
/// 权限拒绝（5）另有原因，不在占用检查里上报，避免误报。
fn is_locked(p: &Path) -> bool {
    match std::fs::OpenOptions::new().read(true).write(true).open(p) {
        Ok(_) => false,
        Err(e) => matches!(e.raw_os_error(), Some(32) | Some(33)),
    }
}

/// 磁盘剩余空间（字节）。非 Windows 或查询失败返回 None。
#[cfg(windows)]
fn free_space_bytes(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut free: u64 = 0;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: wide 是以 NUL 结尾的合法宽字符串；三个出参均为有效指针或 null。
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    if ok != 0 { Some(free) } else { None }
}

#[cfg(not(windows))]
fn free_space_bytes(_dir: &Path) -> Option<u64> {
    None
}

/// 磁盘空间分级：<100MB 直接失败，<1GB 警告，否则通过。
fn classify_space(free: u64) -> Level {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    if free < 100 * MB {
        Level::Fail
    } else if free < GB {
        Level::Warn
    } else {
        Level::Ok
    }
}

/// 把常见 IO 错误翻译成「人话原因」。
fn io_reason(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(5) => "拒绝访问（权限不足）".into(),
        Some(3) => "路径不存在".into(),
        Some(32) | Some(33) => "文件被其他程序占用".into(),
        Some(112) => "磁盘空间不足".into(),
        Some(206) => "文件名或路径过长".into(),
        _ => e.to_string(),
    }
}

/// 针对具体错误给修复建议。
fn io_fix(e: &std::io::Error, what: &str) -> String {
    match e.raw_os_error() {
        Some(5) => format!("以管理员身份运行；或把{what}换到当前用户可写的位置（如 D 盘自建目录）"),
        Some(32) | Some(33) => "关闭占用该文件的游戏 / 程序后重试".into(),
        Some(112) => "清理磁盘或换一个空间充足的盘".into(),
        Some(206) => "把游戏移动到更短的路径".into(),
        _ => format!("请检查{what}的权限与可用空间"),
    }
}

/// 人类可读的体积（保留一位小数）。供 selfcheck 等模块复用。
pub(crate) fn human_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let f = n as f64;
    if f >= GB {
        format!("{:.1} GB", f / GB)
    } else if f >= MB {
        format!("{:.1} MB", f / MB)
    } else if f >= KB {
        format!("{:.1} KB", f / KB)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("stool_precheck_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_root_is_fail() {
        let r = run(Path::new("Z:/definitely/not/here/stool"), None, Scope::both());
        assert!(!r.ok());
        assert_eq!(r.first_fail().unwrap().name, "游戏目录");
        assert!(r.fail_summary().contains("修法"));
    }

    #[test]
    fn healthy_dir_passes() {
        let d = tmpdir("ok");
        let out = d.join("out");
        let r = run(&d, Some(&out), Scope::both());
        assert!(r.ok(), "不应有 Fail：{:?}", r.lines());
        assert_eq!(r.worst(), Level::Ok);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn out_dir_is_created_and_probed() {
        let d = tmpdir("outmake");
        let out = d.join("nested/deep/out");
        let r = run(&d, Some(&out), Scope::out_only());
        assert!(out.is_dir(), "预检应把输出目录建出来");
        assert!(r.ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn probe_writable_succeeds_on_temp() {
        let d = tmpdir("probe");
        assert!(probe_writable(&d).is_ok());
        // 探针文件必须被清理干净
        let leftover = std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok()).any(|e| {
            e.file_name().to_string_lossy().starts_with(".stool_write_probe_")
        });
        assert!(!leftover, "探针文件没有清理");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn probe_writable_fails_on_missing_dir() {
        let missing = std::env::temp_dir().join(format!("stool_nope_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        assert!(probe_writable(&missing).is_err());
    }

    #[test]
    fn io_reason_maps_common_codes() {
        let denied = std::io::Error::from_raw_os_error(5);
        assert!(io_reason(&denied).contains("权限"));
        let busy = std::io::Error::from_raw_os_error(32);
        assert!(io_reason(&busy).contains("占用"));
        let full = std::io::Error::from_raw_os_error(112);
        assert!(io_reason(&full).contains("空间"));
    }

    #[test]
    fn space_levels() {
        assert_eq!(classify_space(10 * 1024 * 1024), Level::Fail);
        assert_eq!(classify_space(500 * 1024 * 1024), Level::Warn);
        assert_eq!(classify_space(50 * 1024 * 1024 * 1024), Level::Ok);
    }

    #[test]
    fn human_bytes_formats() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(2 * 1024 * 1024), "2.0 MB");
    }

    #[test]
    fn worst_and_ok_semantics() {
        let mut r = Report::default();
        assert!(r.ok());
        assert_eq!(r.worst(), Level::Ok);
        r.items.push(Item { name: "x", level: Level::Warn, detail: "w".into(), fix: None });
        assert!(r.ok(), "Warn 不应阻塞");
        r.items.push(Item { name: "y", level: Level::Fail, detail: "f".into(), fix: None });
        assert!(!r.ok());
        assert_eq!(r.worst(), Level::Fail);
    }

    #[test]
    fn blocking_returns_none_for_good_dir() {
        let d = tmpdir("block");
        assert!(blocking(&d, None, Scope::root_only()).is_none());
        assert!(blocking(&d.join("nope"), None, Scope::root_only()).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }
}
