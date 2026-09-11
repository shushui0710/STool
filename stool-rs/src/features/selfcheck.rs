//! 封包自检（P2-6）：把过去靠人工「解包 → 回填 → 重打包 → 比对 `verify/*_diff.txt`」
//! 的验证流程产品化，一条命令确认 STool 对该封包的**读写是否无损**。
//!
//! 闭环（全程磁盘中转，避免把整包多份驻留内存）：
//! ```text
//! 原封包 ──解包──▶ 工作目录（逐条目落盘 + 记录 大小/FNV-1a 哈希）
//!                     │
//!                     └──重打包──▶ 新封包 ──解包──▶ 逐条目比对（大小 + 哈希）
//! ```
//! 比对的是**每个条目的内容**而非封包字节：重压缩 / 条目顺序 / TOC 布局允许变化，
//! 只要每条内容一致，就说明解包与回写没有损坏数据。
//!
//! 支持的类型（与 `restore::TOGGLE_EXTS` 的可回写封包一致）：XP3 / PCK / asar / RPA / RGSSAD v1·v3。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::features::precheck::{human_bytes, Item, Level, Report};
use crate::formats::source::{Source, MAX_ARCHIVE};
use crate::formats::{asar, pck, rpa, rgss, xp3};

/// 解包以整包读入内存的类型（xp3/pck/asar/rgss）超过此体积时直接拒绝，避免无谓的内存压力。
const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// RPA 重打包需要把全部条目读进内存（`write_archive` 是内存式 API）；超过此总量则跳过往返比对。
const RPA_INMEM_CAP: u64 = 512 * 1024 * 1024;

/// 可自检的封包类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Xp3,
    Pck,
    Asar,
    Rpa,
    RgssV1,
    RgssV3,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Xp3 => "XP3（KiriKiri）",
            Kind::Pck => "PCK（Godot）",
            Kind::Asar => "asar（Electron）",
            Kind::Rpa => "RPA（Ren'Py）",
            Kind::RgssV1 => "RGSSAD v1（RPG Maker XP/VX）",
            Kind::RgssV3 => "RGSSAD v3（RPG Maker VX Ace）",
        }
    }

    /// 重打包产物的扩展名（仅用于工作目录内命名，不影响格式）。
    fn ext(self) -> &'static str {
        match self {
            Kind::Xp3 => "xp3",
            Kind::Pck => "pck",
            Kind::Asar => "asar",
            Kind::Rpa => "rpa",
            Kind::RgssV1 => "rgssad",
            Kind::RgssV3 => "rgss3a",
        }
    }
}

/// 解包出的单个条目：归档内名字 + 落盘路径 + 大小 + 内容哈希。
#[derive(Debug, Clone)]
struct Entry {
    name: String,
    disk: PathBuf,
    size: u64,
    hash: u64,
}

/// 往返比对发现的不一致。
#[derive(Debug, Clone)]
pub struct Mismatch {
    pub name: String,
    pub reason: String,
}

/// 单个封包的自检结论。
#[derive(Debug, Clone)]
pub struct Outcome {
    pub archive: PathBuf,
    pub kind: Kind,
    /// 解出的条目数（asar 的 unpacked 文件等不计入）
    pub entries: usize,
    /// 解出的总字节数
    pub total_bytes: u64,
    /// 跳过的条目数（如 asar 外置文件）
    pub skipped: usize,
    /// 重打包是否因体积被跳过（仅验证「可完整解包」）
    pub skipped_repack: bool,
    pub repack_bytes: u64,
    pub mismatches: Vec<Mismatch>,
    pub elapsed_ms: u128,
}

impl Outcome {
    /// 是否无损（重打包被跳过时以「解包成功」为准）。
    pub fn ok(&self) -> bool {
        self.mismatches.is_empty()
    }

    /// 统一为 `precheck::Report`，便于 CLI / GUI 复用同一套打印与判定。
    pub fn report(&self) -> Report {
        let file = self
            .archive
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut items = Vec::new();
        items.push(Item {
            name: "封包",
            level: Level::Ok,
            detail: format!(
                "{file} · {} · {} 条目 · {}",
                self.kind.label(),
                self.entries,
                human_bytes(self.total_bytes)
            ),
            fix: None,
        });
        if self.skipped > 0 {
            items.push(Item {
                name: "外置条目",
                level: Level::Warn,
                detail: format!("{} 个条目存放于封包外（如 asar.unpacked），已跳过", self.skipped),
                fix: None,
            });
        }
        if self.entries == 0 {
            items.push(Item {
                name: "解包",
                level: Level::Warn,
                detail: "封包内没有可提取条目（可能是空包，或索引被加密的保护变体）".into(),
                fix: Some("确认是否为受保护变体；必要时改用该引擎的专用工具解包".into()),
            });
        } else {
            items.push(Item {
                name: "解包",
                level: Level::Ok,
                detail: format!("{} 个条目全部成功解出", self.entries),
                fix: None,
            });
        }
        if self.skipped_repack {
            items.push(Item {
                name: "重打包",
                level: Level::Warn,
                detail: "封包过大，已跳过往返比对（仅验证了可以完整解包）".into(),
                fix: Some("如需完整校验，可对单个较小的封包单独运行 selfcheck".into()),
            });
        } else {
            items.push(Item {
                name: "重打包",
                level: Level::Ok,
                detail: format!("已重建（{}）", human_bytes(self.repack_bytes)),
                fix: None,
            });
            if self.mismatches.is_empty() {
                items.push(Item {
                    name: "往返比对",
                    level: Level::Ok,
                    detail: format!("{} 个条目逐一一致（大小 + FNV-1a 64 哈希）", self.entries),
                    fix: None,
                });
            } else {
                let show = self
                    .mismatches
                    .iter()
                    .take(5)
                    .map(|m| format!("{}（{}）", m.name, m.reason))
                    .collect::<Vec<_>>()
                    .join("；");
                items.push(Item {
                    name: "往返比对",
                    level: Level::Fail,
                    detail: format!("{} 处不一致：{show}", self.mismatches.len()),
                    fix: Some("属解析/回写器缺陷：请记录封包名与不一致条目；该封包暂勿走 STool 回写流程".into()),
                });
            }
        }
        items.push(Item {
            name: "耗时",
            level: Level::Ok,
            detail: format!("{} ms", self.elapsed_ms),
            fix: None,
        });
        Report { items }
    }
}

/// 依据魔数（并回退到扩展名）判定封包类型。无法识别返回 None。
pub fn sniff(path: &Path) -> Option<Kind> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let mut buf = [0u8; 16];
    let n = fs::File::open(path).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    let head = &buf[..n];
    if head.starts_with(b"XP3") {
        return Some(Kind::Xp3);
    }
    if head.starts_with(b"GDPC") {
        return Some(Kind::Pck);
    }
    if head.starts_with(b"RPA-") {
        return Some(Kind::Rpa);
    }
    if head.starts_with(b"RGSSAD\x00") {
        return Some(if head.get(7) == Some(&3) { Kind::RgssV3 } else { Kind::RgssV1 });
    }
    if ext == "asar" && n >= 4 && u32::from_le_bytes(buf[0..4].try_into().unwrap()) == 4 {
        return Some(Kind::Asar);
    }
    match ext.as_str() {
        "xp3" => Some(Kind::Xp3),
        "pck" => Some(Kind::Pck),
        "asar" => Some(Kind::Asar),
        "rpa" => Some(Kind::Rpa),
        "rgssad" | "rgss2a" => Some(Kind::RgssV1),
        "rgss3a" => Some(Kind::RgssV3),
        _ => None,
    }
}

/// 列出游戏根目录顶层的可自检封包（不递归，避免把素材目录里的碎包也卷进来）。
pub fn find_archives(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for e in fs::read_dir(root).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_file() && sniff(&p).is_some() {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// 默认工作目录（系统临时目录下的进程级子目录）。
pub fn default_work_root() -> PathBuf {
    std::env::temp_dir().join(format!("stool_selfcheck_{}", std::process::id()))
}

/// 自检单个封包。`work_root` 下会临时落盘，结束时清理。
pub fn check_archive(archive: &Path, work_root: &Path) -> Result<Outcome, String> {
    let t0 = Instant::now();
    if !archive.is_file() {
        return Err(format!("不是文件: {}", archive.display()));
    }
    let kind = sniff(archive).ok_or_else(|| format!("无法识别的封包类型: {}", archive.display()))?;
    let size = fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
    if !matches!(kind, Kind::Rpa) && size > MAX_ARCHIVE_BYTES {
        return Err(format!("封包过大（{}），自检需要整包读入内存，已跳过", human_bytes(size)));
    }

    let stem = archive
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".into());
    let base_dir = work_root.join(format!("{stem}_{}", short_hash(archive.to_string_lossy().as_bytes())));
    let _ = fs::remove_dir_all(&base_dir);
    fs::create_dir_all(&base_dir).map_err(|e| format!("创建自检工作目录失败: {e}"))?;

    // 无论成功 / 失败都清理工作目录，避免大封包解包残留占盘。
    let result = roundtrip(kind, archive, &base_dir);
    let _ = fs::remove_dir_all(&base_dir);
    let r = result?;

    Ok(Outcome {
        archive: archive.to_path_buf(),
        kind,
        entries: r.entries,
        total_bytes: r.total_bytes,
        skipped: extract_skipped(kind, archive),
        skipped_repack: r.skipped_repack,
        repack_bytes: r.repack_bytes,
        mismatches: r.mismatches,
        elapsed_ms: t0.elapsed().as_millis(),
    })
}

/// 往返校验的中间结果（工作目录已由 [`check_archive`] 负责清理）。
struct Roundtrip {
    entries: usize,
    total_bytes: u64,
    skipped_repack: bool,
    repack_bytes: u64,
    mismatches: Vec<Mismatch>,
}

/// 1) 解包到磁盘 → 2) 重打包 → 3) 回读 → 4) 逐条目比对。
fn roundtrip(kind: Kind, archive: &Path, base_dir: &Path) -> Result<Roundtrip, String> {
    let dir_extract = base_dir.join("extract");
    let dir_verify = base_dir.join("verify");
    let new_archive = base_dir.join(format!("repack.{}", kind.ext()));

    let recs_a = extract_all(kind, archive, &dir_extract)?;
    let entries = recs_a.len();
    let total_bytes: u64 = recs_a.iter().map(|e| e.size).sum();

    let mut skipped_repack = false;
    let mut repack_bytes = 0u64;
    let mut mismatches = Vec::new();
    if kind == Kind::Rpa && total_bytes > RPA_INMEM_CAP {
        skipped_repack = true; // 内存式写 API，超阈值只验「可完整解包」
    } else if entries > 0 {
        repack(kind, &dir_extract, &recs_a, &new_archive)?;
        repack_bytes = fs::metadata(&new_archive).map(|m| m.len()).unwrap_or(0);
        let recs_b = extract_all(kind, &new_archive, &dir_verify)?;
        mismatches = compare(&recs_a, &recs_b);
    }
    Ok(Roundtrip { entries, total_bytes, skipped_repack, repack_bytes, mismatches })
}

/// 自检目录下所有顶层封包，汇总为一份报告。
pub fn check_dir(root: &Path, work_root: &Path) -> Report {
    let mut items = Vec::new();
    if !root.is_dir() {
        items.push(Item {
            name: "游戏目录",
            level: Level::Fail,
            detail: format!("不存在或不是目录: {}", root.display()),
            fix: Some("选择含封包的游戏根目录".into()),
        });
        return Report { items };
    }
    let archives = find_archives(root);
    if archives.is_empty() {
        items.push(Item {
            name: "封包",
            level: Level::Warn,
            detail: format!("{} 顶层未发现可自检的封包", root.display()),
            fix: Some("直接指定封包文件路径，或把待检封包放到该目录下".into()),
        });
        return Report { items };
    }
    let _ = fs::create_dir_all(work_root);
    for a in &archives {
        match check_archive(a, work_root) {
            Ok(o) => {
                let tag = a.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                for mut it in o.report().items {
                    it.detail = format!("[{tag}] {}", it.detail);
                    items.push(it);
                }
            }
            Err(e) => items.push(Item {
                name: "封包",
                level: Level::Fail,
                detail: e,
                fix: Some("确认该文件是否受支持、是否损坏或为加密变体".into()),
            }),
        }
    }
    let _ = fs::remove_dir_all(work_root);
    Report { items }
}

// ---------------------------------------------------------------------------
// 解包 / 重打包 / 比对
// ---------------------------------------------------------------------------

/// 逐条目解包到 `dir`，返回记录（含大小与哈希）。
///
/// 走 `Source` 流式读：509MB 的 rgss3a 也只需常驻「一条目」的内存，
/// 而不是像以前那样把整包读进堆里再逐条切片。
fn extract_all(kind: Kind, archive: &Path, dir: &Path) -> Result<Vec<Entry>, String> {
    fs::create_dir_all(dir).map_err(|e| format!("创建解包目录失败: {e}"))?;
    let mut ex = Extractor::new(dir);
    if kind == Kind::Rpa {
        // RPA 的索引本就在文件尾，`read_index` 已是流式（seek + 读尾部）。
        let idx = rpa::read_index(archive)?;
        for (name, chunks) in &idx.entries {
            let bytes = rpa::read_file(archive, chunks)?;
            ex.put(name, &bytes)?;
        }
        return Ok(ex.out);
    }
    let mut src = Source::open(archive, MAX_ARCHIVE)?;
    match kind {
        Kind::Xp3 => {
            let index = xp3::parse_index(&mut src)?;
            // 加密条目我们读出来就是乱码，比对"乱码==乱码"会得出假的"无损"结论
            let enc = xp3::encrypted_count(&index);
            if enc > 0 {
                return Err(format!(
                    "{enc}/{} 个条目被 KiriKiri 加密方案保护，STool 不解密，无法做无损自检（请先用 GARbro 等导出明文）",
                    index.len()
                ));
            }
            for (name, entry) in &index {
                let bytes = xp3::read_entry(&mut src, entry)?;
                ex.put(name, &bytes)?;
            }
        }
        Kind::Pck => {
            for e in &pck::parse_index(&mut src)? {
                let bytes = pck::read_entry(&mut src, e)?;
                ex.put(&e.path, &bytes)?;
            }
        }
        Kind::Asar => {
            let (files, data_start) = asar::parse_index(&mut src)?;
            for (rel, node) in &files {
                if node.unpacked {
                    continue; // 外置条目（app.asar.unpacked）不参与
                }
                let bytes = asar::read_entry(&mut src, data_start, node)?;
                ex.put(rel, &bytes)?;
            }
        }
        Kind::RgssV1 => {
            for (name, (off, size, key)) in &rgss::parse_index_v1(&mut src)? {
                let bytes = rgss::read_entry_v1(&mut src, *off, *size, *key)?;
                ex.put(name, &bytes)?;
            }
        }
        Kind::RgssV3 => {
            for e in &rgss::parse_index_v3(&mut src)? {
                let bytes = rgss::read_entry_v3(&mut src, e.offset, e.size, e.filekey)?;
                ex.put(&e.name, &bytes)?;
            }
        }
        Kind::Rpa => unreachable!("RPA 已在上面提前返回"),
    }
    Ok(ex.out)
}

/// 用解包产物重建封包（名字沿用归档原名，保证条目名无损）。
fn repack(kind: Kind, extract_dir: &Path, recs: &[Entry], new_archive: &Path) -> Result<usize, String> {
    let name_map: BTreeMap<String, PathBuf> =
        recs.iter().map(|e| (e.name.clone(), e.disk.clone())).collect();
    match kind {
        Kind::Xp3 => xp3::write_paths(new_archive, &name_map),
        Kind::Pck => pck::write_v1_paths(new_archive, &name_map),
        Kind::Asar => asar::pack(extract_dir, new_archive),
        Kind::RgssV1 => {
            rgss::write_v1_paths(new_archive, &name_map)?;
            Ok(recs.len())
        }
        Kind::RgssV3 => {
            rgss::write_v3_paths(new_archive, &name_map)?;
            Ok(recs.len())
        }
        Kind::Rpa => {
            let mut map: BTreeMap<String, Vec<u8>> = BTreeMap::new();
            for e in recs {
                let bytes = fs::read(&e.disk).map_err(|e| e.to_string())?;
                map.insert(e.name.clone(), bytes);
            }
            rpa::write_archive(new_archive, &map, 0)?;
            Ok(map.len())
        }
    }
}

/// asar 的外置条目数（用于报告，避免重复解析：失败即 0）。
fn extract_skipped(kind: Kind, archive: &Path) -> usize {
    if kind != Kind::Asar {
        return 0;
    }
    let Ok(mut src) = Source::open(archive, MAX_ARCHIVE) else { return 0 };
    match asar::parse_index(&mut src) {
        Ok((files, _)) => files.values().filter(|n| n.unpacked).count(),
        Err(_) => 0,
    }
}

/// 逐条目比对（名字按分隔符归一：`\` 与 `/` 等价，避免 RGSS 写出时的分隔符差异误报）。
fn compare(a: &[Entry], b: &[Entry]) -> Vec<Mismatch> {
    let mut mb: HashMap<String, &Entry> = HashMap::new();
    for e in b {
        mb.insert(norm_name(&e.name), e);
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for ea in a {
        let k = norm_name(&ea.name);
        seen.insert(k.clone());
        match mb.get(&k) {
            None => out.push(Mismatch { name: ea.name.clone(), reason: "重打包后缺失".into() }),
            Some(eb) => {
                if eb.size != ea.size {
                    out.push(Mismatch {
                        name: ea.name.clone(),
                        reason: format!("大小变化 {} → {}", ea.size, eb.size),
                    });
                } else if eb.hash != ea.hash {
                    out.push(Mismatch { name: ea.name.clone(), reason: "内容校验和不一致".into() });
                }
            }
        }
    }
    for eb in b {
        if !seen.contains(&norm_name(&eb.name)) {
            out.push(Mismatch { name: eb.name.clone(), reason: "重打包后多出".into() });
        }
    }
    out
}

fn norm_name(n: &str) -> String {
    n.replace('\\', "/")
}

/// 把归档内条目名映射为工作目录内**安全**的相对路径（防目录穿越 / 空名 / 重名冲突）。
fn unique_rel(name: &str, used: &mut HashSet<String>) -> String {
    let cleaned = name.trim_start_matches("res://").replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for part in cleaned.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        parts.push(part);
    }
    let mut rel = if parts.is_empty() { "__unnamed__".to_string() } else { parts.join("/") };
    if !used.insert(rel.clone()) {
        let mut i = 1usize;
        loop {
            let cand = format!("{rel}.{i}");
            if used.insert(cand.clone()) {
                rel = cand;
                break;
            }
            i += 1;
        }
    }
    rel
}

fn hash64(bytes: &[u8]) -> u64 {
    // FNV-1a 64：无依赖、稳定，足够做内容一致性比对（非对抗场景）
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn short_hash(bytes: &[u8]) -> String {
    format!("{:08x}", hash64(bytes) as u32)
}

/// 解包落盘辅助：去重命名 + 记录。
struct Extractor {
    dir: PathBuf,
    used: HashSet<String>,
    out: Vec<Entry>,
}

impl Extractor {
    fn new(dir: &Path) -> Self {
        Extractor { dir: dir.to_path_buf(), used: HashSet::new(), out: Vec::new() }
    }

    fn put(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let rel = unique_rel(name, &mut self.used);
        let path = self.dir.join(&rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
        }
        fs::write(&path, bytes).map_err(|e| format!("写出 {} 失败: {e}", path.display()))?;
        self.out.push(Entry {
            name: name.to_string(),
            disk: path,
            size: bytes.len() as u64,
            hash: hash64(bytes),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_selfcheck_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample() -> BTreeMap<String, Vec<u8>> {
        let mut m = BTreeMap::new();
        m.insert("Data/readme.txt".to_string(), b"hello stool".to_vec());
        m.insert("Data/big.bin".to_string(), vec![7u8; 4096]);
        m.insert("script.rpy".to_string(), b"label start:\n  return\n".to_vec());
        m
    }

    #[test]
    fn xp3_roundtrip_is_lossless() {
        let d = tmpdir("xp3");
        let arc = d.join("data.xp3");
        xp3::write(&arc, &sample()).unwrap();
        assert_eq!(sniff(&arc), Some(Kind::Xp3));
        let o = check_archive(&arc, &d.join("work")).unwrap();
        assert_eq!(o.entries, 3);
        assert!(o.mismatches.is_empty(), "{:?}", o.mismatches);
        assert!(o.ok() && o.report().ok());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn pck_roundtrip_is_lossless() {
        let d = tmpdir("pck");
        let arc = d.join("game.pck");
        let mut files = BTreeMap::new();
        files.insert("res://icon.png".to_string(), vec![1u8, 2, 3, 4]);
        files.insert("res://a/b.txt".to_string(), b"pck body".to_vec());
        pck::write_v1(&arc, &files).unwrap();
        assert_eq!(sniff(&arc), Some(Kind::Pck));
        let o = check_archive(&arc, &d.join("work")).unwrap();
        assert_eq!(o.entries, 2);
        assert!(o.mismatches.is_empty(), "{:?}", o.mismatches);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn rgss_v1_and_v3_roundtrip() {
        let d = tmpdir("rgss");
        // v1：真实归档名字用反斜杠
        let mut files = BTreeMap::new();
        files.insert("Data\\System.rvdata".to_string(), b"sys".to_vec());
        files.insert("Graphics\\Title.png".to_string(), vec![9u8; 2048]);
        let v1 = d.join("Game.rgssad");
        rgss::write_v1(&v1, &files).unwrap();
        assert_eq!(sniff(&v1), Some(Kind::RgssV1));
        let o1 = check_archive(&v1, &d.join("work")).unwrap();
        assert_eq!(o1.entries, 2);
        assert!(o1.mismatches.is_empty(), "{:?}", o1.mismatches);

        let v3 = d.join("Game.rgss3a");
        rgss::write_v3(&v3, &files).unwrap();
        assert_eq!(sniff(&v3), Some(Kind::RgssV3));
        let o3 = check_archive(&v3, &d.join("work")).unwrap();
        assert_eq!(o3.entries, 2);
        assert!(o3.mismatches.is_empty(), "{:?}", o3.mismatches);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn rpa_roundtrip_is_lossless() {
        let d = tmpdir("rpa");
        let arc = d.join("archive.rpa");
        rpa::write_archive(&arc, &sample(), 0xABCD1234).unwrap();
        assert_eq!(sniff(&arc), Some(Kind::Rpa));
        let o = check_archive(&arc, &d.join("work")).unwrap();
        assert_eq!(o.entries, 3);
        assert!(o.mismatches.is_empty(), "{:?}", o.mismatches);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn asar_roundtrip_is_lossless() {
        let d = tmpdir("asar");
        let src = d.join("app");
        fs::create_dir_all(src.join("dist")).unwrap();
        fs::write(src.join("package.json"), b"{\"name\":\"x\"}").unwrap();
        fs::write(src.join("dist/index.html"), b"<html></html>").unwrap();
        let arc = d.join("app.asar");
        asar::pack(&src, &arc).unwrap();
        assert_eq!(sniff(&arc), Some(Kind::Asar));
        let o = check_archive(&arc, &d.join("work")).unwrap();
        assert_eq!(o.entries, 2);
        assert!(o.mismatches.is_empty(), "{:?}", o.mismatches);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_member_is_detected() {
        // 人工构造「重打包后内容不一致」：解包记录后，篡改工作目录里的文件再比对
        let d = tmpdir("mism");
        let arc = d.join("data.xp3");
        xp3::write(&arc, &sample()).unwrap();
        let work = d.join("work");
        let kind = sniff(&arc).unwrap();
        let dir = work.join("x/extract");
        let recs = extract_all(kind, &arc, &dir).unwrap();
        // 篡改其中一个已落盘条目
        let victim = recs.iter().find(|e| e.name.ends_with("readme.txt")).unwrap();
        fs::write(&victim.disk, b"tampered!!!").unwrap();
        let new_arc = work.join("x/repack.xp3");
        repack(kind, &dir, &recs, &new_arc).unwrap();
        let back = extract_all(kind, &new_arc, &work.join("x/verify")).unwrap();
        let mism = compare(&recs, &back);
        assert_eq!(mism.len(), 1, "{mism:?}");
        assert!(mism[0].reason.contains("不一致"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn error_path_leaves_no_work_dir() {
        // 损坏封包（魔数对但内容非法）→ check_archive 报错，且不残留工作目录
        let d = tmpdir("cleanup");
        // 扩展名判为 XP3，但内容不是封包 → 解析必然失败
        let arc = d.join("broken.xp3");
        fs::write(&arc, b"this is definitely not an xp3 archive body").unwrap();
        let work = d.join("work");
        let r = check_archive(&arc, &work);
        assert!(r.is_err(), "损坏封包应报错");
        let leftover = fs::read_dir(&work)
            .map(|rd| rd.flatten().count())
            .unwrap_or(0);
        assert_eq!(leftover, 0, "报错后工作目录应被清理");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn sniff_rejects_unknown() {
        let d = tmpdir("sniff");
        let p = d.join("readme.txt");
        fs::write(&p, b"not an archive").unwrap();
        assert_eq!(sniff(&p), None);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unique_rel_blocks_traversal_and_collisions() {
        let mut used = HashSet::new();
        assert_eq!(unique_rel("../../etc/passwd", &mut used), "etc/passwd");
        assert_eq!(unique_rel("res://a/b.txt", &mut used), "a/b.txt");
        // 与上一次同名 → 追加序号
        let second = unique_rel("a/b.txt", &mut used);
        assert_ne!(second, "a/b.txt");
        assert_eq!(unique_rel("..", &mut used), "__unnamed__");
    }

    #[test]
    fn check_dir_reports_on_multiple_archives() {
        let d = tmpdir("dir");
        xp3::write(&d.join("a.xp3"), &sample()).unwrap();
        let mut files = BTreeMap::new();
        files.insert("res://x.y".to_string(), b"z".to_vec());
        pck::write_v1(&d.join("b.pck"), &files).unwrap();
        let rep = check_dir(&d, &d.join("work"));
        assert!(rep.ok(), "{:?}", rep.lines());
        assert!(rep.items.iter().any(|i| i.detail.contains("a.xp3")));
        assert!(rep.items.iter().any(|i| i.detail.contains("b.pck")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn check_dir_warns_when_no_archive() {
        let d = tmpdir("empty");
        fs::write(d.join("readme.txt"), b"x").unwrap();
        let rep = check_dir(&d, &d.join("work"));
        assert!(rep.ok());
        assert!(rep.items.iter().any(|i| i.level == Level::Warn));
        let _ = fs::remove_dir_all(&d);
    }
}
