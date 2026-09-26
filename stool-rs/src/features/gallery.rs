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

/// 注册表 `REG_BINARY` 的类型值（Windows 的 `REG_BINARY` 常量 = 3）。
///
/// 别以为「二进制」就不能写：Unity 系作品常把**字符串**存成 `REG_BINARY` +
/// ASCII + 结尾 NUL（实测样本：某作的 `GameState_Money` = 字节 `36 31 30 38 00`
/// 即 `"6108\0"` —— 用字符串存钱正是为了绕过 32 位上限），所以这一档要按
/// 「文本字节」对待，并跟着原值的结尾 NUL 走。
const REG_BINARY_TYPE: u32 = 3;

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
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegEnumValueW, RegOpenKeyExW,
        RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_BINARY,
        REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
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

        /// 按指定类型写一个值（`--opt:set=` 用；不猜类型）。
        pub fn set_raw(&self, name: &str, ty: u32, bytes: &[u8]) -> Result<(), String> {
            let w = wide(name);
            let rc: WIN32_ERROR = unsafe {
                RegSetValueExW(self.h, w.as_ptr(), 0, ty, bytes.as_ptr(), bytes.len() as u32)
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

        /// 删除一个值（撤销本工具的写入用）。
        ///
        /// 值不存在（`ERROR_FILE_NOT_FOUND`）也算成功——删除是幂等的，
        /// 重跑一次不该报错。
        pub fn delete_value(&self, name: &str) -> Result<(), String> {
            use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
            let w = wide(name);
            let rc: WIN32_ERROR = unsafe { RegDeleteValueW(self.h, w.as_ptr()) };
            if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!("删除 {name} 失败（错误码 {rc}）"))
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
        pub fn set_raw(&self, _n: &str, _t: u32, _b: &[u8]) -> Result<(), String> {
            Err("注册表功能仅支持 Windows".into())
        }
        pub fn set_from_snap(&self, _s: &ValueSnap) -> Result<(), String> {
            Err("注册表功能仅支持 Windows".into())
        }
        pub fn delete_value(&self, _n: &str) -> Result<(), String> {
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

/// 是否拒绝「纯启发式候选」的批量写入。
///
/// 只在**四个条件同时成立**时才拒绝：真的要写入、精确来源解析不出来
/// （`scan_usable == false`）、也没能用注册表里的现存键族收敛候选
/// （`family_prefix.is_none()`）、且调用方没有显式强制。这几条凑齐说明整批候选
/// 都是从场景/资源二进制里「捞」出来的字符串，写进去几乎不可能命中游戏真正会读的
/// 键 —— 这正是某作被灌进 3000 条垃圾键的场景（见 `unlock` 里的闸门说明）。
/// 调用方显式给 `--opt:force_heuristic=1` 时一律放行（知情选择）。
fn heuristic_write_blocked(
    apply: bool,
    scan_usable: bool,
    family_prefix: Option<&str>,
    forced: bool,
) -> bool {
    apply && !scan_usable && family_prefix.is_none() && !forced
}

/// 把 `text` 编成 `REG_SZ` 的字节（UTF-16LE + 结尾 NUL）。
fn utf16z(text: &str) -> Vec<u8> {
    let mut b: Vec<u8> = text.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    b.extend_from_slice(&[0, 0]);
    b
}

/// 注册表类型 → 给人看的名字。
///
/// 常见类型都给个名字：`--opt:set=` 拒绝改写时要把「它是啥」讲清楚，而不是只丢个数字，
/// 不然用户没法判断该不该手工去改。
fn kind_label(ty: u32) -> &'static str {
    match ty {
        REG_DWORD_TYPE => "DWORD",
        1 => "字符串",
        2 => "可扩展字符串",
        REG_BINARY_TYPE => "二进制",
        5 => "大端 DWORD",
        6 => "符号链接",
        7 => "多字符串",
        8 => "资源列表",
        9 => "完整资源描述",
        10 => "资源需求列表",
        11 => "QWORD",
        _ => "未知类型",
    }
}

/// 去掉尾部的 0 字节（`REG_SZ` 的结尾 NUL 不参与读回比较）。
fn trim_nul(b: &[u8]) -> &[u8] {
    let mut n = b.len();
    while n > 0 && b[n - 1] == 0 {
        n -= 1;
    }
    &b[..n]
}

/// 把一个注册表值渲染成短文本（报告用）。
fn display_val(ty: u32, data: &[u8]) -> String {
    if ty == REG_DWORD_TYPE {
        return data
            .get(..4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]).to_string())
            .unwrap_or_else(|| "<不足 4 字节>".into());
    }
    if ty == 1 {
        // `REG_SZ` 按 UTF-16LE 解到第一个 NUL 为止。
        let mut units: Vec<u16> = Vec::new();
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
        return format!("{:?}", String::from_utf16_lossy(&units));
    }
    if ty == REG_BINARY_TYPE {
        // 文本型二进制就显示文本（别把真二进制乱解成字符串）。
        let body = trim_nul(data);
        if !body.is_empty() && body.iter().all(|b| (0x20..0x7f).contains(b)) {
            return format!("{:?}", String::from_utf8_lossy(body));
        }
    }
    format!("<{} 字节>", data.len())
}

/// 解析 `--opt:set=` 的 `名称=值,名称=值` 串。
///
/// 名称允许空格（如 `Screenmanager Window Position X`），值允许 `0x` 十六进制与
/// 负数。空项跳过；缺 `=` 或键名为空直接报错 —— **宁可不写也不猜**。
fn parse_set_pairs(spec: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for item in spec.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let Some((k, v)) = item.split_once('=') else {
            return Err(format!(
                "`--opt:set=` 里的 `{item}` 缺少 `=`（格式: 名称=值,名称=值）"
            ));
        };
        let (k, v) = (k.trim(), v.trim());
        if k.is_empty() {
            return Err(format!("`--opt:set=` 里的 `{item}` 键名为空"));
        }
        out.push((k.to_string(), v.to_string()));
    }
    if out.is_empty() {
        return Err("`--opt:set=` 没解析出任何键值对（格式: 名称=值,名称=值）".into());
    }
    Ok(out)
}

/// 把用户给的文本按目标类型转成注册表字节。
///
/// **已有值的类型优先**：DWORD 就按整数写、字符串就按字符串写、文本型二进制就按
/// 原样字节写 —— 不擅自改类型。把游戏的字符串设置改成 DWORD 会破坏它，这与画廊写入
/// 是同一条戒律。`existing_nul` 表示原值以 NUL 结尾（`REG_BINARY` 的文本约定），
/// 跟带才能和游戏自己的写法一致。
/// 键还不存在时：能解析成整数就当 DWORD，否则当字符串。
fn coerce_value(
    text: &str,
    existing_ty: Option<u32>,
    existing_nul: bool,
) -> Result<(u32, Vec<u8>), String> {
    let as_dword = || -> Result<u32, String> {
        let t = text.trim();
        let (neg, body) = match t.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, t),
        };
        let n = if let Some(h) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
            u32::from_str_radix(h, 16).map_err(|e| format!("`{text}` 不是合法的十六进制整数: {e}"))?
        } else {
            body.parse::<u32>()
                .map_err(|e| format!("`{text}` 不是合法的整数: {e}"))?
        };
        Ok(if neg { (n as i32).wrapping_neg() as u32 } else { n })
    };
    match existing_ty {
        Some(t) if t == REG_DWORD_TYPE => Ok((REG_DWORD_TYPE, as_dword()?.to_le_bytes().to_vec())),
        Some(t) if t == REG_BINARY_TYPE => {
            let mut b = text.as_bytes().to_vec();
            if existing_nul {
                b.push(0);
            }
            Ok((REG_BINARY_TYPE, b))
        }
        Some(1) => Ok((1, utf16z(text))),
        Some(t) => Err(format!(
            "该键现有类型是 {}（{}）—— 本工具只认 DWORD / 字符串 / 文本型二进制三种，\
             为一个不熟悉的类型造字节等于瞎猜。为免破坏它，`--opt:set=` 拒绝改写，\
             这一条只能手工改注册表。",
            kind_label(t),
            t
        )),
        None => Ok(match as_dword() {
            Ok(v) => (REG_DWORD_TYPE, v.to_le_bytes().to_vec()),
            Err(_) => (1, utf16z(text)),
        }),
    }
}

/// Unity 的 `Op::Unlock` 入口。
///
/// 选项（`--opt:key=value`）：
/// - `apply=1`：真正写入注册表；缺省为**只读扫描**（安全默认）
/// - `filter=<子串>`：只处理键名包含该子串的候选
/// - `max=<n>`：候选上限（默认 3000）
/// - `restore=<快照.json>`：从备份还原，忽略其它写入逻辑
/// - `undo=1`：反向操作——把候选键对应的注册表值**删掉**（撤销本工具写过的痕迹）。
///   同样默认只预览，要真删须再给 `apply=1`；`keep=<前缀,前缀>` 保护游戏真实键
/// - `force_heuristic=1`：候选只有「场景/资源二进制启发式」这一个来源时，
///   允许写入（默认拒绝，见函数体里的闸门说明）
/// - `set=名称=值,名称=值`：**不做候选提取**，只按名字写你点名的键（类型按现存值
///   决定）。适合「这款游戏没有画廊键、内容按进度解锁」的情形 —— 改真实进度键
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

    // 撤销分支：删掉本工具此前可能写进去的候选键（只删候选，绝不动其它值）
    if ctx.opt("undo") == Some("1") {
        return undo_from(ctx, &key, &info, &candidates, &existing);
    }

    if !hash_bad.is_empty() && ctx.opt("apply") == Some("1") {
        return OpOutcome::fail(format!(
            "拒绝写入：哈希自检未通过。{note}"
        ));
    }

    // 按名写指定值分支（`--opt:set=名称=值,…`）。
    //
    // 与画廊解锁的本质区别：**完全不做候选提取**，只写调用方点名的键 —— 也就是说
    // 它「不猜」。像 SheepClicker 这类没有画廊键、内容按进度解锁的作品，唯一能起
    // 作用的入口就是它（改 `GameState_Follower` / `Skill_*Level` 这些真实键）。
    // 必须放在启发式闸门**之前**：闸门管的是「猜出来的候选」，与点名写入无关。
    if let Some(spec) = ctx.opt("set") {
        return set_values(ctx, &key, &info, &existing, spec);
    }

    // 启发式候选的写入闸门。
    //
    // 「精确来源」（程序集字符串 / IL2CPP 元数据字面量）解析不出来、注册表里也没有
    // 可参照的键族时，候选只能从场景/资源的二进制里「捞」字符串。这种候选里混着大量
    // 与画廊无关的东西（shader 关键词、资源名、压缩数据碎片），而且**本工具无从判断
    // 哪条才是游戏真正会读的键**。实测一个只有约 50 个真实键的 IL2CPP 作品被这样写了
    // 一次：3000 条垃圾键进注册表（全部 = 1），游戏照样不开内容；第二次再跑就变成
    // 「写入 0 个」，而报告里看不出任何异常。所以默认**拒绝**这种写入。
    if heuristic_write_blocked(
        ctx.opt("apply") == Some("1"),
        scan.usable(),
        family_prefix.as_deref(),
        ctx.opt("force_heuristic") == Some("1"),
    ) {
        return OpOutcome::fail(format!(
            "拒绝写入：候选来源不可靠（纯启发式，共 {} 条）。\n{asm_note}\n\
             ⚠ 这些候选是从场景/资源二进制里抽取的字符串，无法与画廊建立可靠对应。\
             直接写入会塞进大量与画廊无关的整数设置项，而且**改完你无法从报告里看出哪些写错了**。\n\
             → 先收窄再看预览：`--opt:filter=<子串>`（例如 --opt:filter=cg）；\n\
             → 若确认候选没问题，加 `--opt:force_heuristic=1` 强制写入（写前自动备份）；\n\
             → 写错了用 `--opt:undo=1 --opt:apply=1` 删除本工具写过的候选键。",
            candidates.len()
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
        "Unity 画廊解锁完成\n\
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
    // 「一条都没写」不等于「已经解锁」。候选全都在注册表里且都等于 1，最常见的原因
    // 是**上一次本工具把这张候选表整批写进来了**（候选名多是二进制碎片，游戏根本
    // 不会读），与游戏真实解锁状态无关。必须说清楚，别让人误以为已经成功。
    if written == 0 && !candidates.is_empty() && unlocked == candidates.len() {
        msg.push_str(&format!(
            "\n⚠ 本次一条都没写：全部 {} 个候选在注册表里**都已经等于 1**。\
             这通常说明这批候选是上一次运行本工具时被整批写进去的（候选名多为二进制里捞出的碎片，\
             并非游戏真实键），**不代表游戏已经解锁**。\
             建议用 `--opt:undo=1`（先只读预览，确认后加 `--opt:apply=1`）把本工具写过的候选键删干净，再重新评估。",
            unlocked
        ));
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

/// 撤销本工具的写入：把「候选键」在注册表里对应的值删掉。
///
/// 只删**候选**对应的值（也就是 `unlock --apply` 可能写过的那一批），绝不碰注册表
/// 里的其它值。默认只预览；`--opt:keep=<前缀,前缀>` 保护游戏真实键——命中保护前缀
/// 的候选一律不删（本工具无法自己判断哪条候选是真键，所以这个保护交给调用方给）。
fn undo_from(
    ctx: &Ctx,
    key: &winreg::PrefsKey,
    info: &AppInfo,
    candidates: &[String],
    existing: &[(String, u32, Vec<u8>)],
) -> OpOutcome {
    let location = format!("HKCU\\Software\\{}\\{}", info.company, info.product);
    let keep: Vec<String> = ctx
        .opt("keep")
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let present: BTreeSet<&str> = existing.iter().map(|(n, _, _)| n.as_str()).collect();

    let mut targets: Vec<String> = Vec::new();
    let mut protected: Vec<String> = Vec::new();
    let mut absent = 0usize;
    for c in candidates {
        let vname = prefs_value_name(c);
        if !present.contains(vname.as_str()) {
            absent += 1;
            continue;
        }
        if keep.iter().any(|k| c.starts_with(k.as_str())) {
            protected.push(c.clone());
            continue;
        }
        targets.push(vname);
    }

    if ctx.opt("apply") != Some("1") {
        let mut msg = format!(
            "【只读预览】撤销 Unity 画廊解锁写入\n\
             注册表位置: {location}\n\
             待删除（候选里有、注册表里也有）: {} 个\n\
             被 `--opt:keep` 保护未删: {} 个\n\
             候选里注册表本来就没有的: {absent} 个",
            targets.len(),
            protected.len()
        );
        if keep.is_empty() {
            msg.push_str(
                "\n⚠ 未指定 `--opt:keep=` —— 候选里可能混有游戏**真实**按键（例如 GameState_ / Skill_ 族），\
                 删掉会重置这些设置。强烈建议先给出游戏自己的键族前缀，例如：\
                 --opt:keep=GameState_,Skill_,FollowerData_,AutoObject_,AutoDevice,Milestone_,Volume_,unity,Screenmanager,Unity",
            );
        }
        if !targets.is_empty() {
            let sample: Vec<&String> = targets.iter().take(15).collect();
            msg.push_str(&format!("\n待删样例: {sample:?}"));
        }
        if !protected.is_empty() {
            let sample: Vec<&String> = protected.iter().take(15).collect();
            msg.push_str(&format!("\n受保护样例: {sample:?}"));
        }
        msg.push_str("\n\n→ 确认后加 `--opt:apply=1` 真正删除（删除前会自动整键备份）。");
        return OpOutcome::okn(msg, targets.len());
    }

    if targets.is_empty() {
        return OpOutcome::ok("没有需要删除的值（候选与注册表没有交集，或已全被 --opt:keep 保护）");
    }

    // 删除前把整键备份一份，万一误删还能回滚
    let snap = key.snapshot(&info.company, &info.product, &now_stamp());
    let bak_path = ctx
        .out_dir
        .join(format!("unity_playerprefs_undo_{}.json", snap.taken_at));
    if let Some(parent) = bak_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(e) = fs::write(&bak_path, serde_json::to_string_pretty(&snap).unwrap_or_default()) {
        return OpOutcome::fail(format!("备份注册表失败（已中止，未删除任何值）: {e}"));
    }

    let mut done = 0usize;
    let mut failed = 0usize;
    for (i, name) in targets.iter().enumerate() {
        match key.delete_value(name) {
            Ok(()) => done += 1,
            Err(e) => {
                failed += 1;
                crate::diag::log("WARN", &format!("gallery undo: {e}"));
            }
        }
        ctx.report((i + 1) as f32 / targets.len() as f32, name);
    }

    let mut msg = format!(
        "已删除 {done} 个值（失败 {failed}）\n\
         注册表位置: {location}\n\
         受 `--opt:keep` 保护未删 {} 个；候选里本就不存在 {absent} 个\n\
         删除前备份: {}",
        protected.len(),
        bak_path.display()
    );
    if failed > 0 {
        msg.push_str("\n⚠ 有删除失败项（多为权限问题），可重跑一次——删除是幂等的。");
    }
    OpOutcome {
        success: failed == 0,
        message: msg,
        files_done: done,
        logs: vec![],
    }
}

/// `--opt:set=` 的一行写入计划（先全部算好，任何一条不合法就整体中止）。
struct SetPlan {
    /// 用户写的裸键名
    name: String,
    /// 带 `_h<哈希>` 后缀的真实注册表值名
    vname: String,
    ty: u32,
    bytes: Vec<u8>,
    /// 原值（报告用）
    old: String,
    /// 新值（报告用）
    shown: String,
}

/// 按名写指定值（`--opt:set=名称=值,…`）。
///
/// 只写调用方点名的键：**没有候选、没有启发式、没有猜测**。类型按现存值决定
/// （见 `coerce_value`），写前整键快照、写后逐项读回校验；任何一条入参不合法都
/// **整体中止**，不留「写了一半」的中间态。
fn set_values(
    ctx: &Ctx,
    key: &winreg::PrefsKey,
    info: &AppInfo,
    existing: &[(String, u32, Vec<u8>)],
    spec: &str,
) -> OpOutcome {
    let pairs = match parse_set_pairs(spec) {
        Ok(p) => p,
        Err(e) => return OpOutcome::fail(e),
    };
    let location = format!("HKCU\\Software\\{}\\{}", info.company, info.product);

    let mut plan: Vec<SetPlan> = Vec::new();
    for (name, text) in &pairs {
        let vname = prefs_value_name(name);
        let hit = existing.iter().find(|(n, _, _)| n == &vname);
        let (old, ty, old_nul) = match hit {
            Some((_, t, d)) => (display_val(*t, d), Some(*t), d.last() == Some(&0)),
            None => ("（注册表里还没有这个键）".to_string(), None, false),
        };
        let (t, bytes) = match coerce_value(text, ty, old_nul) {
            Ok(v) => v,
            Err(e) => {
                return OpOutcome::fail(format!(
                    "{e}\n（一条都没写：入参有问题时不留半截状态）"
                ))
            }
        };
        let shown = if t == REG_DWORD_TYPE {
            display_val(t, &bytes)
        } else {
            format!("{text:?}")
        };
        plan.push(SetPlan {
            name: name.clone(),
            vname,
            ty: t,
            bytes,
            old,
            shown,
        });
    }

    if ctx.opt("apply") != Some("1") {
        let mut msg = format!(
            "【只读预览】按名写指定值（只写你点名的键，不做候选提取）\n\
             注册表位置: {location}\n将写入 {} 个键：",
            plan.len()
        );
        for p in &plan {
            msg.push_str(&format!(
                "\n  {} = {}  [{}]   原值 {}",
                p.name,
                p.shown,
                kind_label(p.ty),
                p.old
            ));
        }
        msg.push_str(
            "\n\n→ 确认后加 `--opt:apply=1` 真正写入（写前自动整键备份、写后逐项读回校验）。",
        );
        return OpOutcome::okn(msg, plan.len());
    }

    // 写前整键备份
    let snap = key.snapshot(&info.company, &info.product, &now_stamp());
    let bak_path = ctx
        .out_dir
        .join(format!("unity_playerprefs_set_{}.json", snap.taken_at));
    if let Some(parent) = bak_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(e) = fs::write(&bak_path, serde_json::to_string_pretty(&snap).unwrap_or_default()) {
        return OpOutcome::fail(format!("备份注册表失败（已中止，未写入任何值）: {e}"));
    }

    let mut done = 0usize;
    let mut verified = 0usize;
    let mut failed = 0usize;
    let mut lines: Vec<String> = Vec::new();
    for (i, p) in plan.iter().enumerate() {
        match key.set_raw(&p.vname, p.ty, &p.bytes) {
            Ok(()) => {
                done += 1;
                let ok = key
                    .get(&p.vname)
                    .map(|(rt, rd)| rt == p.ty && trim_nul(&rd) == trim_nul(&p.bytes))
                    .unwrap_or(false);
                if ok {
                    verified += 1;
                    lines.push(format!("  {}: {} → {}", p.name, p.old, p.shown));
                } else {
                    lines.push(format!("  {}: {} → {}  ⚠ 读回不一致", p.name, p.old, p.shown));
                }
            }
            Err(e) => {
                failed += 1;
                lines.push(format!("  {}: 写入失败 —— {e}", p.name));
                crate::diag::log("WARN", &format!("gallery set: {e}"));
            }
        }
        ctx.report((i + 1) as f32 / plan.len() as f32, &p.name);
    }

    let mut msg = format!(
        "按名写指定值完成：写入 {done} 个（读回校验通过 {verified} 个），失败 {failed} 个\n\
         注册表位置: {location}\n写前备份: {}\n{}",
        bak_path.display(),
        lines.join("\n")
    );
    if verified < done {
        msg.push_str("\n⚠ 有写入项读回校验未通过——请用 `--opt:restore=<备份文件>` 回滚后反馈");
    }
    msg.push_str(
        "\n提示：注册表是游戏**启动时**才读的，改完请先完全退出游戏再启动；\
         若游戏有「设置」界面，进去看一眼再退出，让它把状态写回。",
    );
    OpOutcome {
        success: failed == 0 && verified == done,
        message: msg,
        files_done: done,
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
    list_prefs_values(root).into_iter().map(|r| r.name).collect()
}

/// 注册表里一个现存值，按「给人看」的样子摆好（只读，不写）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegKeyRow {
    /// 原始键名（已剥掉 `_h<hash>` 后缀）。点名写入就用它。
    pub name: String,
    /// 该键当前的文本形态（DWORD 是十进制、字符串带引号、二进制文本带引号）。
    pub value: String,
    /// 类型的中文名，如 `DWORD` / `字符串` / `二进制`。
    pub kind: String,
    /// 原始注册表类型码（`--opt:set=` 会据此保持类型）。
    pub ty: u32,
    /// 键名所属的「族前缀」（到第一个 `_` 或 `.` 为止），供 UI 分组。
    pub family: String,
    /// 是否是 Unity / 引擎自身写的键（`unity.*`、`unity_connect.*`、`Screenmanager *`
    /// 等）。这些**不该动** —— 界面要单独标出来，别让用户误改。
    pub engine_own: bool,
}

/// 键族前缀：`GameState_Follower` → `GameState`，`unity.ready` → `unity`。
fn family_prefix(name: &str) -> String {
    let cut = name.find(['_', '.']).unwrap_or(name.len());
    name[..cut].to_string()
}

/// Unity / 引擎自身写的键名前缀（这些改坏了是环境问题，不是游戏进度）。
const ENGINE_OWN_PREFIXES: &[&str] = &["unity", "unity_connect", "Screenmanager", "UnitySelectMonitor"];

/// 是不是引擎自身的键。
fn is_engine_own(name: &str) -> bool {
    ENGINE_OWN_PREFIXES
        .iter()
        .any(|p| name == *p || name.starts_with(&format!("{p}.")) || name.starts_with(&format!("{p} ")))
}

/// 供 GUI / CLI 复用：把注册表里**全部现存值**连类型一起列出来，供用户照着点名改。
///
/// 起因：`--opt:set=` 只认「你告诉我键名」，可用户根本不知道有哪些键可改（SheepClicker
/// 那次就是这样卡住的）。这个函数把「有哪些键、现在是什么值、什么类型」摆到台面上，
/// **只读，绝不写**。
pub fn list_prefs_values(root: &Path) -> Vec<RegKeyRow> {
    let Some(info) = read_app_info(root) else {
        return Vec::new();
    };
    let Ok(key) = winreg::PrefsKey::open(&info.company, &info.product) else {
        return Vec::new();
    };
    let mut rows: Vec<RegKeyRow> = key
        .enum_values()
        .into_iter()
        .filter_map(|(n, ty, data)| {
            let (raw, _) = split_value_name(&n)?;
            Some(RegKeyRow {
                name: raw.to_string(),
                value: display_val(ty, &data),
                kind: kind_label(ty).to_string(),
                ty,
                family: family_prefix(raw),
                engine_own: is_engine_own(raw),
            })
        })
        .collect();
    // 引擎自身的键排到后面（用户要改的是游戏进度键），同类按名字定序，
    // 保证多次调用顺序稳定（UI 里跳来跳去会让人烦）。
    rows.sort_by(|a, b| {
        a.engine_own
            .cmp(&b.engine_own)
            .then_with(|| a.family.cmp(&b.family))
            .then_with(|| a.name.cmp(&b.name))
    });
    rows
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_gate_blocks_unreliable_candidates() {
        // 要写入 + 精确来源不可用 + 没有现存键族可收敛 → 拒绝。
        // 这就是 SheepClicker 被灌进 3000 条垃圾键的那个场景。
        assert!(heuristic_write_blocked(true, false, None, false));
        // 调用方显式知情选择 → 放行
        assert!(!heuristic_write_blocked(true, false, None, true));
        // 只读预览永远不拦（否则看不到候选，等于死胡同）
        assert!(!heuristic_write_blocked(false, false, None, false));
        // 精确来源可用（程序集 / IL2CPP 元数据字面量）→ 放行
        assert!(!heuristic_write_blocked(true, true, None, false));
        // 能用注册表里的现存键族收敛候选 → 放行
        assert!(!heuristic_write_blocked(true, false, Some("CG"), false));
    }

    #[test]
    fn reg_key_rows_group_and_flag_engine_own() {
        // 族前缀：到第一个 `_` 或 `.` 为止，没有分隔符就是整名。
        assert_eq!(family_prefix("GameState_Follower"), "GameState");
        assert_eq!(family_prefix("unity.ready"), "unity");
        assert_eq!(family_prefix("Volume_Master"), "Volume");
        assert_eq!(family_prefix("Solo"), "Solo");
        // 引擎自身的键要能认出来 —— 界面靠它把「别动这些」的那批分出来。
        assert!(is_engine_own("unity.ready"));
        assert!(is_engine_own("unity_connect.version"));
        assert!(is_engine_own("Screenmanager Resolution Width"));
        assert!(is_engine_own("UnitySelectMonitor"));
        // 游戏进度键不能误判成引擎自身的
        assert!(!is_engine_own("GameState_Money"));
        assert!(!is_engine_own("Skill_MaidenLevel"));
        assert!(!is_engine_own("Unityish_Thing"));
    }

    #[test]
    fn set_spec_parsing_is_strict() {
        let p = parse_set_pairs("GameState_Follower=9999,Skill_MaidenLevel=99").unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].0, "GameState_Follower");
        assert_eq!(p[0].1, "9999");
        // 名称可含空格（Unity 自己的 Screenmanager 键就长这样）
        let p = parse_set_pairs("Screenmanager Window Position X = 10").unwrap();
        assert_eq!(p[0].0, "Screenmanager Window Position X");
        assert_eq!(p[0].1, "10");
        // 空项跳过
        assert_eq!(parse_set_pairs("a=1,,b=2").unwrap().len(), 2);
        // 缺 `=` / 键名为空 / 全空 —— 宁可不写也不猜
        assert!(parse_set_pairs("a").is_err());
        assert!(parse_set_pairs("=1").is_err());
        assert!(parse_set_pairs("").is_err());
    }

    #[test]
    fn set_value_coercion_follows_existing_type() {
        // 现存 DWORD → 按整数写（含 0x 与负数）
        assert_eq!(
            coerce_value("9999", Some(REG_DWORD_TYPE), false).unwrap().1,
            9999u32.to_le_bytes()
        );
        assert_eq!(
            coerce_value("0x10", Some(REG_DWORD_TYPE), false).unwrap().1,
            16u32.to_le_bytes()
        );
        assert_eq!(
            coerce_value("-1", Some(REG_DWORD_TYPE), false).unwrap().1,
            u32::MAX.to_le_bytes()
        );
        // 现存字符串（REG_SZ）→ 保持字符串。`utf16z` 编的是 UTF-16LE，
        // 所以用 `display_val` 解回来核对，别拿 ASCII 比。
        let (t, b) = coerce_value("999999", Some(1), false).unwrap();
        assert_eq!(t, 1);
        assert_eq!(display_val(t, &b), "\"999999\"");
        // 现存文本型二进制（本作 GameState_Money 就是这样）→ 原样字节，跟原值的结尾 NUL
        let (t, b) = coerce_value("999999", Some(REG_BINARY_TYPE), true).unwrap();
        assert_eq!(t, REG_BINARY_TYPE);
        assert_eq!(trim_nul(&b), b"999999");
        assert_eq!(b.last(), Some(&0));
        assert_eq!(display_val(t, &b), "\"999999\"");
        let (_, b) = coerce_value("999999", Some(REG_BINARY_TYPE), false).unwrap();
        assert_eq!(b, b"999999");
        // display_val 认得「ASCII + 结尾 NUL」这种文本型二进制
        // （用真机实测的字节：GameState_Money = "6108" + NUL）
        assert_eq!(display_val(REG_BINARY_TYPE, &[b'6', b'1', b'0', b'8', 0]), "\"6108\"");
        // 真二进制（非可打印）只报字节数，不乱解成文本
        assert_eq!(display_val(REG_BINARY_TYPE, &[0xff, 0x00, 0x01]), "<3 字节>");
        // 不擅自改类型：认不出的类型（如 REG_DWORD_BIG_ENDIAN=5）拒绝改写
        assert!(coerce_value("1", Some(5), false).is_err());
        // 新键：能解析成整数就当 DWORD，否则当字符串
        assert_eq!(coerce_value("7", None, false).unwrap().0, REG_DWORD_TYPE);
        assert_eq!(coerce_value("hello", None, false).unwrap().0, 1);
        // DWORD 目标给非整数 → 报错（不猜）
        assert!(coerce_value("hello", Some(REG_DWORD_TYPE), false).is_err());
    }

    #[test]
    fn refused_set_reports_readable_registry_type() {
        // 拒绝改写时要把「它是什么类型」讲清楚，不能只丢一个数字 ——
        // 否则用户没法判断该不该手工去改。
        assert_eq!(kind_label(REG_DWORD_TYPE), "DWORD");
        assert_eq!(kind_label(1), "字符串");
        assert_eq!(kind_label(REG_BINARY_TYPE), "二进制");
        assert_eq!(kind_label(11), "QWORD");
        assert_eq!(kind_label(0x7fff_ffff), "未知类型");
        let e = coerce_value("1", Some(11), false).unwrap_err();
        assert!(e.contains("QWORD"), "{e}");
        assert!(e.contains("（11）"), "{e}");
        assert!(e.contains("拒绝改写"), "{e}");
    }

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
