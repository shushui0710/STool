//! 补丁 / MOD 管理器：文件覆盖 + 备份清单，支持安装/卸载/启停。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEntry {
    pub name: String,
    pub files: Vec<String>,
    pub overwritten: Vec<String>,
    pub enabled: bool,
}

/// MOD 冲突（P2-10）：同一文件被多个**已启用** MOD 覆盖。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// 相对游戏根目录的路径（`/` 分隔）。
    pub rel: String,
    /// 争用该文件的 MOD 名（已排序）。
    pub mods: Vec<String>,
}

pub fn registry_path(game_root: &Path) -> PathBuf {
    game_root.join("stool_mods").join("registry.json")
}

pub fn list_mods(game_root: &Path) -> Vec<ModEntry> {
    fs::read_to_string(registry_path(game_root))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_registry(game_root: &Path, mods: &[ModEntry]) -> Result<(), String> {
    let p = registry_path(game_root);
    crate::settings::ensure_parent(&p);
    fs::write(&p, serde_json::to_string_pretty(mods).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// 安装补丁：覆盖文件 + 备份原文件 + 留存补丁副本（供启停）。
///
/// `allow_conflict=false` 时，若新补丁与「已启用 MOD」覆盖同一文件，直接拒绝并列出冲突，
/// 避免两个 MOD 悄悄互相覆盖导致存档/资源处于不可预期状态（P2-10）。
pub fn install_mod(
    game_root: &Path,
    mod_dir: &Path,
    name: &str,
    progress: &dyn Fn(f32, &str),
    allow_conflict: bool,
) -> Result<ModEntry, String> {
    let name = if name.is_empty() {
        mod_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "patch".into())
    } else {
        name.to_string()
    };
    let mods = list_mods(game_root);
    if mods.iter().any(|m| m.name == name) {
        return Err(format!("MOD '{name}' 已存在，请先卸载或改名"));
    }
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for e in walkdir::WalkDir::new(mod_dir).sort_by_file_name().into_iter().filter_map(|e| e.ok()) {
        if e.file_type().is_file() {
            let rel = e
                .path()
                .strip_prefix(mod_dir)
                .map_err(|e| e.to_string())?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            files.push((rel, e.path().to_path_buf()));
        }
    }
    if files.is_empty() {
        return Err("补丁目录为空".into());
    }
    // 冲突校验（P2-10）：与已启用 MOD 争用同一文件时，默认拒绝安装。
    if !allow_conflict {
        let rels: Vec<String> = files.iter().map(|(r, _)| r.clone()).collect();
        let conf = would_conflict(game_root, &rels, &name);
        if !conf.is_empty() {
            let brief = conf
                .iter()
                .take(5)
                .map(|c| format!("{} ← {}", c.rel, c.mods.join(" / ")))
                .collect::<Vec<_>>()
                .join("；");
            return Err(format!(
                "与已启用 MOD 覆盖同一文件（共 {} 处）：{brief}。请先禁用/卸载冲突 MOD，或使用「允许冲突覆盖」重试",
                conf.len()
            ));
        }
    }
    let mut overwritten = Vec::new();
    for (i, (rel, _)) in files.iter().enumerate() {
        let target = game_root.join(rel);
        if target.exists() {
            overwritten.push(rel.clone());
            let bak = game_root.join("stool_mods").join("_backups").join(&name).join(rel);
            crate::settings::ensure_parent(&bak);
            fs::copy(&target, &bak).map_err(|e| e.to_string())?;
        }
        progress(i as f32 / files.len() as f32, rel);
    }
    for (rel, src) in &files {
        let target = game_root.join(rel);
        crate::settings::ensure_parent(&target);
        fs::copy(src, &target).map_err(|e| e.to_string())?;
        // 留存副本
        let store = game_root.join("stool_mods").join(&name).join(rel);
        crate::settings::ensure_parent(&store);
        fs::copy(src, &store).map_err(|e| e.to_string())?;
    }
    let entry = ModEntry {
        name: name.clone(),
        files: files.into_iter().map(|(r, _)| r).collect(),
        overwritten,
        enabled: true,
    };
    let mut all = list_mods(game_root);
    all.push(entry.clone());
    save_registry(game_root, &all)?;
    Ok(entry)
}

/// 卸载 MOD：还原被覆盖文件，删除新增文件。
pub fn uninstall_mod(game_root: &Path, name: &str) -> Result<(), String> {
    let mut mods = list_mods(game_root);
    let idx = mods.iter().position(|m| m.name == name).ok_or(format!("MOD '{name}' 不存在"))?;
    let entry = mods[idx].clone();
    let bak = game_root.join("stool_mods").join("_backups").join(name);
    for rel in &entry.files {
        let orig = bak.join(rel);
        let target = game_root.join(rel);
        if orig.exists() {
            crate::settings::ensure_parent(&target);
            fs::copy(&orig, &target).map_err(|e| e.to_string())?;
        } else {
            let _ = fs::remove_file(&target);
        }
    }
    let _ = fs::remove_dir_all(&bak);
    let _ = fs::remove_dir_all(game_root.join("stool_mods").join(name));
    mods.remove(idx);
    save_registry(game_root, &mods)
}

/// 启停 MOD。
pub fn toggle_mod(game_root: &Path, name: &str, enabled: bool) -> Result<(), String> {
    let mut mods = list_mods(game_root);
    let entry = mods.iter_mut().find(|m| m.name == name).ok_or(format!("MOD '{name}' 不存在"))?;
    let bak = game_root.join("stool_mods").join("_backups").join(name);
    let store = game_root.join("stool_mods").join(name);
    for rel in &entry.files {
        let target = game_root.join(rel);
        if enabled {
            let src = store.join(rel);
            if src.exists() {
                crate::settings::ensure_parent(&target);
                fs::copy(&src, &target).map_err(|e| e.to_string())?;
            }
        } else {
            let orig = bak.join(rel);
            if orig.exists() {
                crate::settings::ensure_parent(&target);
                fs::copy(&orig, &target).map_err(|e| e.to_string())?;
            } else {
                let _ = fs::remove_file(&target);
            }
        }
    }
    entry.enabled = enabled;
    let name2 = name.to_string();
    save_registry(game_root, &mods)?;
    let _ = name2;
    Ok(())
}

/// 导出当前生效的文件清单（供报告）。
pub fn diff_summary(game_root: &Path) -> BTreeMap<String, usize> {
    list_mods(game_root)
        .into_iter()
        .map(|m| (m.name, m.files.len()))
        .collect()
}

/// 当前生效的冲突：被 ≥2 个**已启用** MOD 覆盖的文件（P2-10）。
pub fn conflicts(game_root: &Path) -> Vec<Conflict> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in list_mods(game_root).into_iter().filter(|m| m.enabled) {
        for rel in &m.files {
            map.entry(rel.clone()).or_default().push(m.name.clone());
        }
    }
    map.into_iter()
        .filter(|(_, names)| names.len() >= 2)
        .map(|(rel, mut mods)| {
            mods.sort();
            Conflict { rel, mods }
        })
        .collect()
}

/// 预演：装入 `files`（MOD 名 `new_name`）后，会与哪些已启用 MOD 冲突。
pub fn would_conflict(game_root: &Path, files: &[String], new_name: &str) -> Vec<Conflict> {
    let want: std::collections::BTreeSet<&str> = files.iter().map(|s| s.as_str()).collect();
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in list_mods(game_root).into_iter().filter(|m| m.enabled && m.name != new_name) {
        for rel in &m.files {
            if want.contains(rel.as_str()) {
                map.entry(rel.clone()).or_default().push(m.name.clone());
            }
        }
    }
    map.into_iter()
        .map(|(rel, mut mods)| {
            mods.sort();
            Conflict { rel, mods }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_game(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_mods_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_mod(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn ok_install(game: &Path, mod_dir: &Path, name: &str, allow: bool) -> Result<ModEntry, String> {
        install_mod(game, mod_dir, name, &|_, _| {}, allow)
    }

    #[test]
    fn install_without_conflict_succeeds() {
        let g = tmp_game("no_conflict");
        let m = g.join("mods_a");
        write_mod(&m, "Scripts/a.txt", "A");
        let e = ok_install(&g, &m, "A", false).unwrap();
        assert_eq!(e.name, "A");
        assert!(conflicts(&g).is_empty());
        let _ = fs::remove_dir_all(&g);
    }

    #[test]
    fn second_mod_conflicting_is_rejected_then_forced() {
        let g = tmp_game("conflict");
        let ma = g.join("mods_a");
        let mb = g.join("mods_b");
        write_mod(&ma, "Data/common.txt", "from A");
        write_mod(&mb, "Data/common.txt", "from B");
        ok_install(&g, &ma, "A", false).unwrap();

        // 预演应识别冲突
        let wc = would_conflict(&g, &["Data/common.txt".to_string()], "B");
        assert_eq!(wc.len(), 1);
        assert_eq!(wc[0].mods, vec!["A".to_string()]);

        // 默认拒绝
        let err = ok_install(&g, &mb, "B", false).unwrap_err();
        assert!(err.contains("覆盖同一文件"), "错误应说明冲突：{err}");
        assert!(err.contains("A"));

        // 强制安装后可检测到真实冲突
        ok_install(&g, &mb, "B", true).unwrap();
        let c = conflicts(&g);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].rel, "Data/common.txt");
        assert_eq!(c[0].mods, vec!["A".to_string(), "B".to_string()]);
        let _ = fs::remove_dir_all(&g);
    }

    #[test]
    fn disabling_one_mod_clears_conflict() {
        let g = tmp_game("disable");
        let ma = g.join("mods_a");
        let mb = g.join("mods_b");
        write_mod(&ma, "x.dat", "A");
        write_mod(&mb, "x.dat", "B");
        ok_install(&g, &ma, "A", false).unwrap();
        ok_install(&g, &mb, "B", true).unwrap();
        assert_eq!(conflicts(&g).len(), 1);
        toggle_mod(&g, "B", false).unwrap();
        assert!(conflicts(&g).is_empty(), "停用后不应再有冲突");
        // A 仍处于启用状态，故 C 抢占同一文件仍会与 A 冲突（B 已停用，不参与预演）
        let wc = would_conflict(&g, &["x.dat".to_string()], "C");
        assert_eq!(wc.len(), 1);
        assert_eq!(wc[0].mods, vec!["A".to_string()]);
        let _ = fs::remove_dir_all(&g);
    }

    #[test]
    fn distinct_files_never_conflict() {
        let g = tmp_game("distinct");
        let ma = g.join("mods_a");
        let mb = g.join("mods_b");
        write_mod(&ma, "p/1.txt", "A");
        write_mod(&mb, "p/2.txt", "B");
        ok_install(&g, &ma, "A", false).unwrap();
        ok_install(&g, &mb, "B", false).unwrap();
        assert!(conflicts(&g).is_empty());
        let _ = fs::remove_dir_all(&g);
    }
}
