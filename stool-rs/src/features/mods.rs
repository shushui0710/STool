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
pub fn install_mod(game_root: &Path, mod_dir: &Path, name: &str, progress: &dyn Fn(f32, &str)) -> Result<ModEntry, String> {
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
    let bak = game_root.join("stool_mods").join("_backups").join(&name);
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
