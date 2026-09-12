//! KiriKiri / 吉里吉里 运行时补丁包（不改原封包、删除即还原）。
//!
//! ## 原理
//! KiriKiri（含吉里吉里Z / KAG）按固定顺序搜索资源，**后者覆盖前者**：
//!
//! ```text
//! Data 文件夹  →  data.xp3  →  patch.xp3  →  patch2.xp3  →  patch3.xp3  →  …
//! ```
//!
//! 因此把改过的文件（保持封包内原始相对路径）打进下一个空号的 `patchN.xp3`，
//! 引擎就会自动以更高优先级加载它 —— 几 GB 的 `data.xp3` 一个字节都不用动，
//! 删掉这个补丁包即完全还原。这也是社区汉化补丁的通行做法。
//!
//! ## 与「回写 data.xp3」相比
//! - 不用重写原封包，中途出错也不会毁掉游戏本体；
//! - 随时可删、可对比，方便反复迭代译文；
//! - 代价：封包结构必须与原名一致（同名同路径才能覆盖）。
//!
//! ## 已知边界
//! - 补丁包内容按**明文**写出（`info.flags = 0`）：引擎遇到未加密条目直接按明文读，
//!   所以即使游戏本体的封包是加密的，补丁包依然有效；
//! - 若游戏的 `Config.tjs` 没有为 patch 包声明搜索路径（`Storages.addAutoPath`），
//!   个别游戏会只在补丁包根目录查找文件；此时把文件放到补丁包根目录，
//!   或直接改用「解包 → 文本 → 回写 data.xp3」流程（见 `caveat`）；
//! - 补丁包只覆盖它包含的文件，未包含的文件仍读 `data.xp3`，所以可以只放改动的脚本。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 运行时补丁包的能力说明（供 GUI / CLI 展示，避免用户误解适用范围）。
pub struct PatchTarget {
    pub plugin_id: &'static str,
    pub engine: &'static str,
    /// 机制说明
    pub mechanism: &'static str,
    /// 兼容性 / 稳定性边界
    pub limits: &'static str,
    /// 个别游戏可能踩的坑与修法
    pub caveat: &'static str,
}

/// 补丁包能力总表（当前仅 KiriKiri 系；新增引擎时在这里加一行）。
pub const PATCH_TARGETS: &[PatchTarget] = &[PatchTarget {
    plugin_id: "kirikiri",
    engine: "KiriKiri / 吉里吉里（含吉里吉里Z、KAG）",
    mechanism: "把改动文件（保持封包内原始相对路径）打成 patchN.xp3；\
                引擎搜索顺序里 patch 包优先级高于 data.xp3，原封包不动、删除即还原",
    limits: "补丁包本身按明文写出，引擎按明文读取；只覆盖包内包含的文件，其余仍读原封包，\
             因此适合只放改过的脚本（.ks/.tjs/文本资源）；只是覆盖，不做字节级补丁",
    caveat: "个别游戏的 Config.tjs 未声明 patch 包的搜索路径（Storages.addAutoPath），\
             此时引擎会只在补丁包根目录找文件 —— 把文件放到补丁包根目录，\
             或改用「解包 → 文本 → 回写 data.xp3」流程（回写会沿用原封包的头部版本）",
}];

/// 该引擎是否支持「补丁包」做法。
pub fn supports(plugin_id: &str) -> bool {
    PATCH_TARGETS.iter().any(|t| t.plugin_id == plugin_id)
}

pub fn target_of(plugin_id: &str) -> Option<&'static PatchTarget> {
    PATCH_TARGETS.iter().find(|t| t.plugin_id == plugin_id)
}

/// 一个补丁包的状态。
pub struct PatchInfo {
    /// 文件名（如 `patch2.xp3`）
    pub name: String,
    /// 字节数
    pub bytes: u64,
    /// 条目数（能解析出来时）
    pub entries: Option<usize>,
    /// 是否由 STool 创建（有 sidecar 记录）
    pub ours: bool,
    /// 是否被引擎加密保护（STool 解不开内容，此处只作提示）
    pub encrypted: bool,
}

/// STool 创建的补丁包的 sidecar 记录文件名。
fn sidecar(root: &Path, name: &str) -> PathBuf {
    root.join(format!("{name}.stool.json"))
}

pub fn is_ours(root: &Path, name: &str) -> bool {
    sidecar(root, name).is_file()
}

/// 列出游戏目录下所有的 `patch*.xp3`（顺带把 `data*.xp3` 也列出来，方便判断搜索顺序）。
pub fn list(root: &Path) -> Vec<PatchInfo> {
    let mut out: Vec<PatchInfo> = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else { return out };
    for e in rd.flatten() {
        let path = e.path();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        let is_patch = lower.starts_with("patch") && lower.ends_with(".xp3");
        let is_data = lower.starts_with("data") && lower.ends_with(".xp3");
        if !is_patch && !is_data {
            continue;
        }
        let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
        let (entries, encrypted) = inspect(&path);
        let ours = is_ours(root, &name);
        out.push(PatchInfo { name, bytes, entries, ours, encrypted });
    }
    // 搜索顺序：data 在前，patch 系列按号排（patch < patch2 < patch3 …）
    out.sort_by_key(|p| (p.name.to_ascii_lowercase().starts_with("patch"), patch_index(&p.name)));
    out
}

/// `patch.xp3` → 1，`patch3.xp3` → 3，其它 → 0（排序用）。
fn patch_index(name: &str) -> u32 {
    let lower = name.to_ascii_lowercase();
    if !lower.starts_with("patch") || !lower.ends_with(".xp3") {
        return 0;
    }
    let mid = &lower[5..lower.len() - 4];
    if mid.is_empty() {
        1
    } else {
        mid.parse::<u32>().unwrap_or(u32::MAX)
    }
}

/// 只读索引拿条目数 / 是否加密（失败时都返回 None / false，不干扰列表展示）。
fn inspect(path: &Path) -> (Option<usize>, bool) {
    let Ok(data) = std::fs::read(path) else { return (None, false) };
    match crate::formats::xp3::parse_bytes(&data) {
        Ok(idx) => (Some(idx.len()), crate::formats::xp3::encrypted_count(&idx) > 0),
        Err(_) => (None, false),
    }
}

/// 下一个可用的补丁包名：按 `patch.xp3 / patch2.xp3 / patch3.xp3 …` 找空号。
///
/// 关键：**绝不覆盖游戏自带的补丁包**（那可能是官方修正或别人的汉化），
/// 所以只挑没被占用的号。
pub fn next_name(root: &Path) -> String {
    let mut used = std::collections::BTreeSet::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let i = patch_index(&name);
            if i > 0 {
                used.insert(i);
            }
        }
    }
    let mut n = 1u32;
    while used.contains(&n) {
        n += 1;
    }
    if n == 1 {
        "patch.xp3".to_string()
    } else {
        format!("patch{n}.xp3")
    }
}

/// 递归收集目录下的文件，键 = 相对路径（统一 `/` 分隔）。
fn collect(dir: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| format!("读取 {} 失败: {e}", d.display()))?;
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                let rel = p
                    .strip_prefix(dir)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, p);
            }
        }
    }
    Ok(out)
}

/// 生成补丁包：把 `src_dir` 下的文件（保持相对路径）打进 `<游戏目录>/<name>`。
///
/// `name` 传 None 时自动取下一个空号（推荐）；传具体名字时：
/// - 名字必须以 `patch` 开头、`.xp3` 结尾（挡掉误把 `data.xp3` 当补丁写的操作）；
/// - 已存在且**不是** STool 创建的 → 拒绝（不覆盖游戏自带补丁包）；
/// - 已存在且是 STool 创建的 → 允许重建（方便改完译文再来一次）。
pub fn build(root: &Path, src_dir: &Path, name: Option<&str>) -> Result<String, String> {
    if !src_dir.is_dir() {
        return Err(format!("改动目录不存在: {}", src_dir.display()));
    }
    let files = collect(src_dir)?;
    if files.is_empty() {
        return Err(format!(
            "改动目录里没有文件: {}（请把改好的脚本按封包内原始相对路径放进来，如 scenario/first.ks）",
            src_dir.display()
        ));
    }

    write_patch(root, files, name, &src_dir.display().to_string())
}

/// 只把**与游戏现有封包内容不同**的文件打进补丁包 —— 这是「解包 → 汉化 → 打补丁」的闭环：
/// 直接把解包产物目录整个交给它，未改动的文件会被自动跳过，补丁包里只有真正改过的东西。
///
/// 对比基准是游戏**当前实际生效**的内容：按引擎搜索顺序（`data.xp3 → patch.xp3 → patch2.xp3 → …`，
/// 后者覆盖前者）依次读出封包里的条目，所以即使游戏自带补丁包，比对结果也是正确的。
///
/// 典型用法：
/// ```text
/// stool-cli extract <游戏目录> -o <解包目录>        # 1. 解包
/// stool-cli text-extract <解包目录> -o text.csv      # 2. 提台词
/// stool-cli text-mtl text.csv …                      # 3. 机翻
/// stool-cli text-import <解包目录> text.csv          # 4. 回写译文到解包目录
/// stool-cli xp3-patch <游戏目录> --from-extract <解包目录>   # 5. 只把改动打成 patchN.xp3
/// ```
pub fn build_changed(root: &Path, src_dir: &Path, name: Option<&str>) -> Result<String, String> {
    if !src_dir.is_dir() {
        return Err(format!("解包目录不存在: {}", src_dir.display()));
    }
    let files = collect(src_dir)?;
    if files.is_empty() {
        return Err(format!("解包目录里没有文件: {}", src_dir.display()));
    }
    let originals = original_entries(root)?;
    let stems = archive_stems(root);

    let mut changed: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut skipped = 0usize;
    // 按封包分组，每个封包只打开一次（大封包重复打开很贵）
    let mut by_arc: BTreeMap<PathBuf, Vec<(String, crate::formats::xp3::Xp3Entry, PathBuf)>> = BTreeMap::new();
    for (rel, p) in &files {
        // 解包产物按「<解包目录>/<封包名>/<封包内路径>」落盘（见 engines 的 extract），
        // 这里要把 `<封包名>/` 这层剥掉，换回**封包内路径**——补丁包里的名字必须是后者。
        let key = to_entry_name(rel, &stems);
        match originals.get(&key) {
            Some((arc, e)) => by_arc.entry(arc.clone()).or_default().push((key, e.clone(), p.clone())),
            None => {
                // 封包里没有 → 新增文件，一律打包
                changed.insert(key, p.clone());
            }
        }
    }
    for (arc, items) in by_arc {
        let mut src = crate::formats::source::Source::open(&arc, crate::formats::source::MAX_ARCHIVE)
            .map_err(|e| format!("打开 {} 失败: {e}", arc.display()))?;
        for (rel, entry, path) in items {
            // 先比大小（省掉绝大多数读取），大小一致再逐字节比
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(u64::MAX);
            if size != entry_size(&entry) {
                changed.insert(rel, path);
                continue;
            }
            let orig = crate::formats::xp3::read_entry(&mut src, &entry).unwrap_or_default();
            let now = std::fs::read(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
            if orig == now {
                skipped += 1;
            } else {
                changed.insert(rel, path);
            }
        }
    }

    if changed.is_empty() {
        return Err(format!(
            "对比后没有发现任何改动（检查了 {} 个文件，全部与游戏现有内容一致）。\n\
             请确认：① 译文确实回写进了这个解包目录；② 目录结构与封包内路径一致（如 scenario/first.ks）。",
            files.len()
        ));
    }
    let n_changed = changed.len();
    let msg = write_patch(root, changed, name, &src_dir.display().to_string())?;
    Ok(format!("{msg}\n已跳过 {skipped} 个未改动文件，仅打包改动的 {n_changed} 个。"))
}

/// 读出游戏现有封包里的条目表：相对路径 → (所在封包, 条目)。
/// 顺序遵循引擎搜索规则，**后加载的覆盖先加载的**，因此拿到的是"当前实际生效"的内容。
fn original_entries(root: &Path) -> Result<BTreeMap<String, (PathBuf, crate::formats::xp3::Xp3Entry)>, String> {
    let mut out: BTreeMap<String, (PathBuf, crate::formats::xp3::Xp3Entry)> = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir(root) else { return Ok(out) };
    let mut arcs: Vec<PathBuf> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        if (lower.starts_with("data") || lower.starts_with("patch")) && lower.ends_with(".xp3") {
            arcs.push(p);
        }
    }
    // data 在前、patch 系列按号在后（后者覆盖前者）
    arcs.sort_by_key(|p| {
        let n = p.file_name().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        (n.starts_with("patch"), patch_index(&n))
    });
    for arc in arcs {
        // 打不开 / 解析失败的封包跳过（可能加密或损坏），不影响其余封包的比对
        let Ok(mut src) = crate::formats::source::Source::open(&arc, crate::formats::source::MAX_ARCHIVE) else {
            continue;
        };
        let Ok(idx) = crate::formats::xp3::parse_index(&mut src) else { continue };
        for (rel, entry) in idx {
            out.insert(rel.replace('\\', "/"), (arc.clone(), entry));
        }
    }
    Ok(out)
}

/// 条目在封包里的原始大小（各段原始大小之和）。
fn entry_size(e: &crate::formats::xp3::Xp3Entry) -> u64 {
    e.segments.iter().map(|s| s.orig_size).sum()
}

/// 游戏目录下所有封包的「名字主干」小写集合（`data.xp3` → `data`，`patch2.xp3` → `patch2`）。
/// 解包时每个封包会单独落到一个以主干命名的子目录里，据此把解包路径还原成封包内路径。
fn archive_stems(root: &Path) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let Ok(rd) = std::fs::read_dir(root) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if (lower.starts_with("data") || lower.starts_with("patch")) && lower.ends_with(".xp3") {
            out.insert(lower.trim_end_matches(".xp3").to_string());
        }
    }
    out
}

/// 解包目录里的相对路径 → 封包内路径。
/// 若首段正好是某个封包主干（`data` / `patch` / `patch2` …）则剥掉这一层；
/// 否则原样返回（用户也可能直接把 `<解包目录>/data` 这一层当输入）。
fn to_entry_name(rel: &str, stems: &std::collections::BTreeSet<String>) -> String {
    if let Some((first, rest)) = rel.split_once('/') {
        if stems.contains(&first.to_ascii_lowercase()) {
            return rest.to_string();
        }
    }
    rel.to_string()
}

/// 落盘：解析包名（含归属保护）→ 备份 → 写 XP3 → 写 sidecar。
fn write_patch(
    root: &Path,
    files: BTreeMap<String, PathBuf>,
    name: Option<&str>,
    origin: &str,
) -> Result<String, String> {
    let name = match name {
        Some(n) => {
            let lower = n.to_ascii_lowercase();
            if !lower.starts_with("patch") || !lower.ends_with(".xp3") {
                return Err(format!("补丁包名必须是 patch*.xp3（避免误写 data.xp3），当前为「{n}」"));
            }
            if root.join(n).exists() && !is_ours(root, n) {
                return Err(format!(
                    "{n} 已存在且不是 STool 创建的（可能是游戏自带补丁或他人汉化），拒绝覆盖。\
                     请换一个名字，或留空自动取下一个空号。"
                ));
            }
            n.to_string()
        }
        None => next_name(root),
    };

    let target = root.join(&name);
    // 写出前备份已有副本（只可能是 STool 自己上次生成的），保证可回退
    if target.exists() {
        crate::settings::backup_once(&target).map_err(|e| format!("备份 {name} 失败: {e}"))?;
    }
    let n = crate::formats::xp3::write_paths_v(&target, &files, crate::formats::xp3::Xp3Version::V2)
        .map_err(|e| format!("写出 {name} 失败: {e}"))?;

    // sidecar：记录「这个包是 STool 建的」，删除时据此保护游戏自带补丁
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let meta = serde_json::json!({
        "tool": "STool",
        "kind": "kirikiri-patch",
        "entries": n,
        "source": origin,
        "created_unix": created,
    });
    std::fs::write(sidecar(root, &name), serde_json::to_string_pretty(&meta).unwrap_or_default())
        .map_err(|e| format!("写入 {name}.stool.json 失败: {e}"))?;

    let bytes = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
    Ok(format!(
        "已生成补丁包 {name}（{n} 个文件，{}）→ {}\n\
         它会被引擎以高于 data.xp3 的优先级加载：直接启动游戏即可看到效果，原封包未改动。\
         想还原就删掉这个文件（本工具「移除」按钮，或手删 {name} 与 {name}.stool.json）。",
        crate::features::precheck::human_bytes(bytes),
        target.display(),
    ))
}

/// 删除 STool 创建的补丁包（连同 sidecar）。非 STool 创建的一律拒绝。
pub fn remove(root: &Path, name: &str) -> Result<String, String> {
    let target = root.join(name);
    if !target.exists() {
        return Err(format!("找不到 {name}"));
    }
    if !is_ours(root, name) {
        return Err(format!(
            "{name} 不是 STool 创建的（没有对应的 .stool.json 记录），拒绝删除 —— \
             以免误删游戏自带补丁或他人汉化。要删请手动处理。"
        ));
    }
    std::fs::remove_file(&target).map_err(|e| format!("删除 {name} 失败: {e}"))?;
    let sc = sidecar(root, name);
    if sc.exists() {
        let _ = std::fs::remove_file(&sc);
    }
    let bak = crate::settings::backup_path_for(&target);
    if bak.exists() {
        let _ = std::fs::remove_file(&bak);
    }
    Ok(format!("已删除 {name}，游戏还原为原封包内容。"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_xp3patch_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_src(dir: &Path) {
        std::fs::create_dir_all(dir.join("scenario")).unwrap();
        std::fs::write(dir.join("scenario").join("first.ks"), "*start\nこんにちは\n").unwrap();
        std::fs::create_dir_all(dir.join("system")).unwrap();
        std::fs::write(dir.join("system").join("config.tjs"), "; cfg\n").unwrap();
    }

    #[test]
    fn next_name_skips_existing_series() {
        let d = tmp("next");
        assert_eq!(next_name(&d), "patch.xp3");
        std::fs::write(d.join("patch.xp3"), b"x").unwrap();
        assert_eq!(next_name(&d), "patch2.xp3");
        std::fs::write(d.join("patch2.xp3"), b"x").unwrap();
        assert_eq!(next_name(&d), "patch3.xp3");
        // 非数字后缀的补丁包（如 patch_locale_cn.xp3）不占号，但要能列出来
        std::fs::write(d.join("patch_locale_cn.xp3"), b"x").unwrap();
        assert_eq!(next_name(&d), "patch3.xp3");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn build_creates_loadable_patch_and_marks_ownership() {
        let d = tmp("build");
        let src = d.join("patch_src");
        write_src(&src);
        let msg = build(&d, &src, None).unwrap();
        assert!(msg.contains("patch.xp3"), "{msg}");
        assert!(d.join("patch.xp3").is_file());
        assert!(is_ours(&d, "patch.xp3"));
        // 内容无损、条目为明文
        let data = std::fs::read(d.join("patch.xp3")).unwrap();
        let idx = crate::formats::xp3::parse_bytes(&data).unwrap();
        assert_eq!(idx.len(), 2);
        assert_eq!(crate::formats::xp3::encrypted_count(&idx), 0);
        let e = idx.get("scenario/first.ks").unwrap();
        assert_eq!(crate::formats::xp3::read_file(&data, e).unwrap(), "*start\nこんにちは\n".as_bytes());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn build_refuses_to_overwrite_foreign_patch() {
        let d = tmp("foreign");
        let src = d.join("patch_src");
        write_src(&src);
        std::fs::write(d.join("patch.xp3"), b"game's own patch").unwrap();
        let err = build(&d, &src, Some("patch.xp3")).unwrap_err();
        assert!(err.contains("拒绝覆盖"), "{err}");
        // 自动取号则没问题
        let msg = build(&d, &src, None).unwrap();
        assert!(msg.contains("patch2.xp3"), "{msg}");
        assert_eq!(std::fs::read(d.join("patch.xp3")).unwrap(), b"game's own patch");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn build_rejects_non_patch_name_and_empty_src() {
        let d = tmp("reject");
        let src = d.join("patch_src");
        write_src(&src);
        assert!(build(&d, &src, Some("data.xp3")).unwrap_err().contains("必须是 patch"));
        let empty = d.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(build(&d, &empty, None).unwrap_err().contains("没有文件"));
        assert!(build(&d, &d.join("nope"), None).unwrap_err().contains("不存在"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn remove_only_touches_our_own_patch() {
        let d = tmp("remove");
        let src = d.join("patch_src");
        write_src(&src);
        // 游戏自带补丁：删不掉
        std::fs::write(d.join("patch.xp3"), b"official").unwrap();
        assert!(remove(&d, "patch.xp3").unwrap_err().contains("拒绝删除"));
        assert!(d.join("patch.xp3").exists());
        // STool 建的：能删，且连 sidecar 一起清掉
        build(&d, &src, None).unwrap();
        assert!(d.join("patch2.xp3").is_file());
        assert!(d.join("patch2.xp3.stool.json").is_file());
        remove(&d, "patch2.xp3").unwrap();
        assert!(!d.join("patch2.xp3").exists());
        assert!(!d.join("patch2.xp3.stool.json").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rebuild_is_allowed_for_our_own_patch_and_keeps_backup() {
        let d = tmp("rebuild");
        let src = d.join("patch_src");
        write_src(&src);
        build(&d, &src, Some("patch.xp3")).unwrap();
        // 改内容后重建：允许覆盖自己的包，并留 .stool.bak
        std::fs::write(src.join("scenario").join("first.ks"), "*start\n新译文\n").unwrap();
        build(&d, &src, Some("patch.xp3")).unwrap();
        assert!(d.join("patch.xp3.stool.bak").is_file());
        let data = std::fs::read(d.join("patch.xp3")).unwrap();
        let idx = crate::formats::xp3::parse_bytes(&data).unwrap();
        let e = idx.get("scenario/first.ks").unwrap();
        assert_eq!(crate::formats::xp3::read_file(&data, e).unwrap(), "*start\n新译文\n".as_bytes());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn list_reports_order_and_ownership() {
        let d = tmp("list");
        let src = d.join("patch_src");
        write_src(&src);
        std::fs::write(d.join("data.xp3"), b"").unwrap();
        build(&d, &src, None).unwrap(); // patch.xp3（我们的）
        std::fs::write(d.join("patch2.xp3"), b"foreign").unwrap();
        let items = list(&d);
        let names: Vec<&str> = items.iter().map(|p| p.name.as_str()).collect();
        // data 在前，patch 系列按号排
        assert_eq!(names, vec!["data.xp3", "patch.xp3", "patch2.xp3"]);
        assert!(items[1].ours);
        assert!(!items[2].ours);
        assert_eq!(items[1].entries, Some(2));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn target_table_covers_kirikiri_only_for_now() {
        assert!(supports("kirikiri"));
        assert!(!supports("renpy"));
        assert!(target_of("kirikiri").unwrap().mechanism.contains("patch"));
    }

    /// 造一个真封包（内容取自临时文件），供 build_changed 当「原封包」比对基准。
    fn make_xp3(archive: &Path, files: &[(&str, &str)]) {
        let dir = archive.parent().unwrap();
        let mut map = BTreeMap::new();
        for (i, (name, content)) in files.iter().enumerate() {
            let p = dir.join(format!("seed_{i}.bin"));
            std::fs::write(&p, content).unwrap();
            map.insert(name.to_string(), p);
        }
        crate::formats::xp3::write_paths_v(archive, &map, crate::formats::xp3::Xp3Version::V2).unwrap();
    }

    #[test]
    fn build_changed_packs_only_modified_files() {
        let d = tmp("changed");
        make_xp3(
            &d.join("data.xp3"),
            &[("scenario/first.ks", "*start\nこんにちは\n"), ("system/config.tjs", "; cfg\n")],
        );

        // 解包目录与封包内容一致 → 必须报「没有发现改动」，且不产生补丁包
        let ext = d.join("extract");
        std::fs::create_dir_all(ext.join("scenario")).unwrap();
        std::fs::create_dir_all(ext.join("system")).unwrap();
        std::fs::write(ext.join("scenario").join("first.ks"), "*start\nこんにちは\n").unwrap();
        std::fs::write(ext.join("system").join("config.tjs"), "; cfg\n").unwrap();
        let err = build_changed(&d, &ext, None).unwrap_err();
        assert!(err.contains("没有发现任何改动"), "实得: {err}");
        assert!(!d.join("patch.xp3").exists(), "无改动不该产出补丁包");

        // 改一个文件 + 新增一个文件 → 补丁包里应恰好这两条，未改动的被跳过
        std::fs::write(ext.join("scenario").join("first.ks"), "*start\n你好\n").unwrap();
        std::fs::write(ext.join("scenario").join("extra.ks"), "*extra\n").unwrap();
        let msg = build_changed(&d, &ext, None).unwrap();
        assert!(msg.contains("已跳过 1 个未改动文件"), "实得: {msg}");

        let idx = crate::formats::xp3::parse(&d.join("patch.xp3")).unwrap();
        assert_eq!(idx.len(), 2, "只应打包改动/新增的 2 个文件");
        assert!(idx.contains_key("scenario/first.ks"));
        assert!(idx.contains_key("scenario/extra.ks"));
        assert!(!idx.contains_key("system/config.tjs"), "未改动文件不该进补丁包");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn build_changed_handles_extract_layout() {
        // 真实解包产物是 `<解包目录>/<封包名>/<封包内路径>`（如 extract/data/scenario/first.ks），
        // 比对与打包都必须换算回「封包内路径」，否则会把全部文件误判成新增。
        let d = tmp("changed_layout");
        make_xp3(&d.join("data.xp3"), &[("scenario/first.ks", "A\n")]);
        let ext = d.join("extract");
        let inner = ext.join("data").join("scenario");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("first.ks"), "A\n").unwrap();
        // 未改动 → 必须报「没有发现改动」
        assert!(
            build_changed(&d, &ext, None).unwrap_err().contains("没有发现任何改动"),
            "解包目录布局下也应正确识别「未改动」"
        );

        // 改动 → 补丁包条目名必须是封包内路径（不带 data/ 这一层）
        std::fs::write(inner.join("first.ks"), "B\n").unwrap();
        build_changed(&d, &ext, None).unwrap();
        let idx = crate::formats::xp3::parse(&d.join("patch.xp3")).unwrap();
        assert_eq!(idx.len(), 1);
        assert!(
            idx.contains_key("scenario/first.ks"),
            "键应为封包内路径，实得 {:?}",
            idx.keys().collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn build_changed_respects_existing_patch_override() {
        // 游戏自带 patch.xp3 覆盖了 data.xp3 里的同名文件：比对基准必须是「后者」（当前生效的那个）
        let d = tmp("changed_override");
        make_xp3(&d.join("data.xp3"), &[("scenario/first.ks", "OLD\n")]);
        make_xp3(&d.join("patch.xp3"), &[("scenario/first.ks", "NEW\n")]);

        let ext = d.join("extract");
        std::fs::create_dir_all(ext.join("scenario")).unwrap();
        // 与 data.xp3 相同、但与被覆盖后的生效内容不同 → 应判定为「有改动」
        std::fs::write(ext.join("scenario").join("first.ks"), "OLD\n").unwrap();
        build_changed(&d, &ext, Some("patch2.xp3")).unwrap();
        let idx = crate::formats::xp3::parse(&d.join("patch2.xp3")).unwrap();
        assert_eq!(idx.len(), 1);
        assert!(idx.contains_key("scenario/first.ks"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
