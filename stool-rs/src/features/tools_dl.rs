//! 外部工具一键下载：从官方 GitHub Release 自动下载、解压并写回配置。
//!
//! 之前需要用户自己找工具、下压缩包、填路径——现在设置页点一下"自动下载"即可。
//! 统一安装到 ~/.stool/tools/<工具名>/ 下。

use std::io::Read;
use std::path::PathBuf;

/// 一个可自动下载的外部工具。
pub struct ToolSpec {
    pub key: &'static str,       // 对应 settings::Config 的字段名
    pub name: &'static str,      // 展示名
    pub repo: &'static str,      // GitHub 仓库（owner/name）
    pub raw_url: Option<&'static str>, // 直接下载单个文件（如 unrpyc.py）
    pub asset_match: fn(name: &str) -> bool, // 从 Release 资产里挑文件
    pub exe_pick: fn(name: &str) -> bool,    // 解压后挑主程序
}

pub const TOOLS: [ToolSpec; 5] = [
    ToolSpec {
        key: "wolfdec",
        name: "WolfDec（Wolf 引擎解包）",
        repo: "Sinflower/WolfDec",
        raw_url: None,
        asset_match: |n| n == "WolfDec.exe",
        exe_pick: |n| n.eq_ignore_ascii_case("wolfdec.exe"),
    },
    ToolSpec {
        key: "asset_ripper",
        name: "AssetRipper（Unity 解包）",
        repo: "AssetRipper/AssetRipper",
        raw_url: None,
        asset_match: |n| n.starts_with("AssetRipper_win_x64") && n.ends_with(".zip"),
        exe_pick: |n| n.to_lowercase().contains("assetripper") && n.ends_with(".exe"),
    },
    ToolSpec {
        key: "garbro",
        name: "GARbro（未知格式兜底）",
        repo: "morkt/GARbro",
        raw_url: None,
        asset_match: |n| {
            let l = n.to_lowercase();
            l.starts_with("garbro") && l.ends_with(".zip")
        },
        exe_pick: |n| n.eq_ignore_ascii_case("garbro.exe"),
    },
    ToolSpec {
        key: "gdre_tools",
        name: "GDRE Tools（Godot 反编译）",
        repo: "GDRETools/gdsdecomp",
        raw_url: None,
        asset_match: |n| {
            let l = n.to_lowercase();
            l.starts_with("gdre_tools-v") && l.contains("windows") && l.ends_with(".zip")
        },
        exe_pick: |n| n.to_lowercase().starts_with("gdre_tools") && n.ends_with(".exe"),
    },
    ToolSpec {
        key: "unrpyc",
        name: "unrpyc（Ren'Py 反编译）",
        repo: "CensoredUsername/unrpyc",
        raw_url: Some("https://raw.githubusercontent.com/CensoredUsername/unrpyc/master/unrpyc.py"),
        asset_match: |_| false,
        exe_pick: |_| false,
    },
];

pub fn spec_by_key(key: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|t| t.key == key)
}

fn tools_root() -> PathBuf {
    crate::settings::userprofile().join(".stool").join("tools")
}

fn agent(proxy: &str) -> Result<ureq::Agent, String> {
    let mut b = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(600))
        .user_agent("STool/0.1 (game tool; +https://github.com)");
    if !proxy.trim().is_empty() {
        b = b.proxy(ureq::Proxy::new(proxy.trim()).map_err(|e| format!("代理地址无效: {e}"))?);
    }
    Ok(b.build())
}

/// 下载指定工具，返回配置路径（exe 或 .py）。
pub fn download(key: &str, proxy: &str, progress: &dyn Fn(f32, &str)) -> Result<PathBuf, String> {
    let spec = spec_by_key(key).ok_or("未知工具")?;
    let dir = tools_root().join(key);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    let http = agent(proxy)?;

    // unrpyc：直接抓 raw 文件
    if let Some(url) = spec.raw_url {
        progress(0.2, "正在下载 unrpyc.py…");
        let resp = http.get(url).call().map_err(|e| format!("下载失败: {e}"))?;
        let mut buf = Vec::new();
        resp.into_reader().read_to_end(&mut buf).map_err(|e| e.to_string())?;
        let dst = dir.join("unrpyc.py");
        std::fs::write(&dst, &buf).map_err(|e| e.to_string())?;
        progress(1.0, "下载完成");
        return Ok(dst);
    }

    // 其他：查 latest release → 挑资产 → 下载 → 解压
    progress(0.1, &format!("查询 {}/releases/latest…", spec.repo));
    let api = format!("https://api.github.com/repos/{}/releases/latest", spec.repo);
    let resp = http
        .get(&api)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("查询发布信息失败: {e}（GitHub 可能需要代理，设置页可配）"))?;
    let meta: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    let assets = meta
        .get("assets")
        .and_then(|a| a.as_array())
        .ok_or("发布信息里没有资产（Assets）")?;
    let pick = assets
        .iter()
        .filter_map(|a| {
            let name = a.get("name")?.as_str()?;
            let url = a.get("browser_download_url")?.as_str()?;
            Some((name.to_string(), url.to_string()))
        })
        .find(|(name, _)| (spec.asset_match)(name))
        .ok_or("最新发布里没有找到匹配 Windows 的文件（可能上游还没发版，请手动下载）")?;

    progress(0.2, &format!("正在下载 {}（可能较大，请稍候）…", pick.0));
    let resp = http.get(&pick.1).call().map_err(|e| format!("下载失败: {e}"))?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.0);
    let mut reader = resp.into_reader();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if total > 0.0 {
            progress(0.2 + 0.6 * (buf.len() as f32 / total), "正在下载…");
        }
    }
    progress(0.85, "正在解压…");

    if pick.0.ends_with(".exe") {
        // 直接是 exe 资产
        let dst = dir.join(&pick.0);
        std::fs::write(&dst, &buf).map_err(|e| e.to_string())?;
    } else {
        // zip：解压全部内容（.NET 程序需要带 dll）
        let cur = std::io::Cursor::new(&buf);
        let mut ar = zip::ZipArchive::new(cur).map_err(|e| format!("解压失败: {e}"))?;
        for i in 0..ar.len() {
            let mut f = ar.by_index(i).map_err(|e| e.to_string())?;
            let Some(name) = f.enclosed_name().map(|p| p.to_path_buf()) else { continue };
            let dst = dir.join(&name);
            if f.is_dir() {
                let _ = std::fs::create_dir_all(&dst);
                continue;
            }
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut out = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut out).map_err(|e| e.to_string())?;
            std::fs::write(&dst, &out).map_err(|e| e.to_string())?;
        }
    }

    // 挑主程序
    let exe = find_exe(&dir, spec.exe_pick).ok_or("解压完成但没找到主程序 exe")?;
    progress(1.0, "完成");
    Ok(exe)
}

fn find_exe(dir: &std::path::Path, pred: fn(&str) -> bool) -> Option<PathBuf> {
    let mut first: Option<PathBuf> = None;
    for e in walkdir::WalkDir::new(dir).max_depth(3).into_iter().filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_file() {
            let Some(ext) = p.extension().and_then(|x| x.to_str()) else { continue };
            if !ext.eq_ignore_ascii_case("exe") {
                continue;
            }
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if pred(&name) {
                return Some(p.to_path_buf());
            }
            if first.is_none() {
                first = Some(p.to_path_buf());
            }
        }
    }
    first
}
