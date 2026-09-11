//! Ren'Py 引擎插件。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::scan::ScanCtx;
use super::{Ctx, Detection, Engine, Op, OpOutcome};
use crate::formats::rpa;

pub struct RenpyPlugin;

impl RenpyPlugin {
    fn save_locations(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let rp = Path::new(&appdata).join("RenPy");
            if rp.is_dir() {
                if let Ok(rd) = fs::read_dir(&rp) {
                    for d in rd.flatten() {
                        if d.path().is_dir() {
                            out.push(d.path());
                        }
                    }
                }
            }
        }
        let _ = root;
        out
    }
}

impl Engine for RenpyPlugin {
    fn id(&self) -> &'static str {
        "renpy"
    }
    fn name(&self) -> &'static str {
        "Ren'Py"
    }
    fn priority(&self) -> i32 {
        90
    }

    fn detect_scan(&self, scan: &ScanCtx) -> Detection {
        let mut d = Detection::new(self.id(), self.name());
        if scan.has_root_dir("renpy") {
            d.hit(45, "renpy/ 引擎目录");
        }
        let rpa = scan.ext_count("rpa");
        if rpa > 0 {
            d.hit(30, format!("{rpa} 个 .rpa 封包"));
        }
        // .rpyc/.rpym 是编译后的脚本；两者都算运行时脚本特征
        let rpyc = scan.ext_sum(&["rpyc", "rpym"]);
        if rpyc > 0 {
            d.hit(25, format!("{rpyc} 个 .rpyc/.rpym 脚本"));
        }
        // 旧版漏判点：纯源码分发（未打包、只留 .rpy）时应仍能识别
        let rpy = scan.ext_count("rpy");
        if rpy > 0 {
            d.hit(20, format!("{rpy} 个 .rpy 源码"));
        }
        if scan.has_root_dir("lib") && scan.has_dir_starting_with("py") {
            d.hit(15, "lib/py*-windows-* 运行时");
        }
        // 目录名兜底：有些发行版把引擎文件改名，但 game/ 与 gui/ 结构仍在
        if scan.has_dir("gui") && scan.has_dir("images") {
            d.hit(10, "game/gui + game/images 资源结构");
        }
        if d.score > 0 && d.score < 60 {
            d.note("证据不足：确认游戏目录是否指到了 Ren'Py 根目录（应含 renpy/ 与 game/）");
        }
        d
    }

    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract, Op::Repack, Op::Decompile, Op::TextExtract, Op::TextImport, Op::TextInject, Op::Save, Op::Unlock]
    }

    /// MTool 式 JSON 注入：生成 game/stool_translate.rpy，利用 Ren'Py 的
    /// config.replace_text 在文本显示前整句替换，不修改任何原始脚本。
    fn text_inject(&self, ctx: &Ctx, json_path: &Path) -> OpOutcome {
        let json = match json_path.exists() {
            true => json_path.to_path_buf(),
            false => ctx.root.join("translation.json"),
        };
        if !json.exists() {
            return OpOutcome::fail(format!("未找到翻译 JSON（{} 不存在）", json.display()));
        }
        match crate::features::inject::install(ctx.root, &json, self.id()) {
            Ok(m) => OpOutcome::ok(m),
            Err(e) => OpOutcome::fail(e),
        }
    }

    fn describe(&self, root: &Path) -> String {
        format!("封包: {} 个 .rpa；存档目录: {}", count_ext(&root.join("game"), "rpa"), {
            let locs = Self::save_locations(root);
            if locs.is_empty() { "未发现".to_string() } else { locs[0].display().to_string() }
        })
    }

    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        let rpas = list_ext(&ctx.root.join("game"), "rpa");
        if rpas.is_empty() {
            return OpOutcome::fail("未找到 .rpa 封包");
        }
        let mut done = 0usize;
        let mut failed = 0usize;
        let mut resume = crate::engines::Resume::open(ctx.out_dir, "extract", ctx.root).configured(ctx);
        for (i, rpa) in rpas.iter().enumerate() {
            if ctx.cancelled() {
                resume.flush();
                return OpOutcome::fail("已取消");
            }
            ctx.report(i as f32 / rpas.len() as f32, &rpa.display().to_string());
            let index = match rpa::read_index(rpa) {
                Ok(ix) => ix,
                Err(_) => {
                    resume.flush();
                    return self.extract_via_unrpa(rpa, ctx.out_dir);
                }
            };
            // 命中续传：读盘（read_file 走 seek）也一并省掉
            let mut jobs: Vec<crate::engines::Job<&[rpa::Chunk]>> = Vec::new();
            let mut skipped_here = 0usize;
            for (name, chunks) in index.entries.iter() {
                if resume.already_done(ctx.out_dir, name) {
                    skipped_here += 1;
                } else {
                    jobs.push(crate::engines::Job { rel: name.clone(), item: chunks.as_slice() });
                }
            }
            let (w, f) = crate::engines::parallel_extract(
                &jobs,
                ctx.out_dir,
                crate::engines::worker_count(ctx),
                ctx,
                || Ok::<(), String>(()),
                |_s, chunks| rpa::read_file(rpa, chunks),
                &mut resume,
            );
            done += w + skipped_here;
            failed += f;
        }
        let skipped = resume.finish(failed == 0);
        let msg = crate::engines::with_fail_note(format!("解包 {done} 个文件 → {}", ctx.out_dir.display()), failed);
        let msg = crate::engines::with_resume_note(msg, skipped);
        OpOutcome::okn(msg, done)
    }

    fn repack(&self, ctx: &Ctx, src_dir: &Path) -> OpOutcome {
        let out = ctx
            .opt("out_archive")
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.root.join("game").join("stool_repack.rpa"));
        let mut files = BTreeMap::new();
        for e in walkdir::WalkDir::new(src_dir).into_iter().filter_map(|e| e.ok()) {
            if e.file_type().is_file() {
                let rel = e
                    .path()
                    .strip_prefix(src_dir)
                    .unwrap_or(e.path())
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                if let Ok(data) = fs::read(e.path()) {
                    files.insert(rel, data);
                }
            }
        }
        match rpa::write_archive(&out, &files, 0xDEADBEEF) {
            Ok(()) => OpOutcome::okn(format!("已生成 {}（{} 个文件）", out.display(), files.len()), files.len()),
            Err(e) => OpOutcome::fail(e),
        }
    }

    fn decompile(&self, ctx: &Ctx) -> OpOutcome {
        let unrpyc = crate::settings::unrpyc_path();
        if !unrpyc.exists() {
            return OpOutcome::fail(format!("未找到 unrpyc: {}", unrpyc.display()));
        }
        let game = ctx.root.join("game");
        let tmp = ctx.out_dir.join("_rpyc_copy");
        let _ = fs::create_dir_all(&tmp);
        let mut n = 0;
        for e in walkdir::WalkDir::new(&game).into_iter().filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_file()
                && p.extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x == "rpyc" || x == "rpym")
                    .unwrap_or(false)
            {
                let rel = p.strip_prefix(&game).unwrap_or(p);
                let dst = tmp.join(rel);
                let _ = fs::create_dir_all(dst.parent().unwrap());
                if fs::copy(p, &dst).is_ok() {
                    n += 1;
                }
            }
        }
        if n == 0 {
            return OpOutcome::fail("未找到 .rpyc/.rpym");
        }
        ctx.report(0.3, "运行 unrpyc 反编译");
        let out = Command::new(python_exe())
            .args([unrpyc.to_string_lossy().as_ref(), "-c", tmp.to_string_lossy().as_ref()])
            .output();
        match out {
            Ok(o) if o.status.success() => OpOutcome::okn(format!("反编译 {n} 个脚本 → {}", tmp.display()), n),
            Ok(o) => OpOutcome::fail(format!(
                "unrpyc 失败: {}",
                String::from_utf8_lossy(&o.stderr).chars().rev().take(400).collect::<String>().chars().rev().collect::<String>()
            )),
            Err(e) => OpOutcome::fail(format!("调用 Python 失败: {e}")),
        }
    }

    fn text_extract(&self, ctx: &Ctx, out_csv: &Path) -> OpOutcome {
        let base = ctx
            .opt("rpy_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.root.join("game"));
        let mut files = Vec::new();
        for e in walkdir::WalkDir::new(&base).into_iter().filter_map(|e| e.ok()) {
            if e.path().is_file() && e.path().extension().map(|x| x == "rpy").unwrap_or(false) {
                files.push(e.path().to_path_buf());
            }
        }
        files.sort();
        if files.is_empty() {
            return OpOutcome::fail("无 .rpy 文件，请先反编译");
        }
        let mut rows: Vec<[String; 4]> = Vec::new(); // id, context, source, translation
        for (i, p) in files.iter().enumerate() {
            let text = fs::read_to_string(p).unwrap_or_default();
            let rel = p.strip_prefix(&base).map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            for (ln, line) in text.lines().enumerate() {
                let s = line.trim();
                if s.is_empty()
                    || s.starts_with('#')
                    || ["define ", "$ ", "jump ", "call ", "scene ", "show ", "hide ", "play ", "stop ", "label ", "init ", "style ", "image "]
                        .iter()
                        .any(|kw| s.starts_with(kw))
                {
                    continue;
                }
                if let Some((_, txt)) = parse_say_line(line) {
                    if !txt.is_empty() {
                        rows.push([format!("{}:{}", rel, ln + 1), "dialogue".into(), txt, String::new()]);
                    }
                    continue;
                }
                if s.starts_with("menu") || s.starts_with("textbutton") || s.starts_with("caption") {
                    for txt in regex_strings(s) {
                        if !txt.is_empty()
                            && !txt.starts_with("images/")
                            && !txt.starts_with("audio/")
                            && !txt.starts_with("gui/")
                            && !txt.starts_with('{')
                        {
                            rows.push([format!("{}:{}", rel, ln + 1), "ui/choice".into(), txt, String::new()]);
                        }
                    }
                }
            }
            ctx.report((i + 1) as f32 / files.len() as f32, &p.display().to_string());
        }
        let n = rows.len();
        match crate::features::text::write_csv(out_csv, &rows) {
            Ok(()) => OpOutcome::okn(format!("提取 {n} 条文本 → {}", out_csv.display()), n),
            Err(e) => OpOutcome::fail(e),
        }
    }

    fn text_import(&self, ctx: &Ctx, csv_path: &Path) -> OpOutcome {
        let base = ctx
            .opt("rpy_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.root.join("game"));
        let out_root = ctx
            .opt("out_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| base.join("_translated"));
        let rows = match crate::features::text::read_csv(csv_path) {
            Ok(r) => r,
            Err(e) => return OpOutcome::fail(e),
        };
        let mut edits: BTreeMap<String, BTreeMap<usize, String>> = BTreeMap::new();
        for r in &rows {
            if r[3].is_empty() {
                continue;
            }
            if let Some((fname, ln)) = r[0].rsplit_once(':') {
                if let Ok(ln) = ln.parse::<usize>() {
                    edits.entry(fname.to_string()).or_default().insert(ln, r[3].clone());
                }
            }
        }
        let mut done = 0usize;
        let mut failed = 0usize;
        for (fname, map) in &edits {
            let src = base.join(fname);
            if !src.exists() {
                continue;
            }
            let mut lines: Vec<String> = fs::read_to_string(&src).unwrap_or_default().lines().map(|s| s.to_string()).collect();
            for (ln, text) in map {
                if *ln >= 1 && *ln <= lines.len() {
                    let indent = lines[*ln - 1].chars().take_while(|c| c.is_whitespace()).collect::<String>();
                    lines[*ln - 1] = format!("{indent}\"{text}\"");
                    done += 1;
                }
            }
            let dst = out_root.join(fname);
            if let Some(parent) = dst.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    failed += 1;
                    crate::diag::log("WARN", &format!("建目录失败 {}: {e}", parent.display()));
                    continue;
                }
            }
            if let Err(e) = fs::write(&dst, lines.join("\n") + "\n") {
                failed += 1;
                crate::diag::log("WARN", &format!("回写失败 {}: {e}", dst.display()));
            }
        }
        let msg = crate::engines::with_fail_note(format!("回填 {done} 条 → {}（把目录复制回 game/ 生效）", out_root.display()), failed);
        OpOutcome::okn(msg, done)
    }

    fn save(&self, ctx: &Ctx) -> OpOutcome {
        let locs = Self::save_locations(ctx.root);
        if locs.is_empty() {
            return OpOutcome::fail("未找到 %AppData%/RenPy/ 存档目录");
        }
        let mut detail = Vec::new();
        for d in &locs {
            let n = walkdir::WalkDir::new(d).into_iter().filter(|e| e.as_ref().map(|x| x.file_type().is_file()).unwrap_or(false)).count();
            detail.push(format!("{}: {n} 个文件", d.display()));
        }
        OpOutcome { success: true, message: "存档目录已定位".into(), files_done: locs.len(), logs: detail }
    }

    fn unlock_impl(
        &self,
        route: crate::features::unlock::UnlockRoute,
        ctx: &Ctx,
    ) -> Option<OpOutcome> {
        if route != crate::features::unlock::UnlockRoute::SaveFileFlag {
            return None;
        }
        Some(self.unlock_persistent(ctx))
    }
}

impl RenpyPlugin {
    /// Ren'Py 的 persistent 解锁实现（由统一调度器在 `SaveFileFlag` 路线下调用）。
    ///
    /// 缺省只读列出可解锁布尔键，`--opt:set_true_all=1` 才写回（写前备份原文件）。
    fn unlock_persistent(&self, ctx: &Ctx) -> OpOutcome {
        let target = match ctx.opt("persistent").map(PathBuf::from) {
            Some(p) => p,
            None => {
                let mut found = None;
                for d in Self::save_locations(ctx.root) {
                    let p = d.join("persistent");
                    if p.exists() {
                        found = Some(p);
                        break;
                    }
                }
                match found {
                    Some(p) => p,
                    None => return OpOutcome::fail("未找到 persistent 文件"),
                }
            }
        };
        let raw = match fs::read(&target) {
            Ok(r) => r,
            Err(e) => return OpOutcome::fail(e.to_string()),
        };
        let start = raw.windows(2).position(|w| w == [0x80, 0x02]).unwrap_or(0);
        let parsed = crate::formats::pickle::loads(&raw[start..]);
        match parsed {
            Ok(v) => {
                if let Some(dict) = v.as_dict() {
                    let keywords = ["unlock", "seen", "clear", "cg", "gallery", "replay", "end"];
                    let mut candidates = Vec::new();
                    for (k, val) in dict {
                        if matches!(val, crate::formats::pickle::Value::Bool(false)) {
                            let key_str = String::from_utf8_lossy(k.as_bytes().unwrap_or(b"")).to_lowercase();
                            if keywords.iter().any(|kw| key_str.contains(kw)) {
                                candidates.push(key_str);
                            }
                        }
                    }
                    if ctx.opt("set_true_all") == Some("1") {
                        // 回写：改 pickle 后重新序列化（仅支持简单键值：True 置换）
                        let patched = patch_false_to_true(&raw[start..], &candidates);
                        if let Some(bak) = backup_file(&target) {
                            if fs::write(&target, &patched).is_ok() {
                                return OpOutcome::okn(format!(
                                    "已将 {} 个解锁布尔键置 True（原文件备份为 {}）",
                                    candidates.len(),
                                    bak.display()
                                ), candidates.len());
                            }
                            return OpOutcome::fail("写回 persistent 失败");
                        }
                        return OpOutcome::fail("备份 persistent 失败");
                    }
                    OpOutcome::okn(format!("共 {} 键，可解锁布尔键 {} 个: {:?}（用 --opt set_true_all=1 一键解锁）", dict.len(), candidates.len(), candidates.iter().take(30).collect::<Vec<_>>()), candidates.len())
                } else {
                    OpOutcome::ok("persistent 已解析（非字典结构，需手动处理）")
                }
            }
            Err(e) => OpOutcome::fail(format!("persistent 解析失败: {e}")),
        }
    }
}

impl RenpyPlugin {
    fn extract_via_unrpa(&self, rpa: &Path, out_dir: &Path) -> OpOutcome {
        let out = Command::new(python_exe())
            .args(["-m", "unrpa", "-m", "-p", out_dir.to_string_lossy().as_ref(), rpa.to_string_lossy().as_ref()])
            .output();
        match out {
            Ok(o) if o.status.success() => OpOutcome::ok(format!("unrpa 解包完成 → {}", out_dir.display())),
            Ok(o) => OpOutcome::fail(format!(
                "解包失败: {}",
                String::from_utf8_lossy(&o.stderr)
            )),
            Err(e) => OpOutcome::fail(format!("unrpa 调用失败: {e}")),
        }
    }
}

/// 把 pickle 中的 false 布尔值按候选键名改成 true（仅对 persistent 顶部字典的简单键）。
fn patch_false_to_true(data: &[u8], candidates: &[String]) -> Vec<u8> {
    // pickle 中 NEWFALSE 是 0x89。我们只处理 "键名 pickle 序列 + 0x89" 的模式：
    // 对每个候选键，找到 "X<len><name> \x89"（BINUNICODE + NEWFALSE）并替换为 NEWTRUE (0x88)。
    let mut out = data.to_vec();
    for key in candidates {
        let keyb = key.as_bytes();
        let mut pat = vec![b'X'];
        pat.extend_from_slice(&(keyb.len() as u32).to_le_bytes());
        pat.extend_from_slice(keyb);
        pat.push(0x89); // NEWFALSE
        let mut pos = 0;
        while pos + pat.len() <= out.len() {
            if out[pos..pos + pat.len()] == pat[..] {
                out[pos + pat.len() - 1] = 0x88; // NEWTRUE
            }
            pos += 1;
        }
    }
    out
}

/// 写回 persistent 前的备份（委托 `settings::backup_once`：**已存在则绝不覆盖**）。
///
/// 旧实现是裸 `fs::copy`，重复解锁第二次就把备份从"原始存档"顶成"已解锁存档"，
/// 原始状态再无法恢复。这里统一走全局策略。
fn backup_file(path: &Path) -> Option<PathBuf> {
    crate::settings::backup_once(path).ok()
}

fn python_exe() -> String {
    std::env::var("STOOL_PYTHON").unwrap_or_else(|_| "python".to_string())
}

pub fn count_ext(dir: &Path, ext: &str) -> usize {
    list_ext(dir, ext).len()
}

pub fn list_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for d in rd.flatten() {
            let p = d.path();
            if p.is_file() && p.extension().map(|e| e == ext).unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// 解析 Ren'Py say 行：返回 (indent, 文本)。
pub fn parse_say_line(line: &str) -> Option<(&str, String)> {
    let indent_end = line.len() - line.trim_start().len();
    let indent = &line[..indent_end];
    let rest = line[indent_end..].trim();
    let qpos = if rest.starts_with('"') { 0 } else { rest.find('"')? };
    let speaker = rest[..qpos].trim();
    if !speaker.is_empty()
        && !speaker.chars().all(|c| c.is_alphanumeric() || "_.[]'".contains(c))
    {
        return None;
    }
    let inner = &rest[qpos + 1..];
    let chars: Vec<char> = inner.chars().collect();
    let mut i = 0;
    let mut text = String::new();
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            text.push(chars[i]);
            text.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if chars[i] == '"' {
            break;
        }
        text.push(chars[i]);
        i += 1;
    }
    Some((indent, text))
}

fn regex_strings(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            let mut j = i + 1;
            let mut buf = String::new();
            while j < chars.len() {
                if chars[j] == '\\' && j + 1 < chars.len() {
                    buf.push(chars[j]);
                    buf.push(chars[j + 1]);
                    j += 2;
                    continue;
                }
                if chars[j] == '"' {
                    break;
                }
                buf.push(chars[j]);
                j += 1;
            }
            out.push(buf);
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}
