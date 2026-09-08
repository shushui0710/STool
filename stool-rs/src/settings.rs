//! 设置：外部工具路径与用户配置（~/.stool/config.json）。

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub output_dir: String,
    pub garbro: String,
    pub asset_ripper: String,
    pub il2cpp_dumper: String,
    pub wolfdec: String,
    pub gdre_tools: String,
    pub unrpyc: String,
    pub python: String,
    pub proxy: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            output_dir: String::new(),
            garbro: String::new(),
            asset_ripper: String::new(),
            il2cpp_dumper: String::new(),
            wolfdec: String::new(),
            gdre_tools: String::new(),
            unrpyc: String::new(),
            python: String::new(),
            proxy: "http://127.0.0.1:7892".into(),
        }
    }
}

pub fn config_path() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|h| PathBuf::from(h).join(".stool").join("config.json"))
        .unwrap_or_else(|| PathBuf::from(".stool").join("config.json"))
}

pub fn load() -> Config {
    fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let p = config_path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&p, serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// unrpyc.py 路径：配置优先，其次 skill 捆绑副本。
pub fn unrpyc_path() -> PathBuf {
    let cfg = load();
    if !cfg.unrpyc.is_empty() {
        let p = PathBuf::from(&cfg.unrpyc);
        if p.exists() {
            return p;
        }
    }
    let candidates = [
        PathBuf::from("D:/STool/tools/unrpyc/unrpyc.py"),
        userprofile()
            .join(".workbuddy")
            .join("skills")
            .join("game-unpacker")
            .join("scripts")
            .join("unrpyc")
            .join("unrpyc.py"),
    ];
    let found = candidates.iter().find(|p| p.exists()).cloned();
    found.unwrap_or(candidates[0].clone())
}

/// Python 解释器：配置优先，其次受管运行时，最后 PATH。
pub fn python_path() -> String {
    let cfg = load();
    if !cfg.python.is_empty() {
        return cfg.python.clone();
    }
    if let Ok(v) = std::env::var("STOOL_PYTHON") {
        return v;
    }
    let managed = userprofile()
        .join(".workbuddy")
        .join("binaries")
        .join("python")
        .join("versions")
        .join("3.13.12")
        .join("python.exe");
    if managed.exists() {
        return managed.to_string_lossy().into_owned();
    }
    "python".to_string()
}

pub fn userprofile() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

pub fn external_tool(cfg: &Config, key: &str) -> Option<PathBuf> {
    let v = match key {
        "garbro" => &cfg.garbro,
        "asset_ripper" => &cfg.asset_ripper,
        "il2cpp_dumper" => &cfg.il2cpp_dumper,
        "wolfdec" => &cfg.wolfdec,
        "gdre_tools" => &cfg.gdre_tools,
        _ => return None,
    };
    if v.is_empty() {
        return None;
    }
    let p = PathBuf::from(v);
    p.exists().then_some(p)
}

/// 确保文件父目录存在。
pub fn ensure_parent(p: &Path) {
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
}
