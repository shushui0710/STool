//! 批量队列（P2-9）：对一个根目录下的多个游戏目录**依次**执行（默认只做安全的 detect），
//! 把"多游戏重复操作 N 遍"变成一条命令。
//!
//! 安全设计：
//! - 默认操作为 `detect`（纯只读）；
//! - 执行写操作时复用 P1-2 的环境预检，预检不通过则跳过该游戏而不是中断整批；
//! - 单个游戏失败不影响后续游戏（逐条记结果）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::engines::{Ctx, Op, Registry};

/// 判定"像游戏目录"的扩展名提示（命中其一即认为是候选游戏目录）。
const GAME_EXT_HINTS: &[&str] = &[
    "exe", "xp3", "pck", "rpa", "arc", "ald", "pfs", "pfs2", "asb", "rgss3a", "rgssad", "nsa", "swf",
];

/// 扫描时跳过的目录名（工具/系统目录，避免把 Cheat Engine、下载器等当成游戏）。
const SKIP_DIRS: &[&str] = &[
    "cheat engine",
    "mtool",
    "openspeedy",
    "tool",
    "tools",
    "bin",
    "$recycle.bin",
    "system volume information",
    "windows",
    "program files",
    "program files (x86)",
    "appdata",
];

/// 单个批处理条目的结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Item {
    /// 游戏目录（绝对或调用方给出的形式）。
    pub root: PathBuf,
    /// 引擎插件 id（未识别为空串）。
    pub engine_id: String,
    /// 引擎展示名。
    pub engine_name: String,
    /// 检测分数。
    pub score: i32,
    /// 置信度文字（高置信 / 已确认 / 疑似 / 未识别）。
    pub confidence: String,
    /// 已确认识别（分数过判定线）或操作执行成功。
    pub ok: bool,
    /// 证据 / 操作结果消息。
    pub message: String,
}

/// 递归扫描 `base`（深度受限），返回候选游戏目录。
///
/// 命中一个目录（含 `.exe` 或已知封包）后不再往下钻，避免把游戏内部的
/// `Data/`、`www/` 等子目录也当成独立游戏；跳过已知工具/系统目录。
pub fn discover(base: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if !base.is_dir() {
        return found;
    }
    let mut stack = vec![(base.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if looks_like_game(&dir) {
            found.push(dir);
            continue;
        }
        if depth >= max_depth {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if !is_dir {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            stack.push((e.path(), depth + 1));
        }
    }
    found.sort();
    found
}

fn looks_like_game(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    for e in rd.flatten() {
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Some(ext) = e.path().extension().and_then(|s| s.to_str()) {
            if GAME_EXT_HINTS.contains(&ext.to_ascii_lowercase().as_str()) {
                return true;
            }
        }
    }
    false
}

/// 对一个目录做一次检测（只读）。
pub fn detect_one(reg: &Registry, root: &Path) -> Item {
    let dets = reg.detect_all(root);
    match Registry::pick_from(&dets) {
        Some((d, confirmed)) => Item {
            root: root.to_path_buf(),
            engine_id: d.plugin_id.clone(),
            engine_name: d.name.clone(),
            score: d.score,
            confidence: d.confidence().label().to_string(),
            ok: confirmed,
            message: d.evidence.join("; "),
        },
        None => Item {
            root: root.to_path_buf(),
            engine_id: String::new(),
            engine_name: "未识别".into(),
            score: 0,
            confidence: "未识别".into(),
            ok: false,
            message: "未匹配到已知引擎".into(),
        },
    }
}

/// 对多个目录依次检测（只读）。`progress(当前序号, 总数, 目录)`。
pub fn detect_batch(roots: &[PathBuf], progress: &dyn Fn(usize, usize, &Path)) -> Vec<Item> {
    let reg = Registry::new();
    let n = roots.len();
    let mut out = Vec::with_capacity(n);
    for (i, root) in roots.iter().enumerate() {
        progress(i, n, root);
        out.push(detect_one(&reg, root));
    }
    out
}

/// 对多个目录依次执行同一操作。`out_base` 下按游戏目录名建子目录作为输出；
/// 未识别引擎或环境预检不通过的条目会被跳过（不影响其余游戏）。
pub fn run_batch(
    roots: &[PathBuf],
    op: Op,
    out_base: &Path,
    opts: &std::collections::HashMap<String, String>,
    progress: &dyn Fn(usize, usize, &Path),
) -> Vec<Item> {
    let reg = Registry::new();
    let n = roots.len();
    let cancel = AtomicBool::new(false);
    let mut out = Vec::with_capacity(n);
    for (i, root) in roots.iter().enumerate() {
        progress(i, n, root);
        let mut item = detect_one(&reg, root);
        // 未达判定线（含兜底的 generic）不执行，避免对非游戏目录乱写
        if !item.ok {
            item.message = "未确认识别，已跳过（可单独用 -p <id> 强制指定）".into();
            out.push(item);
            continue;
        }
        let out_dir = out_base.join(sanitize(root.file_name()));
        let res = run_one(&reg, &item.engine_id, op, root, &out_dir, opts, &cancel);
        item.ok = res.success;
        item.message = res.message;
        out.push(item);
    }
    out
}

fn run_one(
    reg: &Registry,
    det_id: &str,
    op: Op,
    root: &Path,
    out_dir: &Path,
    opts: &std::collections::HashMap<String, String>,
    cancel: &AtomicBool,
) -> crate::engines::OpOutcome {
    let Some(engine) = reg.get(det_id) else {
        return crate::engines::OpOutcome::fail("插件不存在");
    };
    // 复用 P1-2 环境预检：预检不通过就跳过（不中断整批）
    let scope = crate::features::precheck::scope_for_op(op, opts);
    let rep = crate::features::precheck::run(root, Some(out_dir), scope);
    if !rep.ok() {
        return crate::engines::OpOutcome::fail(rep.fail_summary());
    }
    let _ = std::fs::create_dir_all(out_dir);
    let noop = |_: f32, _: &str| {};
    let ctx = Ctx { root, out_dir, options: opts, progress: &noop, cancel };
    match op {
        Op::Extract => engine.extract(&ctx),
        Op::Decompile => engine.decompile(&ctx),
        Op::TextExtract => engine.text_extract(&ctx, out_dir),
        Op::Save => engine.save(&ctx),
        Op::Unlock => engine.unlock(&ctx),
        Op::Repack | Op::TextImport | Op::TextInject => {
            crate::engines::OpOutcome::fail("该操作需要额外输入/参数，暂不支持批量")
        }
    }
}

fn sanitize(name: Option<&std::ffi::OsStr>) -> String {
    let s = name.map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "game".into());
    s.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect()
}

/// 导出 CSV 报告（首行表头，含字段转义）。
pub fn to_csv(items: &[Item]) -> String {
    let mut s = String::from("目录,引擎ID,引擎,分数,置信度,已确认,备注\n");
    for it in items {
        s.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            csv_esc(&it.root.display().to_string()),
            csv_esc(&it.engine_id),
            csv_esc(&it.engine_name),
            it.score,
            csv_esc(&it.confidence),
            if it.ok { "是" } else { "否" },
            csv_esc(&it.message)
        ));
    }
    s
}

fn csv_esc(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 汇总一句话（总数 / 确认 / 未确认 / 引擎分布）。
pub fn summary(items: &[Item]) -> String {
    let total = items.len();
    let ok = items.iter().filter(|i| i.ok).count();
    let unknown = items.iter().filter(|i| !i.ok).count();
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for it in items {
        if it.ok && !it.engine_id.is_empty() {
            *by.entry(it.engine_id.as_str()).or_default() += 1;
        }
    }
    let dist = by.iter().map(|(k, v)| format!("{k}×{v}")).collect::<Vec<_>>().join("、");
    format!("共 {total} 个目录：确认 {ok}、未确认 {unknown}；引擎分布：{}", if dist.is_empty() { "（无）".into() } else { dist })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_batch_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    #[test]
    fn discover_finds_games_and_skips_tools() {
        let base = tmp("disc");
        touch(&base.join("game1/Game.exe"));
        touch(&base.join("nested/game2/data.xp3"));
        fs::create_dir_all(base.join("empty_plain")).unwrap();
        touch(&base.join("tools/Cheat Engine/ce.exe")); // 工具目录应跳过
        let found = discover(&base, 3);
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"game1".to_string()), "应发现 game1：{names:?}");
        assert!(names.contains(&"game2".to_string()), "应发现嵌套 game2：{names:?}");
        assert!(!names.iter().any(|n| n == "empty_plain"), "空目录不应算游戏");
        assert!(!names.iter().any(|n| n == "Cheat Engine"), "工具目录应被跳过");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn discover_stops_at_game_root() {
        let base = tmp("stop");
        touch(&base.join("thegame/Game.exe"));
        touch(&base.join("thegame/Data/Scripts.rvdata2")); // 游戏内部子目录
        let found = discover(&base, 5);
        assert_eq!(found.len(), 1, "命中游戏后不应再往下钻：{found:?}");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn detect_unrecognized_dir() {
        let d = tmp("unk");
        fs::create_dir_all(d.join("random")).unwrap();
        let reg = Registry::new();
        let it = detect_one(&reg, &d);
        // 兜底 generic 会返回一个结果，但分数不过判定线 → ok=false
        assert!(!it.ok, "空目录不应被确认识别（engine_id={}）", it.engine_id);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn csv_escapes_special_chars() {
        assert_eq!(csv_esc("a,b"), "\"a,b\"");
        assert_eq!(csv_esc("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_esc("plain"), "plain");
    }

    #[test]
    fn csv_has_header_and_rows() {
        let items = vec![Item {
            root: PathBuf::from("D:/g,1"),
            engine_id: "kirikiri".into(),
            engine_name: "KiriKiri / KAG".into(),
            score: 70,
            confidence: "已确认".into(),
            ok: true,
            message: "3 个 .xp3 封包".into(),
        }];
        let csv = to_csv(&items);
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].starts_with("目录,引擎ID"));
        assert!(lines[1].contains("\"D:/g,1\""), "含逗号的路径要被引号包住：{}", lines[1]);
        assert!(lines[1].contains("kirikiri"));
    }

    #[test]
    fn summary_counts() {
        let mk = |id: &str, ok: bool| Item {
            root: PathBuf::from("x"),
            engine_id: id.into(),
            engine_name: id.into(),
            score: 0,
            confidence: String::new(),
            ok,
            message: String::new(),
        };
        let items = vec![mk("kirikiri", true), mk("kirikiri", true), mk("godot", false), mk("", false)];
        let s = summary(&items);
        assert!(s.contains("共 4 个目录"));
        assert!(s.contains("确认 2"));
        assert!(s.contains("未确认 2"));
        assert!(s.contains("kirikiri×2"));
    }
}
