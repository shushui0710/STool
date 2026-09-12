//! Artemis Engine 原生支持：`.pfs`（`pf6` / `pf8`）解包与回封。
//!
//! 为什么单开一个原生插件而不是留给 `recognize.rs` 的表驱动识别器：
//! 识别器只能"认出来 + 建议用 GARbro"，而 `.pfs` 的格式已经完全解出并有两个
//! 1.5GB 级真机样本验证（`アマカノ3.pfs` / `美少女万華鏡1 root.pfs`），
//! 因此可以直接在工具内解包与回封，走「解包 → 汉化 → 回封」闭环。
//!
//! 识别判据（只读 `ScanCtx`，零额外 IO）：`.pfs` 数量、`.asb` 脚本、`.pfs2` 分卷。
//! 优先级 70 高于同名识别器的 32，所以只要有 `.pfs` 就由本插件接手。

use std::path::{Path, PathBuf};

use super::scan::ScanCtx;
use super::{Ctx, Detection, Engine, Job, Op, OpOutcome};
use crate::formats::pfs;
use crate::formats::source::{self, Source};

pub struct ArtemisPlugin;

/// 列举所有 `.pfs` 封包（递归；`.pfs` 可能不在游戏根，如 `万華鏡1` 在子目录里）。
fn list_pfs(root: &Path) -> Vec<PathBuf> {
    super::others::list_files_by_ext_pub(root, &["pfs"])
}

/// 分卷封包（`x.pfs.000`）：本工具不支持，但要能识别出来并如实说明。
fn list_split_pfs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(root).max_depth(4).into_iter().filter_map(|e| e.ok()) {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let Some(ext) = p.extension().and_then(|x| x.to_str()) else { continue };
        // 3 位数字扩展名 + 前缀以 .pfs 结尾（x.pfs.000 的 file_stem 是 "x.pfs"）
        if ext.len() == 3 && ext.chars().all(|c| c.is_ascii_digit()) {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem.to_ascii_lowercase().ends_with(".pfs") {
                out.push(p.to_path_buf());
            }
        }
    }
    out.sort();
    out
}

impl Engine for ArtemisPlugin {
    fn id(&self) -> &'static str {
        "artemis"
    }
    fn name(&self) -> &'static str {
        "Artemis Engine"
    }
    fn priority(&self) -> i32 {
        70
    }
    fn detect_scan(&self, scan: &ScanCtx) -> Detection {
        let mut d = Detection::new(self.id(), self.name());
        let n = scan.ext_count("pfs");
        if n > 0 {
            d.hit(70, format!("{n} 个 .pfs 封包（如 {}）", scan.first_ext_name("pfs")));
        }
        if scan.ext_count("pfs2") > 0 {
            d.hit(45, "存在 .pfs2（Artemis 新变体，STool 仅原生支持 .pfs）");
            d.note(".pfs2 目前只作识别，未内置解析");
        }
        let asb = scan.ext_count("asb");
        if asb > 0 {
            d.hit(25, format!("{asb} 个 .asb 脚本（Artemis 字节码）"));
        }
        if scan.has_ext("ast") {
            d.hit(15, "存在 .ast 明文脚本（已解包的 Artemis 游戏）");
        }
        // 分卷封包（x.pfs.000 / .001 …）：本工具不拼接，但要让用户"看得见"，
        // 否则纯分卷的游戏会彻底识别不出来（单条 45 分 → 只作疑似展示）。
        if n == 0 && scan.has_numbered_volume() {
            d.hit(45, "存在 3 位数字扩展名的分卷文件（可能是 x.pfs.000 / .001）");
            d.note("分卷封包 STool 暂不支持拼接，请用 GARbro 打开首个分卷导出");
        }
        d
    }
    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract, Op::Repack]
    }
    fn describe(&self, root: &Path) -> String {
        let pfs_list = list_pfs(root);
        if pfs_list.is_empty() {
            return "未发现 .pfs 封包".into();
        }
        let names: Vec<String> =
            pfs_list.iter().map(|p| super::others::file_name_pub(p)).collect();
        format!("封包 {} 个: {}", names.len(), names.join("、"))
    }
    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        let archives = list_pfs(ctx.root);
        if archives.is_empty() {
            let split = list_split_pfs(ctx.root);
            if !split.is_empty() {
                return OpOutcome::fail(format!(
                    "只发现 Artemis **分卷**封包（{}），STool 暂不支持分卷拼接。\
                     替代做法：用 GARbro 打开第一个分卷（如 x.pfs.000）导出，或找有现成解包工具的版本。",
                    super::others::file_name_pub(&split[0])
                ));
            }
            return OpOutcome::fail("未找到 .pfs 封包");
        }
        let mut done = 0usize;
        let mut failed = 0usize;
        let mut errors: Vec<String> = Vec::new();
        let mut resume = crate::engines::Resume::open(ctx.out_dir, "extract", ctx.root).configured(ctx);
        for (i, arc) in archives.iter().enumerate() {
            if ctx.cancelled() {
                resume.flush();
                return OpOutcome::fail("已取消");
            }
            let mut src = match Source::open(arc, source::MAX_ARCHIVE) {
                Ok(s) => s,
                Err(e) => {
                    errors.push(format!("{}: {e}", super::others::file_name_pub(arc)));
                    continue;
                }
            };
            let index = match pfs::parse_index(&mut src) {
                Ok(ix) => ix,
                Err(e) => {
                    errors.push(format!("{}: {e}", super::others::file_name_pub(arc)));
                    continue;
                }
            };
            // 与 KiriKiri 一致：解包到 <out>/<封包名>/<封包内路径>，
            // 这样回封时能按同名子目录找回，也避免多个封包的同名条目互相覆盖。
            let stem = arc.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let sub = ctx.out_dir.join(&stem);
            let mut jobs: Vec<Job<&pfs::PfsEntry>> = Vec::new();
            for e in &index.entries {
                // 封包内用 '\' 分隔，落盘统一成 '/'
                let rel = e.name.replace('\\', "/");
                if !resume.already_done(&sub, &rel) {
                    jobs.push(Job { rel, item: e });
                }
            }
            let skipped_here = index.entries.len() - jobs.len();
            let (w, f) = crate::engines::parallel_extract(
                &jobs,
                &sub,
                crate::engines::worker_count(ctx),
                ctx,
                || Source::open(arc, source::MAX_ARCHIVE),
                |s, e| pfs::read_entry(s, &index, e),
                &mut resume,
            );
            done += w + skipped_here;
            failed += f;
            ctx.report(i as f32 / archives.len() as f32, &arc.display().to_string());
        }
        let skipped = resume.finish(failed == 0 && errors.is_empty());
        if done == 0 {
            return OpOutcome::fail(format!(
                "未能从 .pfs 解出任何文件: {}",
                if errors.is_empty() { "（封包为空？）".to_string() } else { errors.join("; ") }
            ));
        }
        let mut msg = format!("解包 {done} 个文件 → {}", ctx.out_dir.display());
        if !errors.is_empty() {
            msg.push_str(&format!("；失败封包: {}", errors.join("; ")));
        }
        let msg = crate::engines::with_fail_note(msg, failed);
        let msg = crate::engines::with_resume_note(msg, skipped);
        OpOutcome::okn(msg, done)
    }
    fn repack(&self, ctx: &Ctx, src_dir: &Path) -> OpOutcome {
        let archives = list_pfs(ctx.root);
        if archives.is_empty() {
            return OpOutcome::fail("未找到 .pfs 封包");
        }
        let mut rebuilt = 0usize;
        let mut msgs: Vec<String> = Vec::new();
        for arc in &archives {
            let stem = arc.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let sub = src_dir.join(&stem);
            if !sub.is_dir() {
                continue;
            }
            let files = match super::others::collect_src_files(&sub) {
                Ok(f) => f,
                Err(e) => return OpOutcome::fail(e),
            };
            if files.is_empty() {
                continue;
            }
            // 打开原封包一次：既拿索引顺序（Artemis 线性查找，保持原顺序最稳），
            // 也拿代次（pf8 写的必须还是 pf8，写成 pf6 老游戏读不到）。
            let (orig_order, ver) = match Source::open(arc, source::MAX_ARCHIVE)
                .and_then(|mut s| pfs::parse_index(&mut s))
            {
                Ok(ix) => (
                    ix.entries.iter().map(|e| e.name.clone()).collect::<Vec<String>>(),
                    ix.version,
                ),
                Err(_) => (Vec::new(), b'8'),
            };
            // Artemis 封包内用 '\' 分隔（collect_src_files 给的是 '/'），转换后再写；
            // 原索引里有的按原顺序排在前，解包后新增的文件排在其后。
            let mut ordered: Vec<(String, Vec<u8>)> = Vec::with_capacity(files.len());
            let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
            for name in &orig_order {
                let disk_key = name.replace('\\', "/");
                if let Some(path) = files.get(&disk_key) {
                    match std::fs::read(path) {
                        Ok(b) => {
                            ordered.push((name.clone(), b));
                            used.insert(disk_key);
                        }
                        Err(e) => {
                            return OpOutcome::fail(format!("读取失败 {}: {e}", path.display()))
                        }
                    }
                }
            }
            for (key, path) in &files {
                if used.contains(key) {
                    continue;
                }
                let name = key.replace('/', "\\");
                match std::fs::read(path) {
                    Ok(b) => ordered.push((name, b)),
                    Err(e) => return OpOutcome::fail(format!("读取失败 {}: {e}", path.display())),
                }
            }
            if ordered.is_empty() {
                continue;
            }
            // 备份已存在则保留（绝不覆盖）；备份失败就停手，不做无备份改写。
            if let Err(e) = crate::settings::backup_or_abort(arc) {
                return OpOutcome::fail(format!("{}: {e}", super::others::file_name_pub(arc)));
            }
            match pfs::write_archive(arc, &ordered, ver) {
                Ok(()) => {
                    rebuilt += 1;
                    msgs.push(format!(
                        "{}（{} 文件，pf{}）",
                        super::others::file_name_pub(arc),
                        ordered.len(),
                        ver as char
                    ));
                }
                Err(e) => {
                    return OpOutcome::fail(format!("{} 写出失败: {e}", super::others::file_name_pub(arc)))
                }
            }
        }
        if rebuilt == 0 {
            return OpOutcome::fail(format!(
                "未找到可回写的目录（{} 下应存在与 .pfs 同名的子目录，如 root.pfs → {}/root/）",
                src_dir.display(),
                src_dir.display()
            ));
        }
        OpOutcome::okn(
            format!("已重建 {rebuilt} 个 pfs 封包（原文件均备份为 *.stool.bak）：{}", msgs.join("、")),
            rebuilt,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_artemis_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ctx<'a>(
        root: &'a Path,
        out: &'a Path,
        opts: &'a HashMap<String, String>,
        cancel: &'a AtomicBool,
        prog: &'a dyn Fn(f32, &str),
    ) -> Ctx<'a> {
        Ctx { root, out_dir: out, options: opts, progress: prog, cancel }
    }

    /// 造一个「游戏目录 + root.pfs」，并返回 (临时根, 游戏目录)。
    fn make_game(tag: &str) -> (PathBuf, PathBuf) {
        let d = tmp(tag);
        let root = d.join("game");
        std::fs::create_dir_all(&root).unwrap();
        let files = vec![
            ("system\\ui\\title.lua".to_string(), b"-- title\nreturn {}\n".to_vec()),
            ("pc\\bg_cn.png".to_string(), b"\x89PNG\r\n\x1a\nxxx".to_vec()),
        ];
        pfs::write_archive(&root.join("root.pfs"), &files, b'8').unwrap();
        (d, root)
    }

    #[test]
    fn detect_confirms_on_pfs() {
        let (d, root) = make_game("det");
        let det = ArtemisPlugin.detect(&root);
        assert!(det.ok(), "单个 .pfs 应确认 Artemis（实得 {} 分）", det.score);
        assert_eq!(det.plugin_id, "artemis");
        assert_eq!(det.name, "Artemis Engine");
        // .asb 脚本是补充证据
        std::fs::write(root.join("main.asb"), b"x").unwrap();
        let det2 = ArtemisPlugin.detect(&root);
        assert!(det2.score > det.score, ".asb 应加分");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn detect_split_volume_is_only_suspicious() {
        let d = tmp("split");
        std::fs::write(d.join("x.pfs.000"), b"x").unwrap();
        std::fs::write(d.join("x.pfs.001"), b"x").unwrap();
        let det = ArtemisPlugin.detect(&d);
        assert!(!det.ok(), "纯分卷只能作疑似，不能确认（实得 {} 分）", det.score);
        assert!(det.score > 0);
        assert!(det.notes.contains("分卷"), "应提示分卷不支持：{}", det.notes);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn extract_layout_and_repack_roundtrip() {
        let (d, root) = make_game("rt");
        let out = d.join("out");
        let opts = HashMap::new();
        let cancel = AtomicBool::new(false);
        let prog = |_: f32, _: &str| {};
        let c = ctx(&root, &out, &opts, &cancel, &prog);

        let r = ArtemisPlugin.extract(&c);
        assert!(r.success, "{}", r.message);
        // 布局：<out>/<封包名>/<封包内路径>，且 '\' 换成 '/'
        let lua = out.join("root").join("system").join("ui").join("title.lua");
        let png = out.join("root").join("pc").join("bg_cn.png");
        assert!(lua.is_file(), "{}", r.message);
        assert_eq!(std::fs::read(&png).unwrap(), b"\x89PNG\r\n\x1a\nxxx");

        // 改一个文件 → 回封（应备份原封包、沿用 pf8、未改条目字节不变）
        const NEW_TEXT: &str = "-- 汉化标题\n";
        std::fs::write(&lua, NEW_TEXT.as_bytes()).unwrap();
        let r2 = ArtemisPlugin.repack(&c, &out);
        assert!(r2.success, "{}", r2.message);
        assert!(root.join("root.pfs.stool.bak").is_file(), "原封包必须被备份");

        let arc = root.join("root.pfs");
        assert!(pfs::is_pfs(&arc), "回封产物仍是 pfs");
        let mut src = Source::open(&arc, source::MAX_ARCHIVE).unwrap();
        let ix = pfs::parse_index(&mut src).unwrap();
        assert_eq!(ix.version, b'8', "必须沿用原封包代次");
        let by_name: HashMap<&str, &pfs::PfsEntry> =
            ix.entries.iter().map(|e| (e.name.as_str(), e)).collect();
        assert_eq!(
            pfs::read_entry(&mut src, &ix, by_name["system\\ui\\title.lua"]).unwrap(),
            NEW_TEXT.as_bytes(),
            "改动应生效"
        );
        assert_eq!(
            pfs::read_entry(&mut src, &ix, by_name["pc\\bg_cn.png"]).unwrap(),
            b"\x89PNG\r\n\x1a\nxxx",
            "未改动条目的内容必须原样保留"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn extract_without_pfs_fails_with_reason() {
        let d = tmp("nofile");
        let out = d.join("out");
        let opts = HashMap::new();
        let cancel = AtomicBool::new(false);
        let prog = |_: f32, _: &str| {};
        let c = ctx(&d, &out, &opts, &cancel, &prog);
        let r = ArtemisPlugin.extract(&c);
        assert!(!r.success);
        assert!(r.message.contains("未找到 .pfs"), "{}", r.message);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn describe_lists_archives() {
        let (d, root) = make_game("desc");
        let s = ArtemisPlugin.describe(&root);
        assert!(s.contains("root.pfs"), "{s}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
