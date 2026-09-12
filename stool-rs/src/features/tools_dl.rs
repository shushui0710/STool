//! 外部工具一键下载：从官方 GitHub Release 自动下载、解压并写回配置。
//!
//! 之前需要用户自己找工具、下压缩包、填路径——现在设置页点一下"自动下载"即可。
//! 统一安装到 ~/.stool/tools/<工具名>/ 下。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// 一个可自动下载的外部工具。
pub struct ToolSpec {
    pub key: &'static str,       // 对应 settings::Config 的字段名
    pub name: &'static str,      // 展示名
    pub repo: &'static str,      // GitHub 仓库（owner/name）
    pub raw_url: Option<&'static str>, // 直接下载单个文件（如 unrpyc.py）
    // 固定下载某个 Release 的资产 URL（如 AssetRipper 新版只剩 GUI 无 CLI，
    // 必须钉在 0.3.4.0 的 Console 版）；设置后跳过 Release 查询。
    pub fixed_url: Option<&'static str>,
    pub asset_match: fn(name: &str) -> bool, // 从 Release 资产里挑文件
    pub exe_pick: fn(name: &str) -> bool,    // 解压后挑主程序
}

pub const TOOLS: [ToolSpec; 5] = [
    ToolSpec {
        key: "wolfdec",
        name: "WolfDec（Wolf 引擎解包）",
        repo: "Sinflower/WolfDec",
        raw_url: None,
        fixed_url: None,
        asset_match: |n| n == "WolfDec.exe",
        exe_pick: |n| n.eq_ignore_ascii_case("wolfdec.exe"),
    },
    // 注意：AssetRipper 1.x/2.x 的 Release 只剩 AssetRipper.GUI.Free.exe（本地网页界面），
    // 不再支持 -i/-o 命令行调用；0.3.4.0 是最后一个带 Console 版（AssetRipper.exe，
    // 用法：<输入路径> -o <输出目录> -q）的版本，故固定下载该版。
    ToolSpec {
        key: "asset_ripper",
        name: "AssetRipper（Unity 解包）",
        repo: "AssetRipper/AssetRipper",
        raw_url: None,
        fixed_url: Some("https://github.com/AssetRipper/AssetRipper/releases/download/0.3.4.0/AssetRipper_win_x64.zip"),
        asset_match: |_| false,
        exe_pick: |n| n.eq_ignore_ascii_case("assetripper.exe"),
    },
    ToolSpec {
        key: "garbro",
        name: "GARbro（未知格式兜底）",
        repo: "morkt/GARbro",
        raw_url: None,
        fixed_url: None,
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
        fixed_url: None,
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
        fixed_url: None,
        asset_match: |_| false,
        exe_pick: |_| false,
    },
];

pub fn spec_by_key(key: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|t| t.key == key)
}

fn tools_root() -> PathBuf {
    crate::settings::tools_root()
}

fn agent(proxy: &str) -> Result<ureq::Agent, String> {
    let mut b = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(600))
        .user_agent("STool/0.1 (game tool; +https://github.com)");
    // ureq 2.12 起，仅启用 native-tls 特性并不会自动作为默认 TLS 后端
    // （默认后端只认 rustls 的 "tls" 特性），必须显式调用 tls_connector 接线，
    // 否则所有 HTTPS 请求都会报 "no TLS backend is configured"，
    // 设置页的一键下载因此全部失败。这里显式接线 native-tls（Windows 走 schannel，
    // 直接用系统证书库，对内网/代理环境更友好）。
    b = b.tls_connector(std::sync::Arc::new(
        ureq::native_tls::TlsConnector::new().map_err(|e| format!("TLS 初始化失败: {e}"))?,
    ));
    if !proxy.trim().is_empty() {
        b = b.proxy(ureq::Proxy::new(proxy.trim()).map_err(|e| format!("代理地址无效: {e}"))?);
    }
    Ok(b.build())
}

/// 下载指定工具，返回配置路径（exe 或 .py）。
///
/// **不把整个下载内容读进内存**：边下边写临时文件、边算 SHA-256。
/// 完整性校验：
/// - 能拿到 GitHub Release 资产 API 的 `digest`（`sha256:…`）时，**必须**一致，
///   否则中止并删除临时文件（防代理劫持 / 上游被换包）；
/// - 上游没给摘要时（如 raw.githubusercontent 的单文件），把算出的摘要记进
///   `<工具目录>/.sha256` 并在进度里回显，供人工核对。
pub fn download(key: &str, proxy: &str, progress: &dyn Fn(f32, &str)) -> Result<PathBuf, String> {
    let spec = spec_by_key(key).ok_or("未知工具")?;
    let dir = tools_root().join(key);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    let http = agent(proxy)?;

    // unrpyc：直接抓 raw 文件（raw.githubusercontent 不提供摘要，只能记录）
    if let Some(url) = spec.raw_url {
        progress(0.2, "正在下载 unrpyc.py…");
        let dst = dir.join("unrpyc.py");
        let (got, size) = fetch_to_file(&http, url, &dst, |frac| {
            progress(0.2 + 0.7 * frac, "正在下载 unrpyc.py…")
        })?;
        record_digest(&dir, &got, size, None)?;
        progress(1.0, "下载完成");
        return Ok(dst);
    }

    // 其他：固定 URL 或查发布列表 → 挑资产（连带摘要）→ 下载 → 校验 → 解压
    let (name, url, digest) = if let Some(url) = spec.fixed_url {
        // 钉死版本的工具（AssetRipper 需要带 CLI 的 0.3.4.0，见 TOOLS 注释）
        let name = url.rsplit('/').next().unwrap_or("download.zip").to_string();
        // 固定 URL 也要尽力拿摘要：按 tag 查该 Release 的资产列表（拿不到就退化为记录）
        let digest = release_asset_digest(&http, spec.repo, tag_of(url), &name);
        (name, url.to_string(), digest)
    } else {
        // 注意：不能只看 latest——有的仓库（如 GARbro）新版本不再发 zip 资产，
        // 需要按时间从新到旧遍历最近若干个 Release，找到第一个含匹配资产的版本。
        progress(0.1, &format!("查询 {}/releases…", spec.repo));
        let api = format!("https://api.github.com/repos/{}/releases?per_page=30", spec.repo);
        let resp = http
            .get(&api)
            .set("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| format!("查询发布信息失败: {e}（GitHub 可能需要代理，设置页可配）"))?;
        let releases: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
        let releases = releases.as_array().ok_or("发布信息格式异常（不是列表）")?;
        releases
            .iter()
            .filter_map(|r| r.get("assets").and_then(|a| a.as_array()))
            .flatten()
            .filter_map(|a| {
                let name = a.get("name")?.as_str()?;
                let url = a.get("browser_download_url")?.as_str()?;
                let digest = a
                    .get("digest")
                    .and_then(|d| d.as_str())
                    .and_then(|d| d.strip_prefix("sha256:"))
                    .map(|s| s.to_string());
                Some((name.to_string(), url.to_string(), digest))
            })
            .find(|(name, _, _)| (spec.asset_match)(name))
            .ok_or("最近 30 个发布里没有找到匹配 Windows 的文件（可能上游还没发版，请手动下载）")?
    };

    progress(0.2, &format!("正在下载 {name}（可能较大，请稍候）…"));
    let part = dir.join(".download.part");
    let (got, size) = fetch_to_file(&http, &url, &part, |frac| progress(0.2 + 0.6 * frac, "正在下载…"))?;

    if let Some(want) = &digest {
        if !want.eq_ignore_ascii_case(&got) {
            let _ = std::fs::remove_file(&part);
            return Err(format!(
                "下载内容完整性校验失败，已中止并删除临时文件。\n期望 SHA-256: {want}\n实际 SHA-256: {got}\n\
                 可能是代理/网络劫持或上游替换了文件；请核对后重试，或手动下载并填路径。"
            ));
        }
        progress(0.85, "校验通过（SHA-256 与上游一致），正在解压…");
    } else {
        progress(0.85, "上游未提供摘要，已记录下载校验和供人工核对；正在解压…");
    }

    if name.ends_with(".exe") {
        // 直接是 exe 资产
        let dst = dir.join(&name);
        std::fs::rename(&part, &dst).map_err(|e| format!("落盘失败: {e}"))?;
    } else {
        // zip：解压全部内容（.NET 程序需要带 dll）
        unzip_into(&part, &dir)?;
        let _ = std::fs::remove_file(&part);
    }
    record_digest(&dir, &got, size, digest.as_deref())?;

    // 挑主程序
    let exe = find_exe(&dir, spec.exe_pick).ok_or("解压完成但没找到主程序 exe")?;
    progress(1.0, "完成");
    Ok(exe)
}

/// 把 URL 内容**流式**写入 `dst`，同时算 SHA-256；返回 (十六进制摘要, 字节数)。
fn fetch_to_file(
    http: &ureq::Agent,
    url: &str,
    dst: &Path,
    on_progress: impl Fn(f32),
) -> Result<(String, u64), String> {
    let resp = http.get(url).call().map_err(|e| format!("下载失败: {e}"))?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut f = std::fs::File::create(dst).map_err(|e| format!("创建 {} 失败: {e}", dst.display()))?;
    let mut hasher = crate::hash::Sha256::new();
    let mut buf = [0u8; 65536];
    let mut size = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        size += n as u64;
        if total > 0 {
            on_progress(size as f32 / total as f32);
        }
    }
    f.flush().map_err(|e| e.to_string())?;
    Ok((crate::hash::hex(&hasher.finalize()), size))
}

/// 就地解压 zip（逐条目流式拷贝，不整包进内存）。
fn unzip_into(zip_path: &Path, dir: &Path) -> Result<(), String> {
    let f = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut ar = zip::ZipArchive::new(f).map_err(|e| format!("解压失败: {e}"))?;
    for i in 0..ar.len() {
        let mut z = ar.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = z.enclosed_name().map(|p| p.to_path_buf()) else { continue };
        let dst = dir.join(&name);
        if z.is_dir() {
            let _ = std::fs::create_dir_all(&dst);
            continue;
        }
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut out = std::fs::File::create(&dst).map_err(|e| e.to_string())?;
        std::io::copy(&mut z, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 记录本次下载的校验和（`<工具目录>/.sha256`），供事后核对 / 溯源。
fn record_digest(dir: &Path, got: &str, size: u64, upstream: Option<&str>) -> Result<(), String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let note = serde_json::json!({
        "sha256": got,
        "bytes": size,
        "upstream_digest": upstream,
        "verified": upstream.map(|u| u.eq_ignore_ascii_case(got)).unwrap_or(false),
        "at_unix": now,
    });
    std::fs::write(dir.join(".sha256"), serde_json::to_string_pretty(&note).unwrap_or_default())
        .map_err(|e| format!("写入下载校验记录失败: {e}"))
}

/// 按 tag（或 latest）查询某个 release 里指定资产的 `sha256:` 摘要（尽力而为，失败返回 None）。
fn release_asset_digest(http: &ureq::Agent, repo: &str, tag: Option<&str>, asset: &str) -> Option<String> {
    let url = match tag {
        Some(t) => format!("https://api.github.com/repos/{repo}/releases/tags/{t}"),
        None => format!("https://api.github.com/repos/{repo}/releases/latest"),
    };
    let resp = http.get(&url).set("Accept", "application/vnd.github+json").call().ok()?;
    let v: serde_json::Value = resp.into_json().ok()?;
    for a in v.get("assets")?.as_array()? {
        if a.get("name").and_then(|n| n.as_str()) == Some(asset) {
            return a
                .get("digest")
                .and_then(|d| d.as_str())
                .and_then(|d| d.strip_prefix("sha256:"))
                .map(|s| s.to_string());
        }
    }
    None
}

/// 从 `…/releases/download/<tag>/<file>` 里取出 tag。
fn tag_of(url: &str) -> Option<&str> {
    url.split("/releases/download/").nth(1)?.split('/').next()
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
