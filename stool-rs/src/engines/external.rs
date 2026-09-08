//! Wolf RPG / Unity：检测 + 外部工具委派。

use std::path::Path;
use std::process::Command;

use super::{Ctx, Detection, Engine, Op, OpOutcome};
use crate::settings;

pub struct WolfPlugin;

impl Engine for WolfPlugin {
    fn id(&self) -> &'static str {
        "wolf"
    }
    fn name(&self) -> &'static str {
        "Wolf RPG Editor (ウディタ)"
    }
    fn priority(&self) -> i32 {
        72
    }
    fn detect(&self, root: &Path) -> Detection {
        let wolves = crate::engines::others::list_files_by_ext_pub(&root.join("Data"), &["wolf"]);
        let mut score = 0;
        let mut ev = Vec::new();
        if !wolves.is_empty() {
            score += 75;
            ev.push(format!("{} 个 .wolf 封包", wolves.len()));
        }
        if root.join("Game.exe").exists() && !wolves.is_empty() {
            score += 10;
        }
        Detection { plugin_id: self.id().into(), name: self.name().into(), score, evidence: ev, notes: String::new() }
    }
    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract, Op::TextExtract]
    }
    fn describe(&self, root: &Path) -> String {
        format!("封包: {:?}", crate::engines::others::list_files_by_ext_pub(&root.join("Data"), &["wolf"]).iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect::<Vec<_>>())
    }
    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        let cfg = settings::load();
        let wolfdec = match settings::external_tool(&cfg, "wolfdec") {
            Some(p) => p,
            None => {
                return OpOutcome::fail(
                    "未配置 WolfDec 路径（设置页填写 WolfDec.exe 后重试；GitHub 有命令行发行版）",
                )
            }
        };
        match Command::new(&wolfdec)
            .args(["-o", ctx.out_dir.to_string_lossy().as_ref(), ctx.root.to_string_lossy().as_ref()])
            .output()
        {
            Ok(o) if o.status.success() => OpOutcome::ok(format!("WolfDec 解包完成 → {}", ctx.out_dir.display())),
            Ok(o) => OpOutcome::fail(format!(
                "WolfDec 失败: {}",
                String::from_utf8_lossy(&if o.stderr.is_empty() { o.stdout } else { o.stderr })
            )),
            Err(e) => OpOutcome::fail(format!("WolfDec 调用失败: {e}")),
        }
    }
    /// 文本提取：临时目录跑 WolfDec 解密数据文件，再扫描其中的台词字符串。
    /// Wolf 解密后的 .dat 以 [u32 长度][UTF-8 内容] 形式内嵌字符串（启发式扫描）；
    /// 部分 WolfDec 版本直接输出 .txt，按行扫描。
    fn text_extract(&self, ctx: &Ctx, out_csv: &Path) -> OpOutcome {
        let cfg = settings::load();
        let wolfdec = match settings::external_tool(&cfg, "wolfdec") {
            Some(p) => p,
            None => return OpOutcome::fail("未配置 WolfDec 路径（设置页填写 WolfDec.exe 后重试）"),
        };
        let tmp = std::env::temp_dir().join(format!("stool_wolf_text_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::create_dir_all(&tmp);
        let run = Command::new(&wolfdec)
            .args(["-o", tmp.to_string_lossy().as_ref(), ctx.root.to_string_lossy().as_ref()])
            .output();
        if let Ok(o) = &run {
            if !o.status.success() {
                let _ = std::fs::remove_dir_all(&tmp);
                let msg_src = if o.stderr.is_empty() { &o.stdout } else { &o.stderr };
                return OpOutcome::fail(format!("WolfDec 失败: {}", String::from_utf8_lossy(msg_src)));
            }
        }
        let mut rows: Vec<[String; 4]> = Vec::new();
        for entry in walkdir::WalkDir::new(&tmp).sort_by_file_name() {
            let p = match entry {
                Ok(p) => p,
                Err(_) => continue,
            };
            if !p.file_type().is_file() {
                continue;
            }
            let meta = match p.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.len() > 64 * 1024 * 1024 {
                continue;
            }
            let name = p.file_name().to_string_lossy().to_lowercase();
            let blob = match std::fs::read(p.path()) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let rel = p
                .path()
                .strip_prefix(&tmp)
                .unwrap_or(p.path())
                .to_string_lossy()
                .replace('\\', "/");
            let before = rows.len();
            if name.ends_with(".txt") || name.ends_with(".csv") || name.ends_with(".tsv") {
                scan_text_lines(&blob, &rel, &mut rows);
            } else {
                scan_len_prefixed_strings(&blob, &rel, &mut rows);
            }
            let _ = before;
        }
        let _ = std::fs::remove_dir_all(&tmp);
        if rows.is_empty() {
            return OpOutcome::fail("未从 WolfDec 解密输出中扫描到台词文本");
        }
        let n = rows.len();
        match crate::features::text::write_csv(out_csv, &rows) {
            Ok(()) => OpOutcome::okn(format!("提取 {n} 条文本 → {}（自动调用 WolfDec 解密后扫描）", out_csv.display()), n),
            Err(e) => OpOutcome::fail(e),
        }
    }
}

/// 纯文本文件按行扫描（含 CJK 的行）。
fn scan_text_lines(blob: &[u8], rel: &str, rows: &mut Vec<[String; 4]>) {
    let text = String::from_utf8_lossy(blob);
    for (ln, line) in text.lines().enumerate() {
        let s = line.trim();
        if s.len() >= 2 && crate::engines::others::has_cjk(s) {
            rows.push([format!("{rel}:{ln}"), "wolf".into(), s.chars().take(500).collect(), String::new()]);
        }
    }
}

/// 启发式扫描：Wolf 解密数据中字符串以 [u32 长度][UTF-8 字节] 内嵌。
/// 逐位置尝试读取长度前缀，命中合法 UTF-8 且含 CJK 则记录并跳过。
fn scan_len_prefixed_strings(blob: &[u8], rel: &str, rows: &mut Vec<[String; 4]>) {
    let mut i = 0usize;
    while i + 4 <= blob.len() {
        let len = u32::from_le_bytes(blob[i..i + 4].try_into().unwrap()) as usize;
        if (2..=4096).contains(&len) && i + 4 + len <= blob.len() {
            let cand = &blob[i + 4..i + 4 + len];
            if let Ok(s) = std::str::from_utf8(cand) {
                if crate::engines::others::has_cjk(s) && !s.contains('\u{0}') {
                    rows.push([format!("{rel}:{i:x}"), "wolf".into(), s.chars().take(500).collect(), String::new()]);
                    i += 4 + len;
                    continue;
                }
            }
        }
        i += 1;
    }
}

pub struct UnityPlugin;

impl Engine for UnityPlugin {
    fn id(&self) -> &'static str {
        "unity"
    }
    fn name(&self) -> &'static str {
        "Unity"
    }
    fn priority(&self) -> i32 {
        78
    }
    fn detect(&self, root: &Path) -> Detection {
        let mut score = 0;
        let mut ev = Vec::new();
        let data_dirs: Vec<_> = std::fs::read_dir(root)
            .map(|rd| rd.flatten().filter(|d| d.path().is_dir() && d.file_name().to_string_lossy().ends_with("_Data")).map(|d| d.path()).collect::<Vec<_>>())
            .unwrap_or_default();
        if !data_dirs.is_empty() || root.join("UnityPlayer.dll").exists() {
            score += 75;
            ev.push("UnityPlayer.dll / *_Data".into());
        }
        if let Some(dd) = data_dirs.first() {
            if dd.join("Managed").join("Assembly-CSharp.dll").exists() {
                ev.push("Mono 版（Assembly-CSharp.dll）".into());
            }
        }
        if !crate::engines::others::list_files_by_ext_pub(root, &["dll"]).is_empty() {
            // 仅提示
        }
        Detection { plugin_id: self.id().into(), name: self.name().into(), score, evidence: ev, notes: String::new() }
    }
    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract]
    }
    fn describe(&self, root: &Path) -> String {
        let mono = std::fs::read_dir(root)
            .map(|rd| {
                rd.flatten()
                    .any(|d| d.path().join("Managed").join("Assembly-CSharp.dll").exists())
            })
            .unwrap_or(false);
        if mono { "Mono 版".into() } else { "IL2CPP 版（或未知）".into() }
    }
    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        let cfg = settings::load();
        let ripper = match settings::external_tool(&cfg, "asset_ripper") {
            Some(p) => p,
            None => {
                return OpOutcome::fail(
                    "未配置 AssetRipper 路径（设置页填写后重试；Mono 版亦可用 dnSpy 直接改 Assembly-CSharp.dll）",
                )
            }
        };
        match Command::new(&ripper)
            .args(["-i", ctx.root.to_string_lossy().as_ref(), "-o", ctx.out_dir.to_string_lossy().as_ref()])
            .output()
        {
            Ok(o) if o.status.success() => OpOutcome::ok(format!("AssetRipper 导出完成 → {}", ctx.out_dir.display())),
            Ok(o) => OpOutcome::fail(format!(
                "AssetRipper 失败: {}",
                String::from_utf8_lossy(&if o.stderr.is_empty() { o.stdout } else { o.stderr })
            )),
            Err(e) => OpOutcome::fail(format!("AssetRipper 调用失败: {e}")),
        }
    }
}
