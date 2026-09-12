//! Unity 画廊 / CG 全解锁（PlayerPrefs 注册表路线）。
//!
//! 依据《game-unpacker / references/unity-godot.md》的实测经验实现：
//!
//! - **PlayerPrefs 位置**：`HKCU\Software\<company>\<product>`，
//!   company / product 取自 `<游戏>_Data/app.info` 的前两行。
//! - **值名格式**：`<原始键名>_h<djb2 哈希>`；`SetInt` → `REG_DWORD`，
//!   `SetString` → `REG_BINARY`（UTF-8 + `\0`）。
//! - **哈希**：djb2 变体，按 UTF-8 字节，32 位无符号溢出（见 [`prefs_hash`]）。
//! - **自检**：用注册表里**已存在**的真实键反推哈希，对得上才动手写——
//!   这是避免"写一堆无效键"的关键。
//! - **键名来源（两级）**：
//!   1. **精确**：Mono 版的 `<游戏>_Data/Managed/Assembly-CSharp.dll` 是标准 .NET
//!      程序集，`PlayerPrefs.SetInt("cg_flag_01", 1)` 的**字面量**就躺在 `#US` 堆里，
//!      直接读出来即可（见 [`scan_assemblies`] → `formats::dotnet`）。零依赖、不需要
//!      反编译器、不执行任何代码。
//!   2. **兜底**：场景 / 资源二进制里的 ASCII 串（IL2CPP 版没有 C# 程序集，
//!      `assembly-csharp.dll` 不存在，此时仍用启发式扫描）。
//! - **优先策略**：很多作品自带隐藏的"全开开关"，比逐个写键更彻底；
//!   本模块会在报告里提示这一点。
//!
//! 之所以走注册表而不是改 dll：**Mono 与 IL2CPP 都适用**，且不动游戏文件、
//! 无完整性校验风险。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::engines::{Ctx, OpOutcome};
use crate::formats::{dotnet, il2cpp};

// ---------------------------------------------------------------------------
// 哈希与命名
// ---------------------------------------------------------------------------

/// 注册表 `REG_DWORD` 的类型值（Windows 的 `REG_DWORD` 常量 = 4）。
/// 独立成常量以便非 Windows 平台也能编译同一套判断逻辑。
const REG_DWORD_TYPE: u32 = 4;

/// Unity PlayerPrefs 的 djb2 变体哈希（按 UTF-8 字节，32 位无符号溢出）。
///
/// ```text
/// h = 5381
/// for b in key.utf8: h = ((h * 33) ^ b) & 0xFFFFFFFF
/// ```
pub fn prefs_hash(key: &str) -> u32 {
    let mut h: u32 = 5381;
    for b in key.as_bytes() {
        h = h.wrapping_mul(33) ^ (*b as u32);
    }
    h
}

/// 注册表值名：`<原始键名>_h<哈希>`。
pub fn prefs_value_name(key: &str) -> String {
    format!("{key}_h{}", prefs_hash(key))
}

/// 把一个注册表值名（`xxx_h123456`）反解出原始键名与内嵌哈希。
/// 非该形态返回 `None`。
pub fn split_value_name(value_name: &str) -> Option<(&str, u32)> {
    let (raw, hash) = value_name.rsplit_once("_h")?;
    if raw.is_empty() || hash.is_empty() || !hash.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((raw, hash.parse().ok()?))
}

// ---------------------------------------------------------------------------
// app.info / 数据目录
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub company: String,
    pub product: String,
}

/// 定位 `<游戏>_Data` 目录（大小写不敏感）。
pub fn find_data_dir(root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for e in entries.flatten() {
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if name.to_ascii_lowercase().ends_with("_data") {
            return Some(e.path());
        }
    }
    None
}

/// 读取 `<游戏>_Data/app.info` 的 company / product（前两个非空行）。
pub fn read_app_info(root: &Path) -> Option<AppInfo> {
    let dir = find_data_dir(root)?;
    let text = fs::read_to_string(dir.join("app.info")).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let company = lines.next()?.to_string();
    let product = lines.next()?.to_string();
    Some(AppInfo { company, product })
}

// ---------------------------------------------------------------------------
// 候选键名提取（场景 / 资源文件里的字符串）
// ---------------------------------------------------------------------------

/// Unity 场景 / 资源二进制里的常见噪声（命中即丢弃）。
const NOISE: &[&str] = &[
    "m_Script",
    "MonoBehaviour",
    "UnityEngine",
    "UnityEditor",
    "Assets/",
    "Library/",
    "Packages/",
    "TextMeshPro",
    "Cinemachine",
    "DOTween",
    "Shader ",
    "Hidden/",
    "Sprites/",
    "UberShader",
    "Assembly-",
    "System.",
    "Microsoft.",
    "http://",
    "https://",
];

/// 猜测"像画廊键"的字符串。
///
/// 规则（刻意保守，宁可少不可错）：
/// - 长度 3..=64；至少含一个 ASCII 字母；
/// - 只允许 `[A-Za-z0-9_ .-]` 与非 ASCII（CJK 等）——**键名不会含** `()` `{}` `:` `/`
///   `%` `=` 等符号。这条专治 IL2CPP 字面量池里的框架格式串
///   （`" (offset:"`、`"{0} --> {1}"`）；
/// - **首、末字符**必须是字母/数字/`_`/非 ASCII，且不含连续空格 —— 键名不会是
///   `"-   q"`、`"Pass Culling Disabled -"` 这种被空格/标点包住的串；
/// - 不是纯十六进制串（GUID / hash 噪声）；
/// - 不含 `NOISE` 里的框架名。
pub fn looks_like_key(s: &str) -> bool {
    if s.len() < 3 || s.len() > 64 {
        return false;
    }
    if !s.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if !s.chars().next().map(is_key_edge).unwrap_or(false) {
        return false;
    }
    if !s.chars().next_back().map(is_key_edge).unwrap_or(false) {
        return false;
    }
    if !s.chars().all(is_key_char) {
        return false;
    }
    if s.contains("  ") {
        return false;
    }
    if NOISE.iter().any(|n| s.contains(n)) {
        return false;
    }
    // base64 / base64url 常量块（protobuf 描述符等）：长、只含 base64 字符集、
    // 长度是 4 的倍数、且大小写数字齐全。这类串常以 `Cg1`（protobuf 字段 1 定长）
    // 开头，会骗过 `cg`+数字的判据。游戏里的 PlayerPrefs 键不会长成这样。
    if s.len() >= 24
        && s.len().is_multiple_of(4)
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        let (mut lower, mut upper, mut digit) = (false, false, false);
        for b in s.bytes() {
            if b.is_ascii_lowercase() {
                lower = true;
            } else if b.is_ascii_uppercase() {
                upper = true;
            } else {
                digit = true;
            }
        }
        if lower && upper && digit {
            return false;
        }
    }
    // 纯 hex（>=8 位）视为 GUID/hash 噪声
    if s.len() >= 8 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    true
}

/// 键名**中间**允许出现的字符：ASCII 字母/数字、`_`、`.`、`-`、空格，以及非 ASCII（CJK 等）。
fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ' ') || !c.is_ascii()
}

/// 键名**首/末**允许出现的字符：字母/数字/`_`/非 ASCII（不含空格、`-`、`.`）。
fn is_key_edge(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii()
}

/// 从一段二进制里抽出所有可打印 ASCII 串（3..=80 字节）。
fn extract_ascii_runs(blob: &[u8], out: &mut BTreeSet<String>) {
    let mut i = 0usize;
    while i < blob.len() {
        if (0x20..=0x7e).contains(&blob[i]) {
            let start = i;
            while i < blob.len() && (0x20..=0x7e).contains(&blob[i]) {
                i += 1;
            }
            let run = &blob[start..i];
            if (3..=80).contains(&run.len()) {
                if let Ok(t) = std::str::from_utf8(run) {
                    let t = t.trim();
                    if looks_like_key(t) {
                        out.insert(t.to_string());
                    }
                }
            }
        } else {
            i += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// 精确字符串来源：Mono 程序集（.NET 元数据）/ IL2CPP 元数据
// ---------------------------------------------------------------------------

/// 精确来源的类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreciseKind {
    /// Mono 版：`*_Data/Managed/Assembly-CSharp*.dll`（.NET `#US` / `#Strings` 堆）。
    Dotnet,
    /// IL2CPP 版：`*_Data/il2cpp_data/Metadata/global-metadata.dat`。
    Il2Cpp,
}

impl PreciseKind {
    /// 给用户看的名字。
    pub fn label(self) -> &'static str {
        match self {
            PreciseKind::Dotnet => "程序集",
            PreciseKind::Il2Cpp => "IL2CPP 元数据",
        }
    }
}

/// 精确字符串来源的扫描结果。
#[derive(Debug, Clone, Default)]
pub struct PreciseScan {
    /// 成功解析的来源（路径 + 类别）。
    pub sources: Vec<(PathBuf, PreciseKind)>,
    /// 找到但解析失败的（路径, 原因）。不致命，退回启发式即可。
    pub failures: Vec<(PathBuf, String)>,
    /// 字面量里筛出来的"像键名"的字符串。
    pub keys: Vec<String>,
    /// 命中的画廊类型 / 字段名（如 `GalleryManager`、`_wholeNameList`）。
    pub gallery_types: Vec<String>,
}

impl PreciseScan {
    /// 是否有可用的精确来源。
    pub fn usable(&self) -> bool {
        !self.sources.is_empty()
    }

    /// 是否来自 IL2CPP（字面量池里框架串较多，报告里要提示收窄）。
    pub fn is_il2cpp(&self) -> bool {
        self.sources.iter().any(|(_, k)| *k == PreciseKind::Il2Cpp)
    }
}

/// 在 `<_Data>/Managed/` 下找 `Assembly-CSharp*.dll`（大小写不敏感）。
///
/// 主程序集 `Assembly-CSharp.dll` 排第一，其余（如 `Assembly-CSharp-firstpass.dll`）
/// 按名字排序跟在后面。
pub fn find_assemblies(root: &Path) -> Vec<PathBuf> {
    let Some(data_dir) = find_data_dir(root) else {
        return Vec::new();
    };
    let Some(managed) = read_dir_find_dir(&data_dir, "managed") else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&managed) else {
        return Vec::new();
    };
    let mut rest: Vec<PathBuf> = Vec::new();
    let mut main: Option<PathBuf> = None;
    for e in entries.flatten() {
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let low = e.file_name().to_string_lossy().to_ascii_lowercase();
        if !low.ends_with(".dll") || !low.starts_with("assembly-csharp") {
            continue;
        }
        if low == "assembly-csharp.dll" {
            main = Some(e.path());
        } else {
            rest.push(e.path());
        }
    }
    rest.sort();
    let mut out: Vec<PathBuf> = Vec::with_capacity(rest.len() + 1);
    if let Some(m) = main {
        out.push(m);
    }
    out.extend(rest);
    out
}

/// 定位 IL2CPP 元数据 `*_Data/il2cpp_data/Metadata/global-metadata.dat`。
pub fn find_il2cpp_metadata(root: &Path) -> Option<PathBuf> {
    let data_dir = find_data_dir(root)?;
    let meta = read_dir_find_dir(&data_dir, "il2cpp_data")
        .and_then(|d| read_dir_find_dir(&d, "metadata"))?;
    let p = meta.join("global-metadata.dat");
    p.is_file().then_some(p)
}

/// 在 `dir` 下按名字（大小写不敏感）找一级子目录。
fn read_dir_find_dir(dir: &Path, name_lower: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if e.file_name().to_string_lossy().to_ascii_lowercase() == name_lower {
            return Some(e.path());
        }
    }
    None
}

/// 读所有精确来源：Mono 程序集（优先）；一个都没读到再试 IL2CPP 元数据。
///
/// 只读元数据、不反编译、不加载程序集。任一文件失败都不致命——记入
/// [`PreciseScan::failures`] 后继续。
pub fn scan_precise(root: &Path) -> PreciseScan {
    let mut out = PreciseScan::default();
    let mut keys: BTreeSet<String> = BTreeSet::new();
    let mut types: BTreeSet<String> = BTreeSet::new();

    // Mono：`Assembly-CSharp*.dll`
    for p in find_assemblies(root) {
        match dotnet::read_strings(&p) {
            Ok(s) => {
                collect_literals(s.user_strings.iter(), &mut keys);
                for t in s.gallery_hits() {
                    types.insert(t);
                }
                out.sources.push((p, PreciseKind::Dotnet));
            }
            Err(e) => out.failures.push((p, e)),
        }
    }

    // IL2CPP：`global-metadata.dat`（仅在没有 Mono 程序集时才读，避免重复扫描）
    if out.sources.is_empty() {
        if let Some(p) = find_il2cpp_metadata(root) {
            match il2cpp::read_strings(&p) {
                Ok(s) => {
                    collect_literals(s.literals.iter(), &mut keys);
                    for t in s.gallery_hits() {
                        types.insert(t);
                    }
                    out.sources.push((p, PreciseKind::Il2Cpp));
                }
                Err(e) => out.failures.push((p, e)),
            }
        }
    }

    out.keys = keys.into_iter().collect();
    out.gallery_types = types.into_iter().collect();
    out
}

/// 把一组字面量里"像键名"的收进 `keys`（trim + 过滤 + 去重）。
fn collect_literals<'a>(literals: impl Iterator<Item = &'a String>, keys: &mut BTreeSet<String>) {
    for k in literals {
        let t = k.trim();
        if looks_like_key(t) {
            keys.insert(t.to_string());
        }
    }
}

/// 供报告展示：精确来源的一句话摘要。
fn precise_note(scan: &PreciseScan) -> String {
    if scan.usable() {
        let names: Vec<String> = scan
            .sources
            .iter()
            .map(|(p, k)| {
                format!(
                    "{}({})",
                    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    k.label()
                )
            })
            .collect();
        let mut s = format!(
            "精确字符串来源: {} 个（{}）→ 候选 {} 条",
            scan.sources.len(),
            names.join(", "),
            scan.keys.len()
        );
        if !scan.gallery_types.is_empty() {
            let preview: Vec<&String> = scan.gallery_types.iter().take(6).collect();
            s.push_str(&format!(
                "\n已识别画廊类型/字段 {} 个: {preview:?}",
                scan.gallery_types.len()
            ));
        }
        for (p, e) in &scan.failures {
            s.push_str(&format!("\n⚠ 解析失败 {}: {e}", p.display()));
        }
        s
    } else if let Some((p, e)) = scan.failures.first() {
        format!("⚠ 精确来源解析失败（已退回场景启发式）{}: {e}", p.display())
    } else {
        "精确来源: 未找到 Managed/Assembly-CSharp*.dll 或 il2cpp_data/Metadata/global-metadata.dat，已退回场景启发式".into()
    }
}

// ---------------------------------------------------------------------------
// 候选键名汇总
// ---------------------------------------------------------------------------

/// 扫描候选键名（精确来源 + 场景启发式），去重后保持稳定顺序。
pub fn collect_candidate_keys(root: &Path) -> Vec<String> {
    collect_candidate_keys_detailed(root).0
}

/// 同 [`collect_candidate_keys`]，另外返回精确来源扫描详情（供报告展示）。
pub fn collect_candidate_keys_detailed(root: &Path) -> (Vec<String>, PreciseScan) {
    let mut set: BTreeSet<String> = BTreeSet::new();
    scan_scene_strings(root, &mut set);
    let scan = scan_precise(root);
    for k in &scan.keys {
        if looks_like_key(k) {
            set.insert(k.clone());
        }
    }
    (set.into_iter().collect(), scan)
}

/// 扫描 `<游戏>_Data/` 下的场景与资源文件，收集候选画廊键名。
///
/// 单文件上限 512 MB，避免误读超大文件把内存打满。
fn scan_scene_strings(root: &Path, set: &mut BTreeSet<String>) {
    let Some(data_dir) = find_data_dir(root) else {
        return;
    };
    let mut targets: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&data_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            let is_scene = name.starts_with("level") || name.contains("sharedassets") || name.contains("resources.assets") || name.contains("globalgamemanagers");
            if is_scene && e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                targets.push(e.path());
            }
        }
    }
    for p in targets {
        let Ok(meta) = fs::metadata(&p) else { continue };
        if meta.len() > 512 * 1024 * 1024 {
            continue;
        }
        if let Ok(blob) = fs::read(&p) {
            extract_ascii_runs(&blob, set);
        }
    }
}

/// 从候选里挑出"和已知真实键同族"的那些：共享 >= 2 字符的公共前缀。
/// 无已知键时原样返回。
pub fn prefer_same_family(candidates: &[String], known_raw_keys: &[String]) -> (Vec<String>, Option<String>) {
    if known_raw_keys.is_empty() {
        return (candidates.to_vec(), None);
    }
    let prefix = longest_common_prefix(known_raw_keys);
    // 前缀太短（<=1）没有区分度，不做过滤
    if prefix.chars().count() < 2 {
        return (candidates.to_vec(), None);
    }
    let filtered: Vec<String> = candidates
        .iter()
        .filter(|c| c.starts_with(&prefix))
        .cloned()
        .collect();
    if filtered.is_empty() {
        (candidates.to_vec(), None)
    } else {
        (filtered, Some(prefix))
    }
}

fn longest_common_prefix(items: &[String]) -> String {
    let mut it = items.iter();
    let first = match it.next() {
        Some(f) => f.as_str(),
        None => return String::new(),
    };
    let mut end = first.len();
    for s in it {
        let common = first
            .char_indices()
            .zip(s.chars())
            .take_while(|((_, a), b)| a == b)
            .count();
        // 转回字节边界
        end = end.min(first.char_indices().nth(common).map(|(i, _)| i).unwrap_or(first.len()));
    }
    first[..end].to_string()
}

// ---------------------------------------------------------------------------
// 候选排序
// ---------------------------------------------------------------------------

/// 候选相关性评分（越小越可能真的是画廊键）。
///
/// IL2CPP 字面量池里绝大多数是引擎/框架字符串，若只按字典序排序再截断，
/// 真候选会被挤到截断线之外。所以先按相关性分层，再按字典序。
fn relevance(c: &str) -> u8 {
    if dotnet::is_gallery_like(c) {
        return 0; // 形如 cg_flag / CG01 / GalleryXxx
    }
    let low = c.to_ascii_lowercase();
    if low.contains("cg") || low.contains("gallery") || low.contains("album") {
        return 1; // 含关键词（可能是 cgFlag / myGalleryKey 之类）
    }
    if c.contains('_') {
        return 2; // 有下划线：PlayerPrefs 键的常见形态
    }
    3
}

/// 按相关性 + 字典序排序（确定性）。
pub fn sort_candidates(candidates: &mut [String]) {
    candidates.sort_by(|a, b| relevance(a).cmp(&relevance(b)).then_with(|| a.cmp(b)));
}

/// 相关性为 0/1 的候选个数（报告里提示"高置信候选有几条"）。
pub fn high_confidence_count(candidates: &[String]) -> usize {
    candidates.iter().filter(|c| relevance(c) <= 1).count()
}

// ---------------------------------------------------------------------------
// 注册表访问（Windows）
// ---------------------------------------------------------------------------

/// 快照里的一个注册表值。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValueSnap {
    pub name: String,
    /// `dword` / `string` / `binary` / `other:<n>`
    pub kind: String,
    /// dword → 十进制；string → 文本；binary/other → 十六进制
    pub data: String,
}

/// 写操作前的完整备份（可回滚）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RegSnapshot {
    pub company: String,
    pub product: String,
    pub key_path: String,
    pub taken_at: String,
    pub values: Vec<ValueSnap>,
}

#[cfg(windows)]
mod winreg {
    //! 最小可用的注册表封装（RAII 关闭句柄）。刻意不引第三方 crate，
    //! 复用已在依赖树里的 windows-sys。

    use super::{RegSnapshot, ValueSnap};
    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, WIN32_ERROR};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_BINARY, REG_DWORD,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 打开（或创建）PlayerPrefs 键 `HKCU\Software\<company>\<product>`。
    pub struct PrefsKey {
        h: HKEY,
        pub path: String,
    }

    impl Drop for PrefsKey {
        fn drop(&mut self) {
            if !self.h.is_null() {
                unsafe { RegCloseKey(self.h) };
            }
        }
    }

    fn sub_key(company: &str, product: &str) -> String {
        format!("Software\\{company}\\{product}")
    }

    impl PrefsKey {
        /// 只读打开；不存在返回 Err。
        pub fn open(company: &str, product: &str) -> Result<Self, String> {
            let path = sub_key(company, product);
            let sub = wide(&path);
            let mut h: HKEY = std::ptr::null_mut();
            let rc = unsafe {
                RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_READ | KEY_WRITE, &mut h)
            };
            if rc != ERROR_SUCCESS {
                return Err(format!(
                    "打不开注册表键 HKCU\\{path}（错误码 {rc}）。请先运行一次游戏生成 PlayerPrefs。"
                ));
            }
            Ok(PrefsKey { h, path })
        }

        /// 打开或创建（用于键尚不存在的场景）。
        pub fn open_or_create(company: &str, product: &str) -> Result<Self, String> {
            let path = sub_key(company, product);
            let sub = wide(&path);
            let mut h: HKEY = std::ptr::null_mut();
            let mut disp: u32 = 0;
            let rc = unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    sub.as_ptr(),
                    0,
                    std::ptr::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_READ | KEY_WRITE,
                    std::ptr::null(),
                    &mut h,
                    &mut disp,
                )
            };
            if rc != ERROR_SUCCESS {
                return Err(format!("无法创建注册表键 HKCU\\{path}（错误码 {rc}）"));
            }
            Ok(PrefsKey { h, path })
        }

        /// 枚举全部值：`(名称, 类型, 原始字节)`。
        ///
        /// 注意：**不要**用「lpData=NULL 探长度」那种两趟写法——`RegEnumValueW`
        /// 对 `lpData=NULL` 的行为在文档里是含糊的，实测会直接失败导致枚举为空。
        /// 这里直接给一块足够大的真实缓冲区，一趟取回。
        pub fn enum_values(&self) -> Vec<(String, u32, Vec<u8>)> {
            let mut out = Vec::new();
            let mut name_buf = vec![0u16; 16_384];
            let mut data_buf = vec![0u8; 1 << 20]; // 1 MiB 上限（PlayerPrefs 值远小于此）
            let mut idx: u32 = 0;
            loop {
                let mut name_len = name_buf.len() as u32;
                let mut ty: u32 = 0;
                let mut data_len = data_buf.len() as u32;
                let rc = unsafe {
                    RegEnumValueW(
                        self.h,
                        idx,
                        name_buf.as_mut_ptr(),
                        &mut name_len,
                        std::ptr::null(),
                        &mut ty,
                        data_buf.as_mut_ptr(),
                        &mut data_len,
                    )
                };
                if rc == ERROR_NO_MORE_ITEMS {
                    break;
                }
                if rc != ERROR_SUCCESS {
                    crate::diag::log(
                        "WARN",
                        &format!("gallery: RegEnumValueW(idx={idx}) 返回错误码 {rc}，跳过"),
                    );
                    idx += 1;
                    continue;
                }
                let name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
                out.push((name, ty, data_buf[..data_len as usize].to_vec()));
                idx += 1;
            }
            out
        }

        /// 读单个值（名称 → (类型, 字节)）。
        pub fn get(&self, name: &str) -> Option<(u32, Vec<u8>)> {
            let w = wide(name);
            let mut ty: u32 = 0;
            let mut len: u32 = 0;
            let rc = unsafe {
                RegQueryValueExW(
                    self.h,
                    w.as_ptr(),
                    std::ptr::null(),
                    &mut ty,
                    std::ptr::null_mut(),
                    &mut len,
                )
            };
            if rc != ERROR_SUCCESS {
                return None;
            }
            let mut data = vec![0u8; len as usize];
            let rc2 = unsafe {
                RegQueryValueExW(
                    self.h,
                    w.as_ptr(),
                    std::ptr::null(),
                    &mut ty,
                    data.as_mut_ptr(),
                    &mut len,
                )
            };
            if rc2 != ERROR_SUCCESS {
                return None;
            }
            data.truncate(len as usize);
            Some((ty, data))
        }

        /// 写一个 REG_DWORD。
        pub fn set_dword(&self, name: &str, v: u32) -> Result<(), String> {
            let w = wide(name);
            let bytes = v.to_le_bytes();
            let rc: WIN32_ERROR = unsafe {
                RegSetValueExW(
                    self.h,
                    w.as_ptr(),
                    0,
                    REG_DWORD,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                )
            };
            if rc == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(format!("写入 {name} 失败（错误码 {rc}）"))
            }
        }

        /// 按快照类型回写一个值（用于还原）。
        pub fn set_from_snap(&self, snap: &ValueSnap) -> Result<(), String> {
            let w = wide(&snap.name);
            let (ty, bytes): (u32, Vec<u8>) = if let Some(v) = snap.kind.strip_prefix("other:") {
                (v.parse().unwrap_or(REG_BINARY), hex_decode(&snap.data))
            } else {
                match snap.kind.as_str() {
                    "dword" => (REG_DWORD, snap.data.parse::<u32>().unwrap_or(0).to_le_bytes().to_vec()),
                    "string" => (REG_SZ, {
                        let mut b: Vec<u8> = snap.data.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
                        b.extend_from_slice(&[0, 0]);
                        b
                    }),
                    _ => (REG_BINARY, hex_decode(&snap.data)),
                }
            };
            let rc: WIN32_ERROR = unsafe {
                RegSetValueExW(
                    self.h,
                    w.as_ptr(),
                    0,
                    ty,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                )
            };
            if rc == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(format!("还原 {} 失败（错误码 {rc}）", snap.name))
            }
        }

        /// 当前键下全部值的快照（写前备份用）。
        pub fn snapshot(&self, company: &str, product: &str, taken_at: &str) -> RegSnapshot {
            let values = self
                .enum_values()
                .into_iter()
                .map(|(name, ty, data)| ValueSnap {
                    name,
                    kind: match ty {
                        1 => "string".into(),
                        3 => "binary".into(),
                        4 => "dword".into(),
                        other => format!("other:{other}"),
                    },
                    data: if ty == REG_DWORD {
                        let v = data
                            .get(..4)
                            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                            .unwrap_or(0);
                        v.to_string()
                    } else if ty == REG_SZ {
                        let mut units: Vec<u16> = Vec::with_capacity(data.len() / 2);
                        for c in data.chunks(2) {
                            if c.len() < 2 {
                                break;
                            }
                            let u = u16::from_le_bytes([c[0], c[1]]);
                            if u == 0 {
                                break;
                            }
                            units.push(u);
                        }
                        String::from_utf16_lossy(&units)
                    } else {
                        hex_encode(&data)
                    },
                })
                .collect();
            RegSnapshot {
                company: company.to_string(),
                product: product.to_string(),
                key_path: format!("HKCU\\{}", self.path),
                taken_at: taken_at.to_string(),
                values,
            }
        }
    }

    fn hex_encode(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }    fn hex_decode(s: &str) -> Vec<u8> {
        let cs: Vec<char> = s.chars().collect();
        cs.chunks(2)
            .filter_map(|p| {
                if p.len() == 2 {
                    u8::from_str_radix(&format!("{}{}", p[0], p[1]), 16).ok()
                } else {
                    None
                }
            })
            .collect()
    }

    /// **仅供测试**：递归删除 `HKCU\Software\<company>\<product>` 及其父键 `<company>`，
    /// 让注册表回归测试能自建自删、不残留空壳键。
    #[cfg(test)]
    pub fn delete_tree(company: &str, product: &str) -> Result<(), String> {
        use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
        use windows_sys::Win32::System::Registry::RegDeleteTreeW;
        let mut h: HKEY = std::ptr::null_mut();
        let rc = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide("Software").as_ptr(),
                0,
                KEY_READ | KEY_WRITE,
                &mut h,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(format!("打开 Software 键失败（错误码 {rc}）"));
        }
        // 先删 product 子键，再删 company 父键（否则父键非空删不掉，会残留空壳）
        let mut last_err = None;
        for name in [format!("{company}\\{product}"), company.to_string()] {
            let rc2 = unsafe { RegDeleteTreeW(h, wide(&name).as_ptr()) };
            if rc2 != ERROR_SUCCESS && rc2 != ERROR_FILE_NOT_FOUND {
                last_err = Some(format!("删除 {name} 失败（错误码 {rc2}）"));
            }
        }
        unsafe { RegCloseKey(h) };
        match last_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[cfg(not(windows))]
mod winreg {
    use super::{RegSnapshot, ValueSnap};
    pub struct PrefsKey;
    impl PrefsKey {
        pub fn open(_c: &str, _p: &str) -> Result<Self, String> {
            Err("注册表功能仅支持 Windows".into())
        }
        pub fn open_or_create(_c: &str, _p: &str) -> Result<Self, String> {
            Err("注册表功能仅支持 Windows".into())
        }
        pub fn enum_values(&self) -> Vec<(String, u32, Vec<u8>)> {
            Vec::new()
        }
        pub fn get(&self, _n: &str) -> Option<(u32, Vec<u8>)> {
            None
        }
        pub fn set_dword(&self, _n: &str, _v: u32) -> Result<(), String> {
            Err("注册表功能仅支持 Windows".into())
        }
        pub fn set_from_snap(&self, _s: &ValueSnap) -> Result<(), String> {
            Err("注册表功能仅支持 Windows".into())
        }
        pub fn snapshot(&self, _c: &str, _p: &str, _t: &str) -> RegSnapshot {
            RegSnapshot {
                company: String::new(),
                product: String::new(),
                key_path: String::new(),
                taken_at: String::new(),
                values: Vec::new(),
            }
        }
    }

    #[cfg(test)]
    pub fn delete_tree(_c: &str, _p: &str) -> Result<(), String> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

fn now_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Unity 的 `Op::Unlock` 入口。
///
/// 选项（`--opt:key=value`）：
/// - `apply=1`：真正写入注册表；缺省为**只读扫描**（安全默认）
/// - `filter=<子串>`：只处理键名包含该子串的候选
/// - `max=<n>`：候选上限（默认 3000）
/// - `restore=<快照.json>`：从备份还原，忽略其它写入逻辑
pub fn unlock(ctx: &Ctx) -> OpOutcome {
    // 还原分支
    if let Some(path) = ctx.opt("restore") {
        return restore_from(ctx, Path::new(path));
    }

    let Some(info) = read_app_info(ctx.root) else {
        return OpOutcome::fail(
            "未找到 <游戏>_Data/app.info（无法确定 PlayerPrefs 的注册表路径）。\
             请确认目录选到含 UnityPlayer.dll 的那一层。",
        );
    };
    ctx.report(0.1, "已读取 app.info");

    // 打开注册表键：优先只读打开（说明游戏跑过），失败再尝试创建
    let key = match winreg::PrefsKey::open(&info.company, &info.product) {
        Ok(k) => k,
        Err(e) => {
            let created = winreg::PrefsKey::open_or_create(&info.company, &info.product);
            match created {
                Ok(k) => {
                    ctx.report(0.2, "注册表键不存在，已创建（建议先运行一次游戏再解锁）");
                    k
                }
                Err(_) => return OpOutcome::fail(e),
            }
        }
    };
    let location = format!("HKCU\\Software\\{}\\{}", info.company, info.product);
    ctx.report(0.25, "已打开 PlayerPrefs 注册表键");

    // 收集已存在的真实键，用于哈希自检
    let existing = key.enum_values();
    let mut known_raw: Vec<String> = Vec::new();
    let mut hash_ok = 0usize;
    let mut hash_bad: Vec<String> = Vec::new();
    for (name, _, _) in &existing {
        if let Some((raw, embedded)) = split_value_name(name) {
            known_raw.push(raw.to_string());
            if prefs_hash(raw) == embedded {
                hash_ok += 1;
            } else {
                hash_bad.push(name.clone());
            }
        }
    }
    let note;
    if hash_ok > 0 || !hash_bad.is_empty() {
        if hash_bad.is_empty() {
            note = format!("哈希自检通过（{hash_ok} 个现存键反推一致）");
        } else {
            note = format!(
                "⚠ 哈希自检异常：{hash_ok} 个一致 / {} 个不一致（如 {}）——请勿写入，先反馈样本",
                hash_bad.len(),
                hash_bad.first().cloned().unwrap_or_default()
            );
        }
    } else {
        note = "注册表下暂无 PlayerPrefs 键（游戏可能还没跑过），无法用现存键校验哈希".into();
    }
    ctx.report(0.4, "已完成哈希自检");

    // 候选键名（精确来源 + 场景启发式）
    let (all_candidates, scan) = collect_candidate_keys_detailed(ctx.root);
    let asm_note = precise_note(&scan);
    let (family_filtered, family_prefix) = prefer_same_family(&all_candidates, &known_raw);
    let mut candidates = family_filtered;
    if let Some(f) = ctx.opt("filter") {
        candidates.retain(|c| c.contains(f));
    }
    let max: usize = ctx.opt("max").and_then(|s| s.parse().ok()).unwrap_or(3000);
    // 先去重，再按相关性排序，**最后才截断** —— 否则 IL2CPP 池里的框架串
    // 会把真正的 CG 键挤出截断线。
    candidates.sort();
    candidates.dedup();
    sort_candidates(&mut candidates);
    let high_conf = high_confidence_count(&candidates);
    let truncated = candidates.len() > max;
    candidates.truncate(max);
    ctx.report(0.7, "已提取候选键名");

    if !hash_bad.is_empty() && ctx.opt("apply") == Some("1") {
        return OpOutcome::fail(format!(
            "拒绝写入：哈希自检未通过。{note}"
        ));
    }

    // 按注册表现状给候选分类（解锁态 / 锁定态 / 新增 / 非 int / 与裸值重名）
    let plain_names: BTreeSet<&str> = existing.iter().map(|(n, _, _)| n.as_str()).collect();
    let (mut unlocked, mut locked, mut new_keys, mut non_int, mut plain_hit) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    for c in &candidates {
        match key.get(&prefs_value_name(c)) {
            Some((ty, data)) => {
                if ty != REG_DWORD_TYPE {
                    non_int += 1;
                } else if data
                    .get(..4)
                    .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    == Some(1)
                {
                    unlocked += 1;
                } else {
                    locked += 1;
                }
            }
            None => {
                if plain_names.contains(c.as_str()) {
                    plain_hit += 1;
                } else {
                    new_keys += 1;
                }
            }
        }
    }

    let apply = ctx.opt("apply") == Some("1");

    if !apply {
        let mut msg = format!(
            "【只读扫描】Unity 画廊解锁预览\n\
             注册表位置: {location}\n\
             {note}\n\
             {asm_note}\n\
             候选键名: {} 个 —— 已解锁 {unlocked} / 锁定 {locked} / 注册表中尚无 {new_keys}；\
             将跳过：非 int {non_int} / 与裸值重名 {plain_hit}",
            candidates.len()
        );
        if let Some(p) = &family_prefix {
            msg.push_str(&format!("\n已按现存键族前缀 `{p}` 收敛候选"));
        }
        if high_conf > 0 {
            msg.push_str(&format!("\n其中高置信候选（形如 cg_/CG01/含 gallery 关键词）{high_conf} 条，已排在前面"));
        }
        if truncated {
            msg.push_str(&format!("\n（候选过多，已截断到 {max}；可用 --opt:max= 或 --opt:filter= 收窄）"));
        }
        if scan.is_il2cpp() {
            msg.push_str(
                "\n注意：IL2CPP 元数据的字面量池含大量引擎/框架字符串，候选里可能混入与画廊无关的名字；\
                 建议配合 `--opt:filter=` 收窄（例如 --opt:filter=cg）",
            );
        }
        if !candidates.is_empty() {
            let sample: Vec<&String> = candidates.iter().take(20).collect();
            msg.push_str(&format!("\n示例: {sample:?}"));
        }
        msg.push_str(
            "\n\n→ 确认无误后用 `--opt:apply=1` 真正写入（写前自动备份、写后读回校验）。\
             \n→ 更彻底的做法：很多作品自带隐藏的『全开开关』（如 Gallery Open / Debug 菜单），\
             打开后游戏会一次性开放全部条目，比逐个写键更完整——建议先在游戏设置/标题界面找一找。",
        );
        return OpOutcome::okn(msg, candidates.len());
    }

    if candidates.is_empty() {
        return OpOutcome::fail("没有可写入的候选键名（可尝试 --opt:filter= 放宽条件）");
    }

    // 写前备份
    let snap = key.snapshot(&info.company, &info.product, &now_stamp());
    let bak_path = ctx.out_dir.join("unity_playerprefs_backup.json");
    if let Some(parent) = bak_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(e) = fs::write(&bak_path, serde_json::to_string_pretty(&snap).unwrap_or_default()) {
        return OpOutcome::fail(format!(
            "备份注册表失败（为安全起见已中止，未写入任何值）: {e}"
        ));
    }
    ctx.report(0.8, "已备份原注册表值");

    // 写入 + 读回校验
    let mut written = 0usize;
    let mut verified = 0usize;
    let mut failed = 0usize;
    let mut skipped_nonint = 0usize;
    let mut skipped_plain = 0usize;
    let mut flipped = 0usize;
    // 注册表里已存在的**裸值名**（无 `_h` 后缀）。真正的 PlayerPrefs 键一定带后缀，
    // 所以同名的裸值说明它不是 pref（多为噪声），不要为它新建 `<name>_h<hash>`。
    let plain_names: BTreeSet<&str> = existing.iter().map(|(n, _, _)| n.as_str()).collect();
    for c in &candidates {
        let vname = prefs_value_name(c);
        match key.get(&vname) {
            Some((ty, data)) => {
                // 已存在但**不是 int 类型**：说明它不是画廊解锁位，
                // 强行改成 DWORD 会破坏游戏的字符串/字节设置 —— 跳过。
                if ty != REG_DWORD_TYPE {
                    skipped_nonint += 1;
                    crate::diag::log(
                        "WARN",
                        &format!("gallery: 跳过非 DWORD 键 {vname}（注册表类型 {ty}）"),
                    );
                    continue;
                }
                let cur = data
                    .get(..4)
                    .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                if cur == Some(1) {
                    continue; // 已是解锁态，保持原状
                }
                flipped += 1;
            }
            None => {
                if plain_names.contains(c.as_str()) {
                    skipped_plain += 1;
                    continue;
                }
            }
        }
        match key.set_dword(&vname, 1) {
            Ok(()) => {
                written += 1;
                if let Some((ty, data)) = key.get(&vname) {
                    if ty == REG_DWORD_TYPE
                        && data.get(..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) == Some(1)
                    {
                        verified += 1;
                    }
                }
            }
            Err(e) => {
                failed += 1;
                crate::diag::log("WARN", &format!("gallery: {e}"));
            }
        }
        let frac = 0.8
            + 0.2 * (written + failed + skipped_nonint + skipped_plain) as f32
                / candidates.len().max(1) as f32;
        ctx.report(frac, c);
    }

    let mut msg = format!(
        "✔ Unity 画廊解锁完成\n\
         注册表位置: {location}\n\
         {note}\n\
         {asm_note}\n\
         候选 {} 个 → 写入 {written} 个（其中翻转既有 0→1 的 {flipped} 个；读回校验通过 {verified} 个），失败 {failed} 个；\
         跳过：非 int 键 {skipped_nonint} 个 / 与裸值重名 {skipped_plain} 个\n\
         原值已备份: {}",
        candidates.len(),
        bak_path.display()
    );
    if family_prefix.is_none() {
        msg.push_str(if scan.usable() {
            "\n提示：候选来自程序集字符串（精确字面量），已按过滤规则剔除明显噪声；\
             若仍担心混入无关设置项，可用 `--opt:filter=` 收窄后重跑。"
        } else {
            "\n⚠ 未能用「现存键族前缀」收敛候选（注册表里没有可参照的 CG 键），\
             且未读到程序集精确字符串，当前为启发式候选，可能混入与画廊无关的整数设置项。\
             若担心误改，请先用 `--opt:filter=` 收窄后重跑。"
        });
    }
    if verified < written {
        msg.push_str("\n⚠ 有写入项读回校验未通过，请用 `--opt:restore=<备份文件>` 回滚后反馈");
    }
    msg.push_str("\n提示：启动游戏查看画廊是否全开；若仍有未开项，多为未收录的键名，可用 `--opt:filter=` 细化后重跑。");

    let ok = written > 0 && verified == written;
    OpOutcome {
        success: ok,
        message: msg,
        files_done: written,
        logs: vec![],
    }
}

/// 从备份快照还原（覆盖式回写快照里记录的全部值）。
fn restore_from(ctx: &Ctx, path: &Path) -> OpOutcome {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return OpOutcome::fail(format!("读取备份失败 {}: {e}", path.display())),
    };
    let snap: RegSnapshot = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => return OpOutcome::fail(format!("备份文件格式不符: {e}")),
    };
    let key = match winreg::PrefsKey::open(&snap.company, &snap.product) {
        Ok(k) => k,
        Err(e) => return OpOutcome::fail(e),
    };
    let mut done = 0usize;
    let mut failed = 0usize;
    for (i, v) in snap.values.iter().enumerate() {
        match key.set_from_snap(v) {
            Ok(()) => done += 1,
            Err(e) => {
                failed += 1;
                crate::diag::log("WARN", &format!("gallery restore: {e}"));
            }
        }
        ctx.report(
            (i + 1) as f32 / snap.values.len().max(1) as f32,
            &v.name,
        );
    }
    let snap_names: BTreeSet<&str> = snap.values.iter().map(|v| v.name.as_str()).collect();
    // 备份之后新增的值不在快照里，还原不会删除它们——如实列出，避免误以为"完全回滚"
    let extra: Vec<String> = key
        .enum_values()
        .into_iter()
        .map(|(n, _, _)| n)
        .filter(|n| !snap_names.contains(n.as_str()))
        .collect();

    let mut msg = format!(
        "已从备份还原 {done} 个注册表值（失败 {failed}）\n来源: {}",
        path.display()
    );
    if !extra.is_empty() {
        msg.push_str(&format!(
            "\n注意：以下 {} 个值是备份**之后**新增的，还原不会自动删除（如需彻底回滚请手动删除）：{}",
            extra.len(),
            extra.join(", ")
        ));
    }
    OpOutcome {
        success: failed == 0,
        message: msg,
        files_done: done,
        logs: vec![],
    }
}

/// 供 GUI / CLI 复用：把当前已有的原始键名列出（不含哈希后缀）。
pub fn known_keys(root: &Path) -> Vec<String> {
    let Some(info) = read_app_info(root) else {
        return Vec::new();
    };
    let Ok(key) = winreg::PrefsKey::open(&info.company, &info.product) else {
        return Vec::new();
    };
    key.enum_values()
        .into_iter()
        .filter_map(|(n, _, _)| split_value_name(&n).map(|(raw, _)| raw.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_known_vector() {
        // skill 实测样本：`Orc Kabe` → 3173159926
        assert_eq!(prefs_hash("Orc Kabe"), 3_173_159_926);
    }

    #[test]
    fn hash_is_deterministic_and_utf8() {
        // 相同输入稳定；CJK 按 UTF-8 字节参与
        assert_eq!(prefs_hash("CG01"), prefs_hash("CG01"));
        assert_ne!(prefs_hash("CG01"), prefs_hash("CG02"));
        // 不 panic 且为 32 位
        let _ = prefs_hash("回想・シーン");
    }

    #[test]
    fn value_name_roundtrip() {
        let v = prefs_value_name("Orc Kabe");
        assert_eq!(v, "Orc Kabe_h3173159926");
        let (raw, h) = split_value_name(&v).unwrap();
        assert_eq!(raw, "Orc Kabe");
        assert_eq!(h, 3_173_159_926);
        // 非该形态
        assert!(split_value_name("Volume").is_none());
        assert!(split_value_name("_h123").is_none());
        assert!(split_value_name("abc_hxyz").is_none());
    }

    #[test]
    fn looks_like_key_filters_noise() {
        assert!(looks_like_key("Orc Kabe"));
        assert!(looks_like_key("CG_01_a"));
        assert!(looks_like_key("cg.flag-1"));
        assert!(looks_like_key("CG回想1"));
        // 纯非 ASCII（无 ASCII 字母）拒绝——否则 IL2CPP 池里成片的 CJK 台词会灌进来
        assert!(!looks_like_key("回想シーン"));
        // 噪声
        assert!(!looks_like_key("Assets/Textures/a"));
        assert!(!looks_like_key("m_Script"));
        assert!(!looks_like_key("UnityEngine.Object"));
        assert!(!looks_like_key("deadbeefcafebabe"));
        assert!(!looks_like_key("12"));
        assert!(!looks_like_key("123456"));
        // IL2CPP 字面量池里的框架格式串（新增白名单专治这些）
        assert!(!looks_like_key(" (offset:"));
        assert!(!looks_like_key("{0} --> {1}"));
        assert!(!looks_like_key("   -   W:"));
        assert!(!looks_like_key("Data:"));
        assert!(!looks_like_key("a=b"));
        assert!(!looks_like_key(" [1] x"));
        // 首/末字符规则 + 连续空格规则
        assert!(!looks_like_key("-   q"));
        assert!(!looks_like_key("-  A"));
        assert!(!looks_like_key("Pass Culling Disabled -"));
        assert!(!looks_like_key("--- End of inner exception stack trace --"));
        assert!(!looks_like_key("            model"));
        assert!(!looks_like_key("Modifiers:  ok"));
        // protobuf 描述符的 base64 常量块（长 + base64 字符集 + 4 的倍数 + 大小写数字齐全）
        assert!(!looks_like_key("Cg1UWVBFX1NGSVhFRDMyEA8SEQoNVFlQRV9TRklYRUQ2NBAQEg8KC1RZUEVf"));
        assert!(!looks_like_key("Cg1yZXNlcnZlZF9uYW1lGAUgAygJGi8KEUVudW1SZXNlcnZlZFJhbmdlEg0K"));
        // 但真键必须保住：只有小写+数字（无大写）→ 不误杀
        assert!(looks_like_key("cg_button_name1"));
        assert!(looks_like_key("CG8KK0sidd"));
    }

    #[test]
    fn relevance_puts_gallery_like_first() {
        let mut v: Vec<String> = vec![
            "zzz_framework_helper".into(),
            "Screenmanager Resolution Width".into(),
            "myGalleryKey".into(),
            "cg_flag_01".into(),
            "CG01".into(),
        ];
        sort_candidates(&mut v);
        // 形如 cg_/CG01 的排最前
        assert_eq!(v[0], "CG01");
        assert_eq!(v[1], "cg_flag_01");
        assert_eq!(v[2], "myGalleryKey");
        assert_eq!(high_confidence_count(&v), 3);
        // 排序必须确定性（同分按字典序）
        let mut v2 = v.clone();
        v2.reverse();
        sort_candidates(&mut v2);
        assert_eq!(v, v2);
    }

    #[test]
    fn extracts_ascii_runs() {
        let mut blob = b"\x00\x00".to_vec();
        blob.extend_from_slice(b"Orc Kabe");
        blob.push(0);
        blob.extend_from_slice(b"\xff\xfe");
        blob.extend_from_slice(b"CG_Scene_02");
        blob.push(0);
        blob.extend_from_slice(b"Assets/x"); // 含斜杠，应被过滤
        let mut set = BTreeSet::new();
        extract_ascii_runs(&blob, &mut set);
        let v: Vec<String> = set.into_iter().collect();
        assert!(v.contains(&"Orc Kabe".to_string()));
        assert!(v.contains(&"CG_Scene_02".to_string()));
        assert!(!v.iter().any(|s| s.contains("Assets/")));
    }

    #[test]
    fn common_prefix_and_family() {
        let known = vec!["Orc Kabe".to_string(), "Orc Kabe2".to_string()];
        assert_eq!(longest_common_prefix(&known), "Orc Kabe");
        let cands = vec![
            "Orc Kabe3".to_string(),
            "TOTALLY UNRELATED".to_string(),
            "Orc Kabe4".to_string(),
        ];
        let (f, p) = prefer_same_family(&cands, &known);
        assert_eq!(p.as_deref(), Some("Orc Kabe"));
        assert_eq!(f.len(), 2);
        // 无已知键 → 原样
        let (f2, p2) = prefer_same_family(&cands, &[]);
        assert!(p2.is_none());
        assert_eq!(f2.len(), 3);
    }

    #[test]
    fn app_info_parsing() {
        let dir = std::env::temp_dir().join(format!("stool_gal_{}", std::process::id()));
        let data = dir.join("MyGame_Data");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("app.info"), "FooSoft\nOrcGame\n").unwrap();
        let info = read_app_info(&dir).unwrap();
        assert_eq!(info.company, "FooSoft");
        assert_eq!(info.product, "OrcGame");
        assert_eq!(find_data_dir(&dir).unwrap(), data);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn candidate_scan_finds_scene_strings() {
        let dir = std::env::temp_dir().join(format!("stool_gal_scan_{}", std::process::id()));
        let data = dir.join("G_Data");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&data).unwrap();
        let mut blob = Vec::new();
        for s in ["Orc Kabe", "Slime Girl", "Assets/Noise"] {
            blob.extend_from_slice(s.as_bytes());
            blob.push(0);
        }
        fs::write(data.join("level0"), &blob).unwrap();
        let keys = collect_candidate_keys(&dir);
        assert!(keys.contains(&"Orc Kabe".to_string()));
        assert!(keys.contains(&"Slime Girl".to_string()));
        assert!(!keys.iter().any(|k| k.contains("Assets/")));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 精确来源接入（Mono）：`<_Data>/Managed/Assembly-CSharp.dll` 里的字面量
    /// 必须进入候选集，且画廊类型要被识别出来。
    #[test]
    fn dotnet_scan_feeds_candidates() {
        let dir = std::env::temp_dir().join(format!("stool_gal_asm_{}", std::process::id()));
        let data = dir.join("G_Data");
        let managed = data.join("Managed");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&managed).unwrap();

        // 只放程序集、不放任何场景文件 —— 候选应当**只**来自程序集
        let dll = crate::formats::dotnet::tests_support::minimal_pe(
            &["GalleryManager", "_wholeNameList"],
            &["cg_flag_01", "cg_flag_02"],
        );
        fs::write(managed.join("Assembly-CSharp.dll"), &dll).unwrap();

        let found = find_assemblies(&dir);
        assert_eq!(found.len(), 1, "应找到 1 个程序集: {found:?}");
        assert!(found[0].ends_with("Assembly-CSharp.dll"));

        let (keys, scan) = collect_candidate_keys_detailed(&dir);
        assert!(scan.usable(), "程序集应解析成功: {scan:?}");
        assert_eq!(scan.sources[0].1, PreciseKind::Dotnet);
        assert!(keys.contains(&"cg_flag_01".to_string()), "{keys:?}");
        assert!(keys.contains(&"cg_flag_02".to_string()), "{keys:?}");
        assert!(scan.gallery_types.iter().any(|t| t == "GalleryManager"), "{scan:?}");
        assert!(scan.gallery_types.iter().any(|t| t == "_wholeNameList"), "{scan:?}");

        // 程序集坏掉时不致命：记 failures，且退回场景启发式（此处无场景 → 空候选）
        fs::write(managed.join("Assembly-CSharp.dll"), b"MZ\x00\x00 garbage").unwrap();
        let scan2 = scan_precise(&dir);
        assert!(!scan2.usable());
        assert_eq!(scan2.failures.len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    /// 精确来源接入（IL2CPP）：没有 Managed 目录时，改读
    /// `il2cpp_data/Metadata/global-metadata.dat`。
    #[test]
    fn il2cpp_scan_feeds_candidates() {
        let dir = std::env::temp_dir().join(format!("stool_gal_il2_{}", std::process::id()));
        let meta = dir.join("G_Data").join("il2cpp_data").join("Metadata");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&meta).unwrap();

        let md = crate::formats::il2cpp::tests_support::build_metadata(
            29,
            &["cg_flag_01", "{0} --> {1}", "cg_flag_02"],
            &["mscorlib", "GalleryManager"],
        );
        fs::write(meta.join("global-metadata.dat"), &md).unwrap();
        assert!(find_il2cpp_metadata(&dir).is_some());

        let (keys, scan) = collect_candidate_keys_detailed(&dir);
        assert!(scan.usable(), "IL2CPP 元数据应解析成功: {scan:?}");
        assert_eq!(scan.sources[0].1, PreciseKind::Il2Cpp);
        assert!(scan.is_il2cpp());
        assert!(keys.contains(&"cg_flag_01".to_string()), "{keys:?}");
        assert!(keys.contains(&"cg_flag_02".to_string()), "{keys:?}");
        // 框架格式串必须被挡掉（白名单字符集）
        assert!(!keys.iter().any(|k| k.contains("-->")), "{keys:?}");
        assert!(scan.gallery_types.iter().any(|t| t == "GalleryManager"), "{scan:?}");
        assert!(precise_note(&scan).contains("IL2CPP"));

        let _ = fs::remove_dir_all(&dir);
    }

    /// 两种精确来源都缺席 → 不报错，只退回启发式。
    #[test]
    fn missing_precise_sources_is_not_an_error() {
        let dir = std::env::temp_dir().join(format!("stool_gal_none_{}", std::process::id()));
        let data = dir.join("G_Data");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("level0"), b"Orc Kabe\0").unwrap();
        let scan = scan_precise(&dir);
        assert!(!scan.usable());
        assert!(scan.failures.is_empty());
        assert!(precise_note(&scan).contains("已退回场景启发式"));
        assert!(collect_candidate_keys(&dir).contains(&"Orc Kabe".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 真实注册表端到端回归：建键 → 写 → 枚举 → 哈希自检 → 快照 → 还原 → 删键。
    ///
    /// **默认 `#[ignore]`**（会写真实注册表，虽然自建自删）。显式运行：
    /// `cargo test --lib gallery::tests::registry_roundtrip -- --ignored --nocapture`
    #[test]
    #[ignore = "会写入真实注册表（自建自删），需显式 --ignored 运行"]
    fn registry_roundtrip() {
        let (company, product) = ("StoolGalleryTest", "Unlock");
        let _ = winreg::delete_tree(company, product); // 清掉上次可能的残留

        // 1. 建键 + 写入
        {
            let key = winreg::PrefsKey::open_or_create(company, product).expect("创建测试键失败");
            key.set_dword(&prefs_value_name("Orc Kabe"), 1).unwrap();
            key.set_dword(&prefs_value_name("Slime Girl"), 0).unwrap();
        }

        // 2. 读回 + 枚举 + 哈希自检（本次修复的 FFI 路径）
        let snapshot = {
            let key = winreg::PrefsKey::open(company, product).expect("打开测试键失败");
            let vals = key.enum_values();
            assert_eq!(vals.len(), 2, "枚举应返回 2 个值，实际 {}", vals.len());
            let names: Vec<String> = vals.iter().map(|(n, _, _)| n.clone()).collect();
            assert!(names.contains(&"Orc Kabe_h3173159926".to_string()));
            assert!(names.contains(&"Slime Girl_h3352420299".to_string()));
            // 哈希自检：每个现存键反推都必须一致
            for (n, ty, data) in &vals {
                assert_eq!(*ty, REG_DWORD_TYPE, "{n} 应为 DWORD");
                if let Some((raw, h)) = split_value_name(n) {
                    assert_eq!(prefs_hash(raw), h, "哈希自检失败: {n}");
                }
                assert!(!data.is_empty());
            }
            // 单值读取
            let (ty, d) = key.get(&prefs_value_name("Slime Girl")).unwrap();
            assert_eq!(ty, REG_DWORD_TYPE);
            assert_eq!(u32::from_le_bytes([d[0], d[1], d[2], d[3]]), 0);
            key.snapshot(company, product, "test")
        };

        // 3. 改一个再还原
        {
            let key = winreg::PrefsKey::open(company, product).unwrap();
            key.set_dword(&prefs_value_name("Slime Girl"), 1).unwrap();
            for v in &snapshot.values {
                key.set_from_snap(v).unwrap();
            }
            let (_, d) = key.get(&prefs_value_name("Slime Girl")).unwrap();
            assert_eq!(u32::from_le_bytes([d[0], d[1], d[2], d[3]]), 0, "还原后应为 0");
        }

        // 4. 清理
        winreg::delete_tree(company, product).expect("删除测试键失败");
        assert!(winreg::PrefsKey::open(company, product).is_err(), "测试键应已删除");
    }
}
