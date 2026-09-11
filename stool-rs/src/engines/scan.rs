//! 一次性目录扫描（检测专用）。
//!
//! 旧实现对每个插件各扫一遍整棵目录树：10 个插件 ≈ 8 次全树 walkdir + 2 次子树遍历，
//! 且 `list_files_by_ext` 还会对结果做一次无用的 sort。对大目录（Galgame 常 2–10 万文件）
//! 就是几十万次 stat 系统调用，全部串行。
//!
//! 这里把检测需要的全部文件系统信息在**一次** walkdir 里收齐，所有插件从内存里查。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// 目录名采样深度：超过该深度的目录名不再记录（只统计扩展名），避免深目录撑爆内存。
const DIR_NAME_DEPTH: usize = 5;
/// 单次扫描的文件数上限（防御超大数据目录）；超过即截断并置 `truncated`。
const MAX_FILES: usize = 600_000;

/// 检测关心的"特征文件名"（小写全名）。
///
/// 有些强特征文件埋在子目录里（`data/Actors.json`、`*_Data/Managed/Assembly-CSharp.dll`），
/// 只靠"根目录一级文件名"或"扩展名计数"都判不出来。若给**每个**文件都建名字索引，
/// 十万级素材目录要多花几 MB 内存且没意义；所以这里只统计一份固定白名单，
/// 命中集合最多几十个键，代价可忽略。
const NOTABLE_FILES: &[&str] = &[
    "actors.json",          // RPG Maker MV/MZ 核心数据（data/ 或 www/data/）
    "game.rpgproject",      // RPG Maker MV 工程文件
    "assembly-csharp.dll",  // Unity Mono 程序集
    "gameassembly.dll",     // Unity IL2CPP 程序集
    "project.godot",        // Godot 明文工程
    "config.tjs",           // KiriKiri / TyranoScript 配置
    "startup.tjs",          // KiriKiri 启动脚本
    "data.win",             // GameMaker 编译数据
    "gameexe.dat",          // SiglusEngine 配置
    "gameexe.ini",          // RealLive 配置
    "seen.txt",             // RealLive 已读记录
    "tyrano.js",            // TyranoScript 运行时（tyrano/ 内）
    "main.lua",             // LÖVE 入口
    // —— 以下为盲区引擎的"埋起来的强特征"（实测确认，见 docs/引擎识别依据与解锁策略.md）——
    "bregexp.dll",          // BGI / Ethornell（Buriko）特征 DLL
    "bgi.gdb",              // BGI 存档 / 配置（含 SystemData 标记）
    "libcocos2d.dll",       // Cocos2d-x / JSB 运行时
    "rpg_rt.exe",           // RPG Maker 2000/2003 运行时
    "nscript.dat",          // NScripter 加密脚本
    "arc.nsa",              // NScripter 资源包
    "system40.exe",         // AliceSoft System 运行时
];

/// 一次扫描的结果，供所有引擎插件共享。
#[derive(Debug, Default)]
pub struct ScanCtx {
    pub root: PathBuf,
    /// 根目录一级文件名（小写）
    root_files: HashSet<String>,
    /// 根目录一级子目录名（小写）
    root_dirs: HashSet<String>,
    /// 任意深度出现过的目录名（小写，深度 ≤ DIR_NAME_DEPTH）
    dir_names: HashSet<String>,
    /// 目录名 → 首个出现路径（小写键）
    dir_first: HashMap<String, PathBuf>,
    /// 扩展名 → 文件数（小写，不含点）
    ext_count: HashMap<String, usize>,
    /// 扩展名 → 首个出现文件路径（小写键）
    ext_first: HashMap<String, PathBuf>,
    /// 根目录一级的 .exe 文件名（小写）
    exes: Vec<String>,
    /// 白名单特征文件（`NOTABLE_FILES`）在任意深度命中的名字集合（小写）
    notable_files: HashSet<String>,
    /// 是否因文件数超限被截断（截断时扩展名计数可能偏低）
    pub truncated: bool,
    /// 实际遍历到的文件数
    pub files_seen: usize,
}

/// 扩展名是否是"分卷序号"（恰 3 位数字，如 `000` / `001`）。
fn is_numbered_volume_ext(ext: &str) -> bool {
    ext.len() == 3 && ext.bytes().all(|b| b.is_ascii_digit())
}

impl ScanCtx {
    /// 构建：1 次 read_dir（根一级）+ 1 次 walkdir（递归）。
    pub fn build(root: &Path) -> Self {
        let mut s = ScanCtx { root: root.to_path_buf(), ..Default::default() };

        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir {
                    s.root_dirs.insert(name);
                } else {
                    if name.ends_with(".exe") {
                        s.exes.push(name.clone());
                    }
                    s.root_files.insert(name);
                }
            }
        }

        for entry in walkdir::WalkDir::new(root).follow_links(false).into_iter().flatten() {
            let p = entry.path();
            if entry.file_type().is_dir() {
                let d = entry.depth();
                if d > 0 && d <= DIR_NAME_DEPTH {
                    let dn = entry.file_name().to_string_lossy().to_lowercase();
                    s.dir_first.entry(dn.clone()).or_insert_with(|| p.to_path_buf());
                    s.dir_names.insert(dn);
                }
                continue;
            }
            s.files_seen += 1;
            if s.files_seen > MAX_FILES {
                s.truncated = true;
                break;
            }
            // 白名单特征文件：用 eq_ignore_ascii_case 逐个比，避免给每个文件都分配小写串
            if let Some(fname) = entry.file_name().to_str() {
                if let Some(n) = NOTABLE_FILES.iter().copied().find(|n| fname.eq_ignore_ascii_case(n)) {
                    s.notable_files.insert(n.to_string());
                }
            }
            if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
                let ext = ext.to_lowercase();
                *s.ext_count.entry(ext.clone()).or_insert(0) += 1;
                s.ext_first.entry(ext).or_insert_with(|| p.to_path_buf());
            }
        }
        s
    }

    // ---- 根目录一级 ----

    pub fn has_root_file(&self, name: &str) -> bool {
        self.root_files.contains(&name.to_lowercase())
    }

    pub fn has_root_dir(&self, name: &str) -> bool {
        self.root_dirs.contains(&name.to_lowercase())
    }

    /// 根目录一级中以 `suffix` 结尾的子目录名（小写）。
    pub fn root_dirs_ending(&self, suffix: &str) -> Vec<&str> {
        let sfx = suffix.to_lowercase();
        self.root_dirs.iter().filter(|d| d.ends_with(&sfx)).map(|s| s.as_str()).collect()
    }

    /// 根目录一级中以 `suffix` 结尾的文件名（小写）。
    pub fn root_files_ending(&self, suffix: &str) -> Vec<&str> {
        let sfx = suffix.to_lowercase();
        self.root_files.iter().filter(|d| d.ends_with(&sfx)).map(|s| s.as_str()).collect()
    }

    /// 根目录一级的 .exe 名（小写）。
    pub fn exe_names(&self) -> &[String] {
        &self.exes
    }

    /// 是否存在名字包含 `needle` 的根级 exe（不区分大小写）。
    pub fn any_exe_contains(&self, needle: &str) -> bool {
        let n = needle.to_lowercase();
        self.exes.iter().any(|e| e.contains(&n))
    }

    /// 是否存在该名字的文件：根目录一级 **或** `NOTABLE_FILES` 白名单（任意深度）。
    /// 用于 `data/Actors.json`、`*_Data/Managed/Assembly-CSharp.dll` 这类埋起来的强特征。
    pub fn has_file_named(&self, name: &str) -> bool {
        let n = name.to_lowercase();
        self.root_files.contains(&n) || self.notable_files.contains(&n)
    }

    // ---- 递归（任意深度）----

    /// 是否存在以 `prefix` 开头的目录名（如 `py3-windows-x86_64`）。
    pub fn has_dir_starting_with(&self, prefix: &str) -> bool {
        let p = prefix.to_lowercase();
        self.dir_names.iter().any(|d| d.starts_with(&p))
    }

    /// 是否存在以 `suffix` 结尾的目录名（任意深度 ≤ `DIR_NAME_DEPTH`）。
    pub fn has_dir_ending_with(&self, suffix: &str) -> bool {
        let s = suffix.to_lowercase();
        self.dir_names.iter().any(|d| d.ends_with(&s))
    }

    /// 是否存在"分卷"文件：某扩展名恰为 3 位数字（`x.pfs.000` / `data.7z.001`）。
    ///
    /// 用于消解"单文件同名扩展名"的歧义 —— 例如 `.pfs` 在 BGI 与 Artemis 里同名，
    /// 只有**带分卷**才是 Artemis 的可靠判据。
    pub fn has_numbered_volume(&self) -> bool {
        self.ext_count.keys().any(|e| is_numbered_volume_ext(e))
    }

    /// 任意位置是否存在该目录名。
    pub fn has_dir(&self, name: &str) -> bool {
        self.dir_names.contains(&name.to_lowercase())
    }

    /// 该目录名的首个出现路径。
    pub fn dir_path(&self, name: &str) -> Option<&Path> {
        self.dir_first.get(&name.to_lowercase()).map(|p| p.as_path())
    }

    /// 该扩展名的文件数（小写，不含点）。
    pub fn ext_count(&self, ext: &str) -> usize {
        self.ext_count.get(&ext.to_lowercase()).copied().unwrap_or(0)
    }

    /// 是否存在该扩展名的文件。
    pub fn has_ext(&self, ext: &str) -> bool {
        self.ext_count(ext) > 0
    }

    /// 该扩展名的首个文件路径（便于 evidence 里给出具体文件）。
    pub fn first_ext(&self, ext: &str) -> Option<&Path> {
        self.ext_first.get(&ext.to_lowercase()).map(|p| p.as_path())
    }

    /// 首个命中文件的文件名（小写），用于 evidence。
    pub fn first_ext_name(&self, ext: &str) -> String {
        self.first_ext(ext)
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// 任意一个扩展名命中即真。
    pub fn has_any_ext(&self, exts: &[&str]) -> bool {
        exts.iter().any(|e| self.has_ext(e))
    }

    /// 这些扩展名的文件总数。
    pub fn ext_sum(&self, exts: &[&str]) -> usize {
        exts.iter().map(|e| self.ext_count(e)).sum()
    }

    /// 所有"有计数的扩展名"，按数量降序（供兜底提示）。
    pub fn top_exts(&self, n: usize) -> Vec<(String, usize)> {
        let mut v: Vec<(String, usize)> = self.ext_count.iter().map(|(k, c)| (k.clone(), *c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    /// 一句话描述扫描规模（供 GUI 显示"扫了多少文件、是否被截断"）。
    pub fn summary(&self) -> String {
        if self.truncated {
            format!("已扫描 {} 个文件（超出上限，结果可能不全）", self.files_seen)
        } else {
            format!("已扫描 {} 个文件", self.files_seen)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_scan_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn test_scan_collects_root_and_ext() {
        let d = tmpdir("basic");
        std::fs::create_dir_all(d.join("game")).unwrap();
        std::fs::create_dir_all(d.join("renpy")).unwrap();
        std::fs::write(d.join("Game.exe"), b"x").unwrap();
        std::fs::write(d.join("game").join("a.rpa"), b"x").unwrap();
        std::fs::write(d.join("game").join("b.rpyc"), b"x").unwrap();
        std::fs::write(d.join("game").join("c.RPYC"), b"x").unwrap();

        let s = ScanCtx::build(&d);
        assert!(s.has_root_file("game.exe"));
        assert!(s.has_root_dir("game"));
        assert!(s.has_dir("renpy"));
        assert_eq!(s.ext_count("rpa"), 1);
        assert_eq!(s.ext_count("rpyc"), 2, "扩展名应大小写归一");
        assert!(s.any_exe_contains("game"));
        assert!(!s.truncated);
        assert_eq!(s.files_seen, 4);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_scan_suffix_helpers() {
        let d = tmpdir("suffix");
        std::fs::create_dir_all(d.join("Game_Data").join("Managed")).unwrap();
        std::fs::write(d.join("Game_Data").join("Managed").join("Assembly-CSharp.dll"), b"x").unwrap();
        std::fs::write(d.join("UnityPlayer.dll"), b"x").unwrap();

        let s = ScanCtx::build(&d);
        assert_eq!(s.root_dirs_ending("_data"), vec!["game_data"]);
        assert_eq!(s.root_files_ending(".dll"), vec!["unityplayer.dll"]);
        assert!(s.has_dir("managed"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_scan_notable_nested_file() {
        let d = tmpdir("notable");
        std::fs::create_dir_all(d.join("www").join("data")).unwrap();
        std::fs::write(d.join("www").join("data").join("Actors.json"), b"{}").unwrap();
        std::fs::create_dir_all(d.join("Game_Data").join("Managed")).unwrap();
        std::fs::write(d.join("Game_Data").join("Managed").join("Assembly-CSharp.dll"), b"x").unwrap();

        let s = ScanCtx::build(&d);
        assert!(s.has_file_named("actors.json"), "嵌套白名单文件应能命中");
        assert!(s.has_file_named("Assembly-CSharp.dll"), "白名单匹配应大小写不敏感");
        assert!(!s.has_file_named("not-there.json"));
        assert!(s.has_dir_starting_with("game_"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_scan_missing_dir_is_empty() {
        let s = ScanCtx::build(Path::new("D:/STool/__definitely_not_here__"));
        assert_eq!(s.files_seen, 0);
        assert!(!s.has_ext("xp3"));
        assert!(s.exe_names().is_empty());
    }

    #[test]
    fn test_scan_numbered_volume_and_dir_suffix() {
        let d = tmpdir("numvol");
        std::fs::create_dir_all(d.join("Engine").join("Content")).unwrap();
        std::fs::write(d.join("x.pfs"), b"x").unwrap();
        std::fs::write(d.join("x.pfs.000"), b"x").unwrap();
        std::fs::write(d.join("x.pfs.001"), b"x").unwrap();
        std::fs::write(d.join("normal.dat"), b"x").unwrap();

        let s = ScanCtx::build(&d);
        assert!(s.has_numbered_volume(), "x.pfs.000/001 应被识别为分卷");
        assert!(s.has_dir_ending_with("content"), "Engine/Content 应能被后缀匹配到");
        assert!(!s.has_dir_ending_with("nope"));
        // 没有分卷时应为 false
        let d2 = tmpdir("numvol_none");
        std::fs::write(d2.join("x.pfs"), b"x").unwrap();
        assert!(!ScanCtx::build(&d2).has_numbered_volume());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }
}
