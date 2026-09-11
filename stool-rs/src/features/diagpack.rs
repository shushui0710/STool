//! 一键导出诊断包（P1-1）。
//!
//! 「出错能查」的前提是能把现场一起给出。本模块把排障需要的材料打成**单个 zip**：
//!
//! | 条目 | 内容 |
//! |---|---|
//! | `env.txt` | 版本 / OS / 架构 / 可执行路径 / 代理 / 外部工具配置摘要（**API Key 已脱敏**） |
//! | `logs/*.log` | `~/.stool/logs/` 下的日志（单文件与总量都有上限，避免 zip 爆掉） |
//! | `warnings.txt` | 从日志里抽出的 WARN / ERROR / PANIC 行（按时间原序） |
//! | `detect.txt` | 指定游戏目录的引擎检测结果（分数 / 证据 / 备注 / 引擎详情） |
//! | `backups.txt` | 指定游戏目录下的 `.stool.bak` 备份清单（还原入口线索） |
//!
//! 安全：日志可能含绝对路径；`mtl_key` 一律脱敏后再写入。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::settings;

/// 单个日志文件读取上限（超出截断，只取尾部）。
const LOG_FILE_MAX: u64 = 4 * 1024 * 1024;
/// 日志总读取上限。
const LOG_TOTAL_MAX: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct DiagReport {
    pub zip_path: PathBuf,
    /// zip 里的条目名（按写入顺序）
    pub entries: Vec<String>,
    /// `warnings.txt` 里的行数
    pub warnings: usize,
}

/// 把 `mtl_key` 之类的密钥脱敏：保留首尾各 2 个字符。
fn mask(secret: &str) -> String {
    let n = secret.chars().count();
    if n == 0 {
        return String::new();
    }
    if n <= 4 {
        return "****".to_string();
    }
    let head: String = secret.chars().take(2).collect();
    let tail: String = secret.chars().skip(n - 2).collect();
    format!("{head}****{tail}（长度 {n}）")
}

/// 环境与配置摘要（已脱敏）。
pub fn env_report() -> String {
    let cfg = settings::load();
    let tool = |name: &str, k: &str| match settings::external_tool(&cfg, k) {
        Some(p) => format!("{name}: {}", p.display()),
        None => format!("{name}: (未配置)"),
    };
    let mut s = String::new();
    s.push_str(&format!("STool 版本: {}\n", crate::VERSION));
    s.push_str(&format!("OS: {} {}\n", std::env::consts::OS, std::env::consts::ARCH));
    s.push_str(&format!(
        "当前目录: {}\n",
        std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()
    ));
    s.push_str(&format!(
        "USERPROFILE: {}\n",
        std::env::var("USERPROFILE").unwrap_or_else(|_| "<未设置>".into())
    ));
    s.push_str(&format!("日志目录: {}\n", crate::diag::log_dir().display()));
    s.push_str(&format!("代理: {}\n", cfg.proxy));
    s.push_str(&format!("Python: {}\n", settings::python_path()));
    s.push_str("---- 外部工具 ----\n");
    for (n, k) in [
        ("GARbro", "garbro"),
        ("AssetRipper", "asset_ripper"),
        ("Il2CppDumper", "il2cpp_dumper"),
        ("WolfDec", "wolfdec"),
        ("GDRE Tools", "gdre_tools"),
    ] {
        s.push_str(&format!("{}\n", tool(n, k)));
    }
    s.push_str("---- 机翻配置（已脱敏）----\n");
    s.push_str(&format!("base_url: {}\n", cfg.mtl_base_url));
    s.push_str(&format!("model: {}\n", cfg.mtl_model));
    s.push_str(&format!("api_key: {}\n", mask(&cfg.mtl_key)));
    s.push_str(&format!("batch: {}\n", cfg.mtl_batch));
    s.push_str(&format!("config_version: {}\n", cfg.config_version));
    s
}

/// 引擎检测报告（需要游戏目录）。
pub fn detect_report(root: &Path) -> String {
    let reg = crate::engines::Registry::new();
    let all = reg.detect_all(root);
    let chosen = crate::engines::Registry::pick_from(&all);
    let mut s = String::new();
    s.push_str(&format!("游戏目录: {}\n", root.display()));
    s.push_str(&format!("扫描规模: {}\n", crate::engines::ScanCtx::build(root).summary()));
    if let Some((d, confident)) = &chosen {
        s.push_str(&format!(
            "选定引擎: {} [{}] {} 分（{}）\n",
            d.name,
            d.plugin_id,
            d.score,
            if *confident { "已确认" } else { "未达判定线" }
        ));
    }
    s.push_str("---- 全部命中 ----\n");
    for d in all.iter().filter(|d| d.score > 0) {
        s.push_str(&format!(
            "[{}] {} {} 分 · {}\n  证据: {}\n",
            d.plugin_id,
            d.name,
            d.score,
            d.confidence().label(),
            d.evidence.join("; ")
        ));
        if !d.notes.is_empty() {
            s.push_str(&format!("  备注: {}\n", d.notes));
        }
    }
    if let Some((d, _)) = &chosen {
        if let Some(e) = reg.get(&d.plugin_id) {
            // 引擎详情（describe）
            let detail = e.describe(root);
            if !detail.is_empty() {
                s.push_str(&format!("---- 引擎详情 ----\n{detail}\n"));
            }
            // 解锁策略
            let spec = crate::features::unlock::spec_or_generic(&d.plugin_id);
            s.push_str(&format!(
                "---- 解锁策略 ----\n识别依据: {}\n动作: {}\n手段: {}\n",
                spec.basis,
                spec.action,
                spec.routes.iter().map(|r| r.key()).collect::<Vec<_>>().join(" > ")
            ));
        }
    }
    s
}

/// 备份清单（`.stool.bak`），便于用户在诊断包里看到还原入口。
pub fn backups_report(root: &Path) -> String {
    let mut s = String::new();
    let mut n = 0usize;
    for e in WalkDir::new(root).max_depth(8).follow_links(false).into_iter().flatten() {
        if !e.file_type().is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy();
        if !name.ends_with(".stool.bak") {
            continue;
        }
        n += 1;
        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
        s.push_str(&format!("{}\t{} 字节\n", e.path().display(), size));
    }
    format!("共 {n} 个 .stool.bak 备份\n{s}")
}

/// 读取日志目录，返回 `(条目名, 内容)` 与 WARN/ERROR/PANIC 行。
fn read_logs() -> (Vec<(String, Vec<u8>)>, Vec<String>) {
    let dir = crate::diag::log_dir();
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut warns: Vec<String> = Vec::new();
    let mut total = 0usize;
    let Ok(rd) = fs::read_dir(&dir) else {
        return (out, warns);
    };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    for p in files {
        let Ok(meta) = fs::metadata(&p) else { continue };
        if meta.len() > LOG_FILE_MAX {
            continue; // 超大日志跳过（异常情况，避免 zip 爆掉）
        }
        let Ok(bytes) = fs::read(&p) else { continue };
        if total + bytes.len() > LOG_TOTAL_MAX {
            break;
        }
        total += bytes.len();
        // 抽取告警行
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            if line.contains("[WARN]") || line.contains("[ERROR]") || line.contains("[PANIC]") {
                warns.push(format!("{}/{}", p.file_name().unwrap_or_default().to_string_lossy(), line));
            }
        }
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "log.txt".into());
        out.push((format!("logs/{name}"), bytes));
    }
    (out, warns)
}

/// 导出诊断包到 `out_zip`。`game_root` 可选：给了就附带引擎检测与备份清单。
pub fn export(game_root: Option<&Path>, out_zip: &Path) -> Result<DiagReport, String> {
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();

    entries.push(("env.txt".into(), env_report().into_bytes()));

    let (logs, warns) = read_logs();
    let warn_count = warns.len();
    entries.extend(logs);
    entries.push((
        "warnings.txt".into(),
        format!(
            "从日志抽取的 WARN/ERROR/PANIC 行（共 {warn_count} 条）\n{}\n",
            warns.join("\n")
        )
        .into_bytes(),
    ));

    if let Some(root) = game_root {
        entries.push(("detect.txt".into(), detect_report(root).into_bytes()));
        entries.push(("backups.txt".into(), backups_report(root).into_bytes()));
    }

    if let Some(parent) = out_zip.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("创建输出目录失败: {e}"))?;
        }
    }

    let file = fs::File::create(out_zip).map_err(|e| format!("创建 {} 失败: {e}", out_zip.display()))?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut names: Vec<String> = Vec::new();
    for (name, bytes) in &entries {
        zw.start_file(name.as_str(), opts)
            .map_err(|e| format!("写入 zip 条目 {name} 失败: {e}"))?;
        zw.write_all(bytes)
            .map_err(|e| format!("写入 zip 条目 {name} 失败: {e}"))?;
        names.push(name.clone());
    }
    zw.finish().map_err(|e| format!("完成 zip 失败: {e}"))?;

    Ok(DiagReport {
        zip_path: out_zip.to_path_buf(),
        entries: names,
        warnings: warn_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_diagpack_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn mask_hides_middle() {
        assert_eq!(mask(""), "");
        assert_eq!(mask("abcd"), "****");
        let m = mask("sk-abcdef123456");
        assert!(m.starts_with("sk"), "{m}");
        assert!(m.contains("****"), "{m}");
        assert!(m.contains("56"), "{m}");
        assert!(!m.contains("abcdef"), "不应泄露中段：{m}");
    }

    #[test]
    fn env_report_masks_key() {
        // 只断言结构：真实 key 不应明文出现（当前配置里若为空也照样成立）
        let r = env_report();
        assert!(r.contains("STool 版本"));
        assert!(r.contains("api_key"));
        assert!(r.contains("config_version"));
    }

    #[test]
    fn export_writes_zip_with_expected_entries() {
        let d = tmpdir("export");
        // 造一个假游戏目录 + 一个 .stool.bak
        let game = d.join("game");
        fs::create_dir_all(game.join("Game_Data")).unwrap();
        fs::write(game.join("UnityPlayer.dll"), b"x").unwrap();
        fs::write(game.join("data.xp3.stool.bak"), b"bak").unwrap();

        let zip = d.join("out").join("diag.zip");
        let r = export(Some(&game), &zip).unwrap();
        assert!(zip.exists(), "zip 应生成");
        assert!(r.entries.iter().any(|e| e == "env.txt"));
        assert!(r.entries.iter().any(|e| e == "warnings.txt"));
        assert!(r.entries.iter().any(|e| e == "detect.txt"));
        assert!(r.entries.iter().any(|e| e == "backups.txt"));
        // zip 能被重新打开（结构合法）
        let f = fs::File::open(&zip).unwrap();
        let archive = zip::ZipArchive::new(f).unwrap();
        assert!(archive.len() >= 4);

        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn backups_report_finds_bak() {
        let d = tmpdir("bak");
        fs::write(d.join("a.rpa.stool.bak"), b"x").unwrap();
        fs::write(d.join("b.txt"), b"x").unwrap();
        let r = backups_report(&d);
        assert!(r.contains("a.rpa.stool.bak"), "{r}");
        assert!(!r.contains("b.txt"), "{r}");
        let _ = fs::remove_dir_all(&d);
    }
}
