//! 备份还原与双档案切换：
//! - `.stool.bak` 一键还原（所有写回原文件的操作都会先留备份，这里提供还原入口）；
//! - repack 后的 原版/汉化 封包一键切换：把备份固化为 `.stool.orig` / `.stool.trans`
//!   两个稳定副本，切换时把其中一个写回正式文件（游戏无需改名、无需重装）。

use std::fs;
use std::path::{Path, PathBuf};

const BAK_SUFFIX: &str = ".stool.bak";
/// 参与双档案切换的封包类型（存档等备份不参与，避免误切）。
const TOGGLE_EXTS: [&str; 4] = ["xp3", "pck", "asar", "rpa"];

/// 递归扫描 root 下所有 *.stool.bak 备份文件。
pub fn find_backups(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.file_name().to_string_lossy().ends_with(BAK_SUFFIX))
        .map(|e| e.into_path())
        .collect();
    out.sort();
    out
}

/// 由备份路径推出原文件路径（`data.xp3.stool.bak` → `data.xp3`）。
fn original_of(bak: &Path) -> PathBuf {
    let name = bak.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = name.strip_suffix(BAK_SUFFIX).unwrap_or(&name);
    bak.with_file_name(stem)
}

/// 用 .stool.bak 还原单个文件（保留备份本身，可反复还原）。
pub fn restore_one(bak: &Path) -> Result<String, String> {
    if !bak.exists() {
        return Err(format!("备份不存在: {}", bak.display()));
    }
    let orig = original_of(bak);
    fs::copy(bak, &orig).map_err(|e| format!("还原 {} 失败: {e}", orig.display()))?;
    Ok(format!("已还原 {}（备份保留，可再次还原）", orig.display()))
}

/// 回填一键闭环（P2-5）：用「重新打包出的新封包」替换游戏里的原封包。
///
/// 覆盖前 `backup_once` 留底（**已存在则保留首次原版，绝不覆盖**），这次留底恰好也是
/// 双档案切换所需的 `.stool.bak`，所以替换完即可直接用 `archive-toggle` 在原版/汉化间切换。
/// `target` 必须由调用方显式指定，避免"认错封包"造成误替换。
pub fn apply_repack(target: &Path, new_archive: &Path) -> Result<String, String> {
    if !target.exists() {
        return Err(format!("原封包不存在: {}", target.display()));
    }
    if !new_archive.exists() {
        return Err(format!("新封包不存在: {}", new_archive.display()));
    }
    let same = match (target.canonicalize(), new_archive.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => target == new_archive,
    };
    if same {
        return Err("原封包与新封包是同一个文件".into());
    }
    let ext = target.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !TOGGLE_EXTS.contains(&ext.as_str()) {
        return Err(format!("只支持 {} 类型封包，收到 .{ext}", TOGGLE_EXTS.join("/")));
    }
    // 备份（首次原版；已存在则不覆盖）——同时满足双档案切换的前提
    let bak = crate::settings::backup_or_abort(target)?;
    crate::settings::ensure_parent(target);
    fs::copy(new_archive, target).map_err(|e| format!("写入 {} 失败: {e}", target.display()))?;
    let name = |p: &Path| p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(format!(
        "已用 {} 替换 {}（原版备份 {} 已保留，可用 archive-toggle 切换原版/汉化）",
        name(new_archive),
        name(target),
        bak.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    ))
}

/// 双档案切换：对 root 下所有可切换封包执行一次 原版 ↔ 汉化 翻转。
/// 首次调用会把当前 备份→.stool.orig、正式文件→.stool.trans 固化下来；
/// 之后每次调用把非激活副本写回正式文件，并用 .stool.side 记录当前激活态。
pub fn archive_toggle(root: &Path) -> Result<String, String> {
    let baks = find_backups(root);
    let mut done: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for bak in &baks {
        let ext = bak.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        // bak 文件名形如 data.xp3.stool.bak，其扩展名是 bak；用去掉后缀后的原文件判断类型
        let orig = original_of(bak);
        let orig_ext = orig.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = ext;
        if !TOGGLE_EXTS.contains(&orig_ext.as_str()) {
            skipped.push(orig.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
            continue;
        }
        if !orig.exists() {
            skipped.push(format!("{}（原文件缺失）", orig.display()));
            continue;
        }
        {
            let state = toggle_one(&orig, bak)?;
            done.push(format!("{} → 当前 {}", orig.display(), state))
        }
    }
    if done.is_empty() {
        return Err(format!(
            "没有可切换的封包（需要先对 {} 类型封包做过 repack，留下 .stool.bak 备份）{}",
            TOGGLE_EXTS.join("/"),
            if skipped.is_empty() { String::new() } else { format!("；已跳过 {} 个无关备份", skipped.len()) }
        ));
    }
    Ok(format!("已切换 {} 个封包：{}", done.len(), done.join("；")))
}

fn toggle_one(orig: &Path, bak: &Path) -> Result<&'static str, String> {
    let side = |suffix: &str| -> PathBuf {
        let mut n = orig.file_name().map(|s| s.to_os_string()).unwrap_or_default();
        n.push(suffix);
        orig.with_file_name(n)
    };
    let orig_side = side(".stool.orig");
    let trans_side = side(".stool.trans");
    let state_file = side(".stool.side");

    // 首次：固化两个稳定副本（orig=备份即原版，trans=当前正式文件即汉化版）
    if !orig_side.exists() {
        fs::copy(bak, &orig_side).map_err(|e| format!("固化原版副本失败: {e}"))?;
    }
    if !trans_side.exists() {
        fs::copy(orig, &trans_side).map_err(|e| format!("固化汉化副本失败: {e}"))?;
    }
    // 当前激活态（缺省认为汉化版在用）
    let cur = fs::read_to_string(&state_file).unwrap_or_else(|_| "trans".into());
    let next = if cur.trim() == "orig" { "trans" } else { "orig" };
    let src = if next == "orig" { &orig_side } else { &trans_side };
    fs::copy(src, orig).map_err(|e| format!("写回封包失败: {e}"))?;
    fs::write(&state_file, next).map_err(|e| format!("写入切换状态失败: {e}"))?;
    Ok(if next == "orig" { "原版" } else { "汉化" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_restore_one_roundtrip() {
        let dir = std::env::temp_dir().join(format!("stool_rst_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let orig = dir.join("data.xp3");
        let bak = dir.join("data.xp3.stool.bak");
        fs::write(&orig, b"translated").unwrap();
        fs::write(&bak, b"original").unwrap();
        let msg = restore_one(&bak).unwrap();
        assert!(msg.contains("data.xp3"));
        assert_eq!(fs::read(&orig).unwrap(), b"original"); // 已还原
        assert_eq!(fs::read(&bak).unwrap(), b"original"); // 备份保留
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_archive_toggle() {        let dir = std::env::temp_dir().join(format!("stool_tgl_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let orig = dir.join("data.xp3");
        let bak = dir.join("data.xp3.stool.bak");
        fs::write(&bak, b"original").unwrap();
        fs::write(&orig, b"translated").unwrap();
        // 无关备份应被跳过
        let save = dir.join("Save01.rpgsave.stool.bak");
        fs::write(&save, b"save").unwrap();

        let m1 = archive_toggle(&dir).unwrap(); // trans → orig
        assert!(m1.contains("原版"));
        assert_eq!(fs::read(&orig).unwrap(), b"original");
        let m2 = archive_toggle(&dir).unwrap(); // orig → trans
        assert!(m2.contains("汉化"));
        assert_eq!(fs::read(&orig).unwrap(), b"translated");
        // 存档备份未被当成封包
        assert_eq!(fs::read(&save).unwrap(), b"save");
        let _ = fs::remove_dir_all(&dir);
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_rp_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn apply_repack_replaces_and_backs_up() {
        let d = tmp("apply");
        let orig = d.join("data.xp3");
        let new = d.join("stool_repack.xp3");
        fs::write(&orig, b"original").unwrap();
        fs::write(&new, b"translated").unwrap();

        let msg = apply_repack(&orig, &new).unwrap();
        assert!(msg.contains("data.xp3"));
        assert_eq!(fs::read(&orig).unwrap(), b"translated"); // 已替换
        // 备份即首次原版，且恰好满足双档案切换
        assert_eq!(fs::read(orig.with_file_name("data.xp3.stool.bak")).unwrap(), b"original");
        let m = archive_toggle(&d).unwrap();
        assert!(m.contains("原版"));
        assert_eq!(fs::read(&orig).unwrap(), b"original");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn apply_repack_backup_never_overwritten() {
        let d = tmp("bakkeep");
        let orig = d.join("a.rpa");
        fs::write(&orig, b"v1").unwrap();
        for (payload, tag) in [(b"v2", "n2"), (b"v3", "n3")] {
            let new = d.join(format!("{tag}.rpa"));
            fs::write(&new, payload).unwrap();
            apply_repack(&orig, &new).unwrap();
        }
        assert_eq!(fs::read(&orig).unwrap(), b"v3");
        // 备份必须仍是首次的原版 v1，不能被后来的替换覆盖
        assert_eq!(fs::read(orig.with_file_name("a.rpa.stool.bak")).unwrap(), b"v1");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn apply_repack_rejects_same_and_unsupported() {
        let d = tmp("reject");
        let orig = d.join("data.xp3");
        fs::write(&orig, b"x").unwrap();
        assert!(apply_repack(&orig, &orig).is_err(), "同一文件应被拒绝");
        let txt = d.join("readme.txt");
        fs::write(&txt, b"y").unwrap();
        assert!(apply_repack(&txt, &orig).is_err(), "不支持的扩展名应被拒绝");
        let missing = d.join("nope.rpa");
        assert!(apply_repack(&missing, &orig).is_err(), "原封包缺失应被拒绝");
        let _ = fs::remove_dir_all(&d);
    }
}
