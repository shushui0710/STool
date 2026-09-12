//! 其余引擎插件：RPG Maker MV/MZ、RGSS、KiriKiri、Godot、NScripter、HTML/Electron。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::scan::ScanCtx;
use super::{Ctx, Detection, Engine, Op, OpOutcome};
use crate::formats::source::{self, Source};
use crate::formats::{asar, marshal, nscript, pck, rgss, rpgmmv, xp3};

pub fn list_files_by_ext(root: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_file() {
            if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
                if exts.contains(&ext.to_lowercase().as_str()) {
                    out.push(p.to_path_buf());
                }
            }
        }
    }
    out.sort();
    out
}

/// 供兄弟模块使用的别名。
pub fn list_files_by_ext_pub(root: &Path, exts: &[&str]) -> Vec<PathBuf> {
    list_files_by_ext(root, exts)
}

/// 供兄弟模块使用的别名（`file_name` 本体是模块私有）。
pub fn file_name_pub(p: &Path) -> String {
    file_name(p)
}

// ---------------- RPG Maker MV / MZ ----------------

pub struct RpgMakerMvPlugin;

impl RpgMakerMvPlugin {
    pub fn data_dir(root: &Path) -> Option<PathBuf> {
        [root.join("www").join("data"), root.join("data")]
            .into_iter()
            .find(|c| c.join("Actors.json").exists())
    }
}

impl Engine for RpgMakerMvPlugin {
    fn id(&self) -> &'static str {
        "rpgmaker_mv"
    }
    fn name(&self) -> &'static str {
        "RPG Maker MV / MZ"
    }
    fn priority(&self) -> i32 {
        85
    }
    fn detect_scan(&self, scan: &ScanCtx) -> Detection {
        let mut d = Detection::new(self.id(), self.name());
        // Actors.json 是 MV/MZ 独有的核心数据（在 data/ 或 www/data/ 里，故走白名单名前查）
        if scan.has_file_named("actors.json") {
            d.hit(65, "data/Actors.json 游戏数据");
        }
        let enc = scan.ext_sum(&["rpgmvp", "rpgmvo", "rpgmvm"]);
        let enc_mz = scan.ext_sum(&["png_", "ogg_", "m4a_"]);
        if enc + enc_mz > 0 {
            d.hit(25, format!("{} 个 MV/MZ 加密素材", enc + enc_mz));
        }
        if scan.has_file_named("game.rpgproject") {
            d.hit(15, "Game.rpgproject 工程文件");
        }
        if scan.has_dir("js") {
            d.hit(10, "js/ 脚本目录");
        }
        d
    }
    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract, Op::Repack, Op::TextExtract, Op::TextImport, Op::TextInject, Op::Save]
    }
    fn text_inject(&self, ctx: &Ctx, json_path: &Path) -> OpOutcome {
        // 未指定或指定的 JSON 不存在时，回退到游戏目录下的 MTool 默认文件名
        let json = match json_path.exists() {
            true => json_path.to_path_buf(),
            false => ctx.root.join("translation.json"),
        };
        if !json.exists() {
            return OpOutcome::fail(format!(
                "未找到翻译 JSON（{} 不存在）。格式：{{\"原文\":\"译文\"}}，可从“文本提取”的 CSV 生成",
                json.display()
            ));
        }
        match crate::features::inject::install(ctx.root, &json, self.id()) {
            Ok(m) => OpOutcome::ok(m),
            Err(e) => OpOutcome::fail(e),
        }
    }
    fn describe(&self, root: &Path) -> String {
        match Self::data_dir(root) {
            Some(d) => format!("数据目录: {}", d.display()),
            None => String::new(),
        }
    }
    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        match rpgmmv::decrypt_dir(ctx.root, ctx.out_dir) {
            Ok(0) => OpOutcome::fail("未发现加密素材"),
            Ok(n) => OpOutcome::okn(format!("解密 {n} 个素材 → {}", ctx.out_dir.display()), n),
            Err(e) => OpOutcome::fail(e),
        }
    }
    fn repack(&self, ctx: &Ctx, src_dir: &Path) -> OpOutcome {
        match rpgmmv::encrypt_into(ctx.root, src_dir, ctx.opt("mz") == Some("1")) {
            Ok(n) => OpOutcome::okn(format!("加密回写 {n} 个素材"), n),
            Err(e) => OpOutcome::fail(e),
        }
    }
    fn text_extract(&self, ctx: &Ctx, out_csv: &Path) -> OpOutcome {
        let data_dir = match Self::data_dir(ctx.root) {
            Some(d) => d,
            None => return OpOutcome::fail("未找到 data/ 目录"),
        };
        let mut rows: Vec<[String; 4]> = Vec::new();
        let mut maps: Vec<PathBuf> = fs::read_dir(&data_dir)
            .map(|rd| {
                rd.flatten()
                    .map(|d| d.path())
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n.starts_with("Map") && n.ends_with(".json"))
                            .unwrap_or(false)
                    })
                    .collect()
            })
            .unwrap_or_default();
        maps.sort();
        for (i, f) in maps.iter().enumerate() {
            if let Ok(text) = fs::read_to_string(f) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    let ctx_name = v.get("displayName").and_then(|x| x.as_str()).unwrap_or("");
                    if let Some(events) = v.get("events").and_then(|x| x.as_array()) {
                        for ev in events {
                            if ev.is_null() {
                                continue;
                            }
                            let ename = ev.get("name").and_then(|x| x.as_str()).unwrap_or("");
                            let eid = ev.get("id").and_then(|x| x.as_i64()).unwrap_or(0);
                            let pages = match ev.get("pages").and_then(|x| x.as_array()) {
                                Some(p) => p.clone(),
                                None => {
                                    if ev.get("list").is_some() {
                                        vec![ev.clone()]
                                    } else {
                                        vec![]
                                    }
                                }
                            };
                            for (pi, page) in pages.iter().enumerate() {
                                let list = page.get("list").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                                for (li, cmd) in list.iter().enumerate() {
                                    let code = cmd.get("code").and_then(|x| x.as_i64()).unwrap_or(0);
                                    match code {
                                        401 => {
                                            if let Some(para) = cmd.get("parameters").and_then(|x| x.get(0)).and_then(|x| x.as_str()) {
                                                if !para.is_empty() {
                                                    rows.push([
                                                        format!("{}|{eid}|{pi}|{li}", file_name(f)),
                                                        format!("{ctx_name}/{ename}"),
                                                        para.to_string(),
                                                        String::new(),
                                                    ]);
                                                }
                                            }
                                        }
                                        102 => {
                                            if let Some(choices) = cmd.get("parameters").and_then(|x| x.get(0)).and_then(|x| x.as_array()) {
                                                for (ci, choice) in choices.iter().enumerate() {
                                                    if let Some(s) = choice.as_str() {
                                                        if !s.is_empty() {
                                                            rows.push([
                                                                format!("{}|{eid}|{pi}|{li}|c{ci}", file_name(f)),
                                                                format!("{ctx_name}/{ename}/选项"),
                                                                s.to_string(),
                                                                String::new(),
                                                            ]);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        405 => {
                                            if let Some(para) = cmd.get("parameters").and_then(|x| x.get(0)).and_then(|x| x.as_str()) {
                                                if !para.is_empty() {
                                                    rows.push([
                                                        format!("{}|{eid}|{pi}|{li}|s", file_name(f)),
                                                        format!("{ctx_name}/滚动"),
                                                        para.to_string(),
                                                        String::new(),
                                                    ]);
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            }
            ctx.report((i + 1) as f32 / maps.len().max(1) as f32, &f.display().to_string());
        }
        // 词条
        for sysfile in ["Items.json", "Skills.json", "Armors.json", "Weapons.json", "Classes.json", "Actors.json"] {
            let fp = data_dir.join(sysfile);
            if let Ok(text) = fs::read_to_string(&fp) {
                if let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(&text) {
                    for item in items {
                        if let (Some(id), Some(name)) = (
                            item.get("id").and_then(|x| x.as_i64()),
                            item.get("name").and_then(|x| x.as_str()),
                        ) {
                            if !name.is_empty() {
                                rows.push([format!("{sysfile}|{id}|name"), "词条".into(), name.into(), String::new()]);
                            }
                        }
                    }
                }
            }
        }
        let n = rows.len();
        match crate::features::text::write_csv(out_csv, &rows) {
            Ok(()) => OpOutcome::okn(format!("提取 {n} 条文本 → {}", out_csv.display()), n),
            Err(e) => OpOutcome::fail(e),
        }
    }
    fn text_import(&self, ctx: &Ctx, csv_path: &Path) -> OpOutcome {
        let data_dir = match Self::data_dir(ctx.root) {
            Some(d) => d,
            None => return OpOutcome::fail("未找到 data/ 目录"),
        };
        let rows = match crate::features::text::read_csv(csv_path) {
            Ok(r) => r,
            Err(e) => return OpOutcome::fail(e),
        };
        let out_dir = ctx
            .opt("out_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.parent().unwrap().join("data_translated"));
        let _ = fs::create_dir_all(&out_dir);
        let mut edits: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for r in &rows {
            if !r[3].is_empty() {
                edits.entry(r[1].clone()).or_default().push((r[0].clone(), r[3].clone()));
            }
        }
        let mut done = 0usize;
        let mut write_failed = 0usize;
        for (fname, pairs) in &edits {
            let fp = data_dir.join(fname);
            if !fp.exists() {
                continue;
            }
            let mut v: serde_json::Value = match fs::read_to_string(&fp).ok().and_then(|t| serde_json::from_str(&t).ok()) {
                Some(v) => v,
                None => continue,
            };
            if !fname.starts_with("Map") {
                // 词条
                if let Some(arr) = v.as_array_mut() {
                    for item in arr.iter_mut() {
                        if item.is_null() {
                            continue;
                        }
                        let id = item.get("id").and_then(|x| x.as_i64());
                        for (key, val) in pairs {
                            let parts: Vec<&str> = key.split('|').collect();
                            if parts.len() == 3 && parts[1].parse::<i64>().ok() == id {
                                item.as_object_mut().unwrap().insert("name".into(), serde_json::json!(val));
                                done += 1;
                            }
                        }
                    }
                }
            } else {
                if let Some(events) = v.get_mut("events").and_then(|x| x.as_array_mut()) {
                    for ev in events.iter_mut() {
                        if ev.is_null() {
                            continue;
                        }
                        let eid = ev.get("id").and_then(|x| x.as_i64()).unwrap_or(-1);
                        let pages: Vec<serde_json::Value> = match ev.get("pages").and_then(|x| x.as_array()) {
                            Some(p) => p.clone(),
                            None => {
                                if ev.get("list").is_some() {
                                    vec![ev.clone()]
                                } else {
                                    vec![]
                                }
                            }
                        };
                        // 直接改 events[i].pages[pi].list[li] / events[i].list[li]
                        let mut edit_page = |page: &mut serde_json::Value, pi: usize| {
                            let list = page.get_mut("list").and_then(|x| x.as_array_mut());
                            if let Some(list) = list {
                                for (li, cmd) in list.iter_mut().enumerate() {
                                    let code = cmd.get("code").and_then(|x| x.as_i64()).unwrap_or(0);
                                    for (key, val) in pairs {
                                        let parts: Vec<&str> = key.split('|').collect();
                                        if parts.len() < 4 || parts[1].parse::<i64>().unwrap_or(-2) != eid {
                                            continue;
                                        }
                                        if parts[2].parse::<usize>().unwrap_or(999) != pi
                                            || parts[3].parse::<usize>().unwrap_or(999) != li
                                        {
                                            continue;
                                        }
                                        match code {
                                            401 if parts.len() == 4 => {
                                                cmd.as_object_mut().unwrap()["parameters"][0] = serde_json::json!(val);
                                                done += 1;
                                            }
                                            405 if parts.len() == 5 && parts[4] == "s" => {
                                                cmd.as_object_mut().unwrap()["parameters"][0] = serde_json::json!(val);
                                                done += 1;
                                            }
                                            102 if parts.len() == 5 && parts[4].starts_with('c') => {
                                                if let Ok(ci) = parts[4][1..].parse::<usize>() {
                                                    cmd.as_object_mut().unwrap()["parameters"][0][ci] = serde_json::json!(val);
                                                    done += 1;
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        };
                        if ev.get("pages").is_some() {
                            for (pi, _) in pages.iter().enumerate() {
                                if let Some(p) = ev.get_mut("pages").and_then(|x| x.get_mut(pi)) {
                                    edit_page(p, pi);
                                }
                            }
                        } else if ev.get("list").is_some() {
                            edit_page(ev, 0);
                        }
                    }
                }
            }
            let json = serde_json::to_string_pretty(&v).unwrap_or_default();
            let dst = out_dir.join(fname);
            if let Err(e) = fs::write(&dst, json) {
                write_failed += 1;
                crate::diag::log("WARN", &format!("回写失败 {}: {e}", dst.display()));
            }
        }
        let msg = crate::engines::with_fail_note(format!("回填 {done} 条 → {}（重命名为 data 前请备份原目录）", out_dir.display()), write_failed);
        OpOutcome::okn(msg, done)
    }
    fn save(&self, ctx: &Ctx) -> OpOutcome {
        let sdirs: Vec<PathBuf> = [ctx.root.join("www").join("save"), ctx.root.join("save")]
            .into_iter()
            .filter(|p| p.is_dir())
            .collect();
        let mut files = Vec::new();
        for d in &sdirs {
            files.extend(crate::features::saves::list_files(d, &["rpgsave", "rmmzsave", "rmzsave"]));
        }
        if files.is_empty() {
            return OpOutcome::fail("未找到 save/ 存档文件");
        }
        let out_dir = ctx
            .opt("out_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| sdirs[0].join("_stool_json"));
        let _ = fs::create_dir_all(&out_dir);
        let decode = ctx.opt("action") != Some("encode");
        let mut done = 0usize;
        for p in &files {
            let raw = match fs::read(p) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let result = if p.extension().map(|e| e == "rmzsave").unwrap_or(false) {
                rpgmmv::mz_save_decode(&raw)
            } else {
                rpgmmv::mv_save_decode(&raw)
            };
            let text = match result {
                Ok(t) => t,
                Err(_) => continue,
            };
            let pretty: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if decode {
                let dst = out_dir.join(format!("{}.json", p.file_stem().unwrap_or_default().to_string_lossy()));
                if fs::write(&dst, serde_json::to_string_pretty(&pretty).unwrap_or_default()).is_ok() {
                    done += 1;
                }
            } else {
                let src = out_dir.join(format!("{}.json", p.file_stem().unwrap_or_default().to_string_lossy()));
                if let Ok(edited) = fs::read_to_string(&src) {
                    let bak = p.with_extension(format!("{}.stool.bak", p.extension().unwrap_or_default().to_string_lossy()));
                    let _ = fs::copy(p, &bak);
                    let payload = if p.extension().map(|e| e == "rmzsave").unwrap_or(false) {
                        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                        use std::io::Write;
                        z.write_all(edited.as_bytes()).ok();
                        z.finish().unwrap_or_default()
                    } else {
                        edited.into_bytes()
                    };
                    if fs::write(p, payload).is_ok() {
                        done += 1;
                    }
                }
            }
        }
        if decode {
            OpOutcome::okn(format!("解码 {done} 个存档 → {}", out_dir.display()), done)
        } else {
            OpOutcome::okn(format!("回写 {done} 个存档（原文件已备份 .stool.bak）"), done)
        }
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

// ---------------- RPG Maker XP/VX/Ace (RGSS) ----------------

pub struct RpgMakerRgssPlugin;

impl Engine for RpgMakerRgssPlugin {
    fn id(&self) -> &'static str {
        "rpgmaker_rgss"
    }
    fn name(&self) -> &'static str {
        "RPG Maker XP / VX / VX Ace"
    }
    fn priority(&self) -> i32 {
        80
    }
    fn detect_scan(&self, scan: &ScanCtx) -> Detection {
        let mut d = Detection::new(self.id(), self.name());
        for (ext, k) in [("rgssad", "RGSS1"), ("rgss2a", "RGSS1"), ("rgss3a", "RGSS3")] {
            if scan.has_ext(ext) {
                d.hit(70, format!("{}（.{ext} 封包）", scan.first_ext_name(ext)));
                d.notes = k.into();
            }
        }
        for name in ["Data", "Graphics", "Audio"] {
            if scan.has_root_dir(name) {
                d.hit(10, format!("{name}/ 目录"));
            }
        }
        // 已解包（无 .rgss3a）时：Data/*.rxdata|rvdata|rvdata2 是 RMXP/VX/Ace 的独有数据，
        // 单独就该过线，不能再像旧版只给 15 分导致"解包后反而识别不出"。
        let scripts = scan.ext_sum(&["rxdata", "rvdata", "rvdata2"]);
        if scripts > 0 {
            d.hit(50, format!("{scripts} 个 .rxdata/.rvdata2 游戏数据（已解包）"));
        }
        d
    }
    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract, Op::Repack, Op::Decompile, Op::TextExtract, Op::Save]
    }
    fn describe(&self, root: &Path) -> String {
        format!("封包: {:?}", list_files_by_ext(root, &["rgssad", "rgss2a", "rgss3a"]).iter().map(|p| file_name(p)).collect::<Vec<_>>())
    }
    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        let mut archives = Vec::new();
        for ext in ["rgssad", "rgss2a", "rgss3a"] {
            archives.extend(list_files_by_ext(ctx.root, &[ext]));
        }
        if archives.is_empty() {
            return OpOutcome::fail("未找到 RGSS 封包");
        }
        let mut done = 0usize;
        let mut failed = 0usize;
        let mut resume = crate::engines::Resume::open(ctx.out_dir, "extract", ctx.root).configured(ctx);
        for (i, arc) in archives.iter().enumerate() {
            if ctx.cancelled() {
                resume.flush();
                return OpOutcome::fail("已取消");
            }
            let mut src = match Source::open(arc, source::MAX_ARCHIVE) {
                Ok(s) => s,
                Err(e) => {
                    resume.flush();
                    return OpOutcome::fail(e);
                }
            };
            let is_v3 = arc.extension().map(|e| e == "rgss3a").unwrap_or(false);
            let workers = crate::engines::worker_count(ctx);
            if is_v3 {
                let entries = match rgss::parse_index_v3(&mut src) {
                    Ok(e) => e,
                    Err(er) => {
                        resume.flush();
                        return OpOutcome::fail(er);
                    }
                };
                let mut jobs: Vec<crate::engines::Job<&rgss::V3Entry>> = Vec::new();
                for e in &entries {
                    if !resume.already_done(ctx.out_dir, &e.name) {
                        jobs.push(crate::engines::Job { rel: e.name.clone(), item: e });
                    }
                }
                let skipped_here = entries.len() - jobs.len();
                let (w, f) = crate::engines::parallel_extract(
                    &jobs,
                    ctx.out_dir,
                    workers,
                    ctx,
                    || Source::open(arc, source::MAX_ARCHIVE),
                    |s, e| rgss::read_entry_v3(s, e.offset, e.size, e.filekey),
                    &mut resume,
                );
                done += w + skipped_here;
                failed += f;
            } else {
                let entries = match rgss::parse_index_v1(&mut src) {
                    Ok(e) => e,
                    Err(er) => {
                        resume.flush();
                        return OpOutcome::fail(er);
                    }
                };
                let mut jobs: Vec<crate::engines::Job<(u64, u32, u32)>> = Vec::new();
                for (name, (off, size, key)) in &entries {
                    if !resume.already_done(ctx.out_dir, name) {
                        jobs.push(crate::engines::Job { rel: name.clone(), item: (*off, *size, *key) });
                    }
                }
                let skipped_here = entries.len() - jobs.len();
                let (w, f) = crate::engines::parallel_extract(
                    &jobs,
                    ctx.out_dir,
                    workers,
                    ctx,
                    || Source::open(arc, source::MAX_ARCHIVE),
                    |s, (off, size, key)| rgss::read_entry_v1(s, *off, *size, *key),
                    &mut resume,
                );
                done += w + skipped_here;
                failed += f;
            }
            ctx.report(i as f32 / archives.len() as f32, &arc.display().to_string());
        }
        let skipped = resume.finish(failed == 0);
        let msg = crate::engines::with_fail_note(format!("解包 {done} 个文件 → {}", ctx.out_dir.display()), failed);
        let msg = crate::engines::with_resume_note(msg, skipped);
        OpOutcome::okn(msg, done)
    }
    fn repack(&self, ctx: &Ctx, src_dir: &Path) -> OpOutcome {
        // 找到游戏根目录下的封包（rgss3a=v3，rgssad/rgss2a=v1）
        let arc = ["rgss3a", "rgssad", "rgss2a"]
            .iter()
            .flat_map(|e| list_files_by_ext(ctx.root, &[e]))
            .next();
        let arc = match arc {
            Some(a) => a,
            None => return OpOutcome::fail("未找到 .rgssad/.rgss2a/.rgss3a 封包"),
        };
        let ext = arc.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
        let v3 = ext == "rgss3a";
        // 收集 src_dir 下所有文件（相对路径，分隔符转 '\'）
        let mut files = std::collections::BTreeMap::new();
        for entry in walkdir::WalkDir::new(src_dir).sort_by_file_name() {
            let p = match entry {
                Ok(p) => p,
                Err(e) => return OpOutcome::fail(e.to_string()),
            };
            if p.file_type().is_file() {
                let rel = match p.path().strip_prefix(src_dir) {
                    Ok(r) => r,
                    Err(e) => return OpOutcome::fail(e.to_string()),
                };
                let name = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("\\");
                files.insert(name, p.path().to_path_buf());
            }
        }
        if files.is_empty() {
            return OpOutcome::fail(format!("{} 为空，没有可封包的文件", src_dir.display()));
        }
        // 备份原封包：已存在则保留（绝不覆盖），失败则中止——宁可不动，也不能无备份改写
        let bak = match crate::settings::backup_or_abort(&arc) {
            Ok(b) => b,
            Err(e) => return OpOutcome::fail(e),
        };
        let r = if v3 {
            rgss::write_v3_paths(&arc, &files)
        } else {
            rgss::write_v1_paths(&arc, &files)
        };
        match r {
            Ok(_) => OpOutcome::okn(format!(
                "已重新封包 {} 个文件 → {}（原封包备份为 {}）",
                files.len(),
                arc.display(),
                file_name(&bak)
            ), files.len()),
            Err(e) => OpOutcome::fail(e),
        }
    }
    fn decompile(&self, ctx: &Ctx) -> OpOutcome {
        let scripts = list_files_by_ext(&ctx.root.join("Data"), &["rxdata", "rvdata", "rvdata2"])
            .into_iter()
            .find(|p| file_name(p).starts_with("Scripts"));
        let scripts = match scripts {
            Some(p) => p,
            None => return OpOutcome::fail("未找到 Data/Scripts.r*data*"),
        };
        let data = fs::read(&scripts).unwrap_or_default();
        match marshal::extract_scripts(&data) {
            Ok(codes) => {
                let out = ctx.out_dir.join("scripts");
                let _ = fs::create_dir_all(&out);
                let mut failed = 0usize;
                for (i, raw) in codes.iter().enumerate() {
                    let p = out.join(format!("{i:03}.rb"));
                    if let Err(e) = fs::write(&p, raw) {
                        failed += 1;
                        crate::diag::log("WARN", &format!("写出失败 {}: {e}", p.display()));
                    }
                }
                let n = codes.len();
                let msg = crate::engines::with_fail_note(format!("提取 {n} 个 Ruby 脚本 → {}", out.display()), failed);
                OpOutcome::okn(msg, n)
            }
            Err(e) => OpOutcome::fail(format!("Ruby Marshal 解析失败: {e}")),
        }
    }
    fn text_extract(&self, ctx: &Ctx, out_csv: &Path) -> OpOutcome {
        // 优先：解析 Data/Map*.r*data + CommonEvents 的 Marshal 结构，
        // 抽取事件指令 code=401(对白)/102(选项)/405(滚动文字) 里的字符串
        let data_dir = ctx.root.join("Data");
        let maps = list_files_by_ext(&data_dir, &["rvdata2", "rvdata", "rxdata"])
            .into_iter()
            .filter(|p| {
                let n = file_name(p);
                (n.starts_with("Map") && !n.starts_with("MapInfos")) || n.starts_with("CommonEvents")
            })
            .collect::<Vec<_>>();
        if !maps.is_empty() {
            let mut rows: Vec<[String; 4]> = Vec::new();
            for (i, f) in maps.iter().enumerate() {
                let data = fs::read(f).unwrap_or_default();
                match marshal::load(&data) {
                    Ok(rb) => {
                        let json = marshal::rb_to_json(&rb);
                        rgss_walk_commands(&json, &file_name(f), 0, &mut rows);
                    }
                    Err(e) => ctx.report(0.0, &format!("{} 跳过: {e}", file_name(f))),
                }
                ctx.report((i + 1) as f32 / maps.len().max(1) as f32, &file_name(f));
            }
            let n = rows.len();
            if n > 0 {
                return match crate::features::text::write_csv(out_csv, &rows) {
                    Ok(()) => OpOutcome::okn(format!(
                        "从 {} 个数据文件提取 {n} 条文本（事件对白/选项）→ {}",
                        maps.len(),
                        out_csv.display()
                    ), n),
                    Err(e) => OpOutcome::fail(e),
                };
            }
        }
        // 回退：扫描已提取的 Ruby 脚本
        let base = ctx
            .opt("rb_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.root.join("scripts"));
        if !base.is_dir() {
            return OpOutcome::fail("未找到 Data/Map*.r*data，且 scripts/ 目录不存在（请先解包或反编译）");
        }
        let mut rows: Vec<[String; 4]> = Vec::new();
        let mut files = list_files_by_ext(&base, &["rb"]);
        let total = files.len().max(1);
        for (i, p) in files.drain(..).enumerate() {
            if let Ok(text) = fs::read_to_string(&p) {
                for (ln, line) in text.lines().enumerate() {
                    if has_cjk(line) {
                        rows.push([format!("{}:{}", file_name(&p), ln + 1), "ruby".into(), trim_to(line, 200), String::new()]);
                    }
                }
            }
            ctx.report((i + 1) as f32 / total as f32, &p.display().to_string());
        }
        let n = rows.len();
        match crate::features::text::write_csv(out_csv, &rows) {
            Ok(()) => OpOutcome::okn(format!("提取 {n} 条含 CJK 文本 → {}", out_csv.display()), n),
            Err(e) => OpOutcome::fail(e),
        }
    }
    fn save(&self, ctx: &Ctx) -> OpOutcome {
        let saves = list_files_by_ext(ctx.root, &["rxdata", "rvdata", "rvdata2"])
            .into_iter()
            .filter(|p| file_name(p).starts_with("Save"))
            .collect::<Vec<_>>();
        if saves.is_empty() {
            return OpOutcome::fail("未找到 SaveNN.rxdata/rvdata 存档");
        }
        let detail: Vec<String> = saves.iter().map(|p| p.display().to_string()).collect();
        OpOutcome { success: true, message: format!("发现 {} 个 Marshal 存档", saves.len()), files_done: saves.len(), logs: detail }
    }
}

pub fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        let cp = c as u32;
        (0x3400..=0x9FFF).contains(&cp) || (0xF900..=0xFAFF).contains(&cp) || (0x3040..=0x30FF).contains(&cp)
    })
}

/// 递归遍历 Marshal→JSON 后的事件数据，抽取事件指令 401/102/405 中的文本。
fn rgss_walk_commands(v: &serde_json::Value, file: &str, depth: usize, rows: &mut Vec<[String; 4]>) {
    if depth > 24 {
        return;
    }
    match v {
        serde_json::Value::Array(arr) => {
            // 事件指令结构：[code, indent, parameters...]
            if let Some(serde_json::Value::Number(code)) = arr.first() {
                let code = code.as_i64().unwrap_or(0);
                if code == 401 || code == 102 || code == 405 {
                    if let Some(serde_json::Value::Array(params)) = arr.get(2) {
                        match code {
                            401 | 405 => {
                                if let Some(serde_json::Value::String(s)) = params.first() {
                                    if !s.is_empty() {
                                        rows.push([
                                            format!("{file}|{:016x}|{code}", fxhash_of(v)),
                                            if code == 401 { "对白" } else { "滚动" }.into(),
                                            trim_to(s, 500),
                                            String::new(),
                                        ]);
                                    }
                                }
                            }
                            102 => {
                                if let Some(serde_json::Value::Array(choices)) = params.first() {
                                    for (ci, c) in choices.iter().enumerate() {
                                        if let serde_json::Value::String(s) = c {
                                            if !s.is_empty() {
                                                rows.push([
                                                    format!("{file}|{:016x}|{code}|c{ci}", fxhash_of(v)),
                                                    "选项".into(),
                                                    trim_to(s, 500),
                                                    String::new(),
                                                ]);
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            for item in arr {
                rgss_walk_commands(item, file, depth + 1, rows);
            }
        }
        serde_json::Value::Object(map) => {
            for val in map.values() {
                rgss_walk_commands(val, file, depth + 1, rows);
            }
        }
        _ => {}
    }
}

/// 简单 FNV-1a 哈希：给没有稳定编号的文本行生成可定位的 id。
pub fn fxhash_of(v: &serde_json::Value) -> u64 {
    let s = v.to_string();
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 递归收集 src_dir 下的所有文件（相对路径 -> 绝对路径）。
pub fn collect_src_files(src_dir: &Path) -> Result<std::collections::BTreeMap<String, std::path::PathBuf>, String> {
    let mut files = std::collections::BTreeMap::new();
    for entry in walkdir::WalkDir::new(src_dir).sort_by_file_name() {
        let p = entry.map_err(|e| e.to_string())?;
        if p.file_type().is_file() {
            let rel = p.path().strip_prefix(src_dir).map_err(|e| e.to_string())?;
            let name = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            files.insert(name, p.path().to_path_buf());
        }
    }
    Ok(files)
}

fn trim_to(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ---------------- KiriKiri ----------------

pub struct KirikiriPlugin;

impl Engine for KirikiriPlugin {
    fn id(&self) -> &'static str {
        "kirikiri"
    }
    fn name(&self) -> &'static str {
        "KiriKiri / KAG"
    }
    fn priority(&self) -> i32 {
        75
    }
    fn detect_scan(&self, scan: &ScanCtx) -> Detection {
        let mut d = Detection::new(self.id(), self.name());
        if scan.has_ext("xp3") {
            d.hit(70, format!("{} 个 .xp3 封包", scan.ext_count("xp3")));
        }
        if scan.any_exe_contains("krkr") {
            d.hit(20, "krkr*.exe 运行时");
        }
        // .ks 与 TyranoScript 同名，用 tyrano/ 目录把两者区分开，避免互相误判。
        let is_tyrano = scan.has_root_dir("tyrano");
        let tjs = scan.ext_count("tjs");
        let ks = scan.ext_count("ks");
        if is_tyrano {
            if tjs + ks > 0 {
                d.note("目录里有 tyrano/，更像是 TyranoScript 而非 KiriKiri（见 TyranoBuilder 识别结果）");
            }
        } else {
            // 未打包 / 已解包：明文 .tjs（引擎脚本）+ .ks（KAG 剧情）可见。
            // 旧版只给 15 分，导致"解包后 / 未打包的 KiriKiri"识别不出来，这里补强。
            if tjs + ks > 0 {
                let pts = if tjs + ks >= 5 { 45 } else { 20 };
                d.hit(pts, format!("{tjs} 个 .tjs + {ks} 个 .ks 明文脚本（未打包）"));
            }
            if scan.has_file_named("config.tjs") || scan.has_file_named("startup.tjs") {
                d.hit(25, "Config.tjs / Startup.tjs 引擎配置");
            }
        }
        d
    }
    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract, Op::Repack, Op::TextExtract]
    }
    fn describe(&self, root: &Path) -> String {
        format!("封包: {:?}", list_files_by_ext(root, &["xp3"]).iter().map(|p| file_name(p)).collect::<Vec<_>>())
    }
    fn repack(&self, ctx: &Ctx, src_dir: &Path) -> OpOutcome {
        // 对每个 xp3：若 src_dir/<封包名>/ 存在则从该目录重建该封包
        let xp3s = list_files_by_ext(ctx.root, &["xp3"]);
        if xp3s.is_empty() {
            return OpOutcome::fail("未找到 .xp3 封包");
        }
        let mut rebuilt = 0usize;
        let mut msgs: Vec<String> = Vec::new();
        for arc in &xp3s {
            let stem = arc.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let sub = src_dir.join(&stem);
            if !sub.is_dir() {
                continue;
            }
            let files = match collect_src_files(&sub) {
                Ok(f) => f,
                Err(e) => return OpOutcome::fail(e),
            };
            if files.is_empty() {
                continue;
            }
            // 备份已存在则保留（绝不覆盖）；备份失败就跳过这个封包，而不是无备份改写
            if let Err(e) = crate::settings::backup_or_abort(arc) {
                return OpOutcome::fail(format!("{}: {e}", file_name(arc)));
            }
            // 写出时沿用原封包的头部变体（V1 老游戏套 V2 头可能load不了），
            // 并且绝不能沿用「加密标志」——我们写的是明文内容。
            let ver = xp3::version_of(arc).unwrap_or(xp3::Xp3Version::V2);
            match xp3::write_paths_v(arc, &files, ver) {
                Ok(_) => {
                    rebuilt += 1;
                    msgs.push(format!("{}（{} 文件）", file_name(arc), files.len()));
                }
                Err(e) => return OpOutcome::fail(format!("{} 写出失败: {e}", file_name(arc))),
            }
        }
        if rebuilt == 0 {
            return OpOutcome::fail(format!(
                "未找到可回写的目录（{} 下应存在与 .xp3 同名的子目录，如 data.xp3 → {}/data/）",
                src_dir.display(),
                src_dir.display()
            ));
        }
        OpOutcome::okn(format!("已重建 {rebuilt} 个 xp3 封包（原文件均备份为 *.stool.bak）：{}", msgs.join("、")), rebuilt)
    }
    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        let xp3s: Vec<PathBuf> = list_files_by_ext(ctx.root, &["xp3"]).into_iter().take(20).collect();
        if xp3s.is_empty() {
            return OpOutcome::fail("未找到 .xp3");
        }
        let mut done = 0usize;
        let mut write_failed = 0usize;
        let mut failed = Vec::new();
        let mut encrypted: Vec<String> = Vec::new();
        let mut resume = crate::engines::Resume::open(ctx.out_dir, "extract", ctx.root).configured(ctx);
        for (i, arc) in xp3s.iter().enumerate() {
            if ctx.cancelled() {
                resume.flush();
                return OpOutcome::fail("已取消");
            }
            let mut src = match Source::open(arc, source::MAX_ARCHIVE) {
                Ok(s) => s,
                Err(e) => {
                    failed.push(format!("{}: {e}", file_name(arc)));
                    continue;
                }
            };
            let files = match xp3::parse_index(&mut src) {
                Ok(f) => f,
                Err(e) => {
                    failed.push(format!("{}: {e}", file_name(arc)));
                    continue;
                }
            };
            // 引擎加密的内容我们解不出来（各家 Cx/Hx 方案按游戏定制）。
            // 与其把乱码当成果写盘，不如明确报错并给替代路线。
            let enc = xp3::encrypted_count(&files);
            if enc > 0 {
                encrypted.push(format!(
                    "{}（{enc}/{} 个条目加密）",
                    file_name(arc),
                    files.len()
                ));
                continue;
            }
            let sub = ctx.out_dir.join(arc.file_stem().unwrap_or_default().to_string_lossy().as_ref());
            let mut jobs: Vec<crate::engines::Job<&xp3::Xp3Entry>> = Vec::new();
            for (name, entry) in &files {
                if !resume.already_done(&sub, name) {
                    jobs.push(crate::engines::Job { rel: name.clone(), item: entry });
                }
            }
            let skipped_here = files.len() - jobs.len();
            let (w, f) = crate::engines::parallel_extract(
                &jobs,
                &sub,
                crate::engines::worker_count(ctx),
                ctx,
                || Source::open(arc, source::MAX_ARCHIVE),
                |s, e| xp3::read_entry(s, e),
                &mut resume,
            );
            done += w + skipped_here;
            write_failed += f;
            ctx.report(i as f32 / xp3s.len() as f32, &arc.display().to_string());
        }
        let skipped = resume.finish(write_failed == 0 && failed.is_empty() && encrypted.is_empty());
        // 全部封包都被加密 → 直接失败：这不是“解包失败”，而是本工具不解这类保护，
        // 必须让用户知道原因和替代路线，而不是拿到一堆乱码。
        if done == 0 && !encrypted.is_empty() {
            return OpOutcome::fail(format!(
                "该游戏的 .xp3 内容被 KiriKiri 加密方案保护（{}），STool 不内置解密。\
                 替代做法：用 GARbro / KrrkExtract 等工具按游戏的加密方案解密并导出，\
                 或改用已发布的汉化补丁中的明文脚本；导出成明文目录后，本工具的解包/文本/回写流程都能继续用。",
                encrypted.join("、")
            ));
        }
        let mut msg = format!("解包 {done} 个文件 → {}", ctx.out_dir.display());
        if !failed.is_empty() {
            msg.push_str(&format!("；失败封包（可能自定义加密）: {}", failed.join("; ")));
        }
        if !encrypted.is_empty() {
            msg.push_str(&format!(
                "；跳过加密封包（STool 不解密，请用 GARbro 等导出明文）: {}",
                encrypted.join("; ")
            ));
        }
        let msg = crate::engines::with_fail_note(msg, write_failed);
        let msg = crate::engines::with_resume_note(msg, skipped);
        OpOutcome { success: done > 0, message: msg, files_done: done, logs: vec![] }
    }
    fn text_extract(&self, ctx: &Ctx, out_csv: &Path) -> OpOutcome {
        let ks_files = list_files_by_ext(ctx.root, &["ks"]);
        if ks_files.is_empty() {
            return OpOutcome::fail("未找到 .ks 脚本（请先解包 .xp3）");
        }
        extract_kag_ks(&ks_files, out_csv, ctx, "kag")
    }
}

/// KAG 剧本（KiriKiri 与 TyranoScript 共用 `.ks` 格式）对白提取。
///
/// 规则：`;` 注释、`*` label、`@` 指令行跳过；其余行去掉 `[tag]` 内联标签后
/// 若仍有内容即视为对白。行首/行尾控制符在回填时另处理。
fn extract_kag_ks(files: &[PathBuf], out_csv: &Path, ctx: &Ctx, tag: &str) -> OpOutcome {
    let mut rows: Vec<[String; 4]> = Vec::new();
    let total = files.len().max(1);
    for (i, p) in files.iter().enumerate() {
        let bytes = fs::read(p).unwrap_or_default();
        let text = decode_sjis_or_utf8(&bytes);
        for (ln, line) in text.lines().enumerate() {
            let s = line.trim();
            if s.is_empty() || s.starts_with(';') || s.starts_with('@') || s.starts_with('*') {
                continue;
            }
            let stripped = strip_tags(s);
            if !stripped.is_empty() {
                rows.push([format!("{}:{}", file_name(p), ln + 1), tag.into(), stripped, String::new()]);
            }
        }
        ctx.report((i + 1) as f32 / total as f32, &p.display().to_string());
    }
    let n = rows.len();
    match crate::features::text::write_csv(out_csv, &rows) {
        Ok(()) => OpOutcome::okn(format!("提取 {n} 行对话 → {}", out_csv.display()), n),
        Err(e) => OpOutcome::fail(e),
    }
}

pub fn decode_sjis_or_utf8(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    // Shift-JIS 完整解码（encoding_rs；含半角片假名与双字节汉字）
    let (cow, _, _) = encoding_rs::SHIFT_JIS.decode(bytes);
    cow.into_owned()
}

/// 极简 Shift-JIS 解码：半角片假名直接映射，汉字区做映射表（常用区段），未命中用 U+FFFD。
pub fn sjis_to_string_lossy(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            out.push(b as char);
            i += 1;
        } else if (0xA1..=0xDF).contains(&b) {
            // 半角片假名
            out.push(char::from_u32(0xFF61 + (b - 0xA1) as u32).unwrap_or('\u{FFFD}'));
            i += 1;
        } else {
            out.push('\u{FFFD}');
            i += 1;
        }
    }
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

// ---------------- Godot ----------------

pub struct GodotPlugin;

impl Engine for GodotPlugin {
    fn id(&self) -> &'static str {
        "godot"
    }
    fn name(&self) -> &'static str {
        "Godot"
    }
    fn priority(&self) -> i32 {
        70
    }
    fn detect_scan(&self, scan: &ScanCtx) -> Detection {
        let mut d = Detection::new(self.id(), self.name());
        let pck = scan.has_ext("pck");
        if pck {
            d.hit(70, format!("{} 个 .pck 封包（如 {}）", scan.ext_count("pck"), scan.first_ext_name("pck")));
        }
        // 明文工程（未被 pck 打包）时 project.godot 是 Godot 独有特征，单独过线；
        // 已有 .pck 时只作补充证据，避免重复计分虚高。
        if scan.has_file_named("project.godot") {
            d.hit(if pck { 15 } else { 65 }, "project.godot 工程文件");
        } else if scan.has_dir(".godot") {
            d.hit(25, ".godot/ 导入缓存");
        }
        d
    }
    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract, Op::Repack, Op::Decompile]
    }
    fn describe(&self, root: &Path) -> String {
        format!("封包: {:?}", list_files_by_ext(root, &["pck"]).iter().map(|p| file_name(p)).collect::<Vec<_>>())
    }
    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        let pcks: Vec<PathBuf> = list_files_by_ext(ctx.root, &["pck"]).into_iter().take(10).collect();
        if pcks.is_empty() {
            return OpOutcome::fail("未找到 .pck");
        }
        let mut done = 0usize;
        let mut failed = 0usize;
        let mut resume = crate::engines::Resume::open(ctx.out_dir, "extract", ctx.root).configured(ctx);
        for (i, pckf) in pcks.iter().enumerate() {
            if ctx.cancelled() {
                resume.flush();
                return OpOutcome::fail("已取消");
            }
            let mut src = match Source::open(pckf, source::MAX_ARCHIVE) {
                Ok(s) => s,
                Err(e) => {
                    resume.flush();
                    return OpOutcome::fail(e);
                }
            };
            let entries = match pck::parse_index(&mut src) {
                Ok(e) => e,
                Err(e) => {
                    resume.flush();
                    return OpOutcome::fail(format!("{} 解析失败: {e}", file_name(pckf)));
                }
            };
            let sub = ctx.out_dir.join(pckf.file_stem().unwrap_or_default().to_string_lossy().as_ref());
            let mut jobs: Vec<crate::engines::Job<&pck::PckEntry>> = Vec::new();
            for e in &entries {
                let rel = e.path.trim_start_matches("res://");
                if !resume.already_done(&sub, rel) {
                    jobs.push(crate::engines::Job { rel: rel.to_string(), item: e });
                }
            }
            let skipped_here = entries.len() - jobs.len();
            let (w, f) = crate::engines::parallel_extract(
                &jobs,
                &sub,
                crate::engines::worker_count(ctx),
                ctx,
                || Source::open(pckf, source::MAX_ARCHIVE),
                |s, e| pck::read_entry(s, e),
                &mut resume,
            );
            done += w + skipped_here;
            failed += f;
            ctx.report(i as f32 / pcks.len() as f32, &pckf.display().to_string());
        }
        let skipped = resume.finish(failed == 0);
        let msg = crate::engines::with_fail_note(format!("解包 {done} 个文件 → {}", ctx.out_dir.display()), failed);
        let msg = crate::engines::with_resume_note(msg, skipped);
        OpOutcome::okn(msg, done)
    }
    fn repack(&self, ctx: &Ctx, src_dir: &Path) -> OpOutcome {
        let pckf = match list_files_by_ext(ctx.root, &["pck"]).into_iter().next() {
            Some(p) => p,
            None => return OpOutcome::fail("未找到 .pck 封包"),
        };
        // 加密 pck（v2 目录加密）无法回写，先探测
        let probe = fs::read(&pckf).unwrap_or_default();
        if let Err(e) = pck::parse_bytes(&probe) {
            return OpOutcome::fail(format!("{}：{e}，加密 pck 不支持回写", file_name(&pckf)));
        }
        // 若 src_dir 下存在与 pck 同名的子目录（解包的默认布局），以它为根
        let stem = pckf.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let base = if src_dir.join(&stem).is_dir() { src_dir.join(&stem) } else { src_dir.to_path_buf() };
        let files = match collect_src_files(&base) {
            Ok(f) => f,
            Err(e) => return OpOutcome::fail(e),
        };
        if files.is_empty() {
            return OpOutcome::fail(format!("{} 为空，没有可封包的文件", src_dir.display()));
        }
        // 路径补 res:// 前缀
        let res_files: std::collections::BTreeMap<String, std::path::PathBuf> = files
            .into_iter()
            .map(|(k, v)| (format!("res://{k}"), v))
            .collect();
        let bak = match crate::settings::backup_or_abort(&pckf) {
            Ok(b) => b,
            Err(e) => return OpOutcome::fail(e),
        };
        match pck::write_v1_paths(&pckf, &res_files) {
            Ok(n) => OpOutcome::okn(format!(
                "已重新封包 {n} 个文件 → {}（原封包备份为 {}；注意：回写为 v1 格式，Godot 3.x/未加密 4.x 可用）",
                pckf.display(),
                file_name(&bak)
            ), n),
            Err(e) => OpOutcome::fail(e),
        }
    }
    fn decompile(&self, ctx: &Ctx) -> OpOutcome {
        // 委托 GDRE Tools（可在设置页一键下载）
        let cfg = crate::settings::load();
        let gdre = match crate::settings::external_tool(&cfg, "gdre_tools") {
            Some(p) => p,
            None => return OpOutcome::fail("未配置 GDRE Tools（请到设置页点 ⬇ 下载）"),
        };
        let pckf = match list_files_by_ext(ctx.root, &["pck"]).into_iter().next() {
            Some(p) => p,
            None => return OpOutcome::fail("未找到 .pck 封包"),
        };
        let out = ctx.out_dir.join("gdre_out");
        let _ = fs::create_dir_all(&out);
        let out_s = out.to_string_lossy().into_owned();
        let pck_s = pckf.to_string_lossy().into_owned();
        // 兼容 GDRE 新旧两代命令行参数
        let attempts: [Vec<String>; 2] = [
            vec!["--headless".into(), "--extract".into(), pck_s.clone(), "--output-dir".into(), out_s.clone()],
            vec![format!("--recover={pck_s}"), format!("--output-dir={out_s}")],
        ];
        let mut last_err = String::new();
        for args in &attempts {
            match Command::new(&gdre).args(args).output() {
                Ok(o) if o.status.success() => {
                    return OpOutcome::ok(format!("GDRE Tools 反编译完成 → {}（含 .gd 脚本还原）", out.display()));
                }
                Ok(o) => {
                    last_err = String::from_utf8_lossy(&if o.stderr.is_empty() { o.stdout } else { o.stderr })
                        .lines()
                        .last()
                        .unwrap_or("")
                        .to_string();
                }
                Err(e) => last_err = e.to_string(),
            }
        }
        OpOutcome::fail(format!("GDRE Tools 运行失败: {last_err}"))
    }
}

// ---------------- NScripter ----------------

/// 解密 nscript.dat 并按行拆分，同时返回原文件头的大端密钥（供重新加密）。
fn nscript_lines(root: &Path) -> Result<(Vec<String>, u16), String> {
    let src = root.join("nscript.dat");
    let data = fs::read(&src).map_err(|e| e.to_string())?;
    if data.len() < 3 {
        return Err("nscript.dat 过短".into());
    }
    let key = (((data[0] as u32) << 8) | data[1] as u32) as u16;
    let text = decode_sjis_or_utf8(&nscript::decode(&data));
    Ok((text.lines().map(|s| s.to_string()).collect(), key))
}

pub struct NscripterPlugin;

impl Engine for NscripterPlugin {
    fn id(&self) -> &'static str {
        "nscripter"
    }
    fn name(&self) -> &'static str {
        "NScripter / ONScripter"
    }
    fn priority(&self) -> i32 {
        65
    }
    fn detect_scan(&self, scan: &ScanCtx) -> Detection {
        let mut d = Detection::new(self.id(), self.name());
        if scan.has_file_named("nscript.dat") {
            d.hit(70, "nscript.dat 加密脚本");
        }
        if scan.has_ext("nsa") {
            d.hit(20, format!("{} 个 .nsa 资源包", scan.ext_count("nsa")));
        }
        // .sar 是 NScripter/ONScripter 的归档变体，旧版漏判，这里补上
        if scan.has_ext("sar") {
            d.hit(20, format!("{} 个 .sar 资源包", scan.ext_count("sar")));
        }
        if scan.any_exe_contains("ons") {
            d.hit(15, "ONScripter 运行时");
        }
        d
    }
    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract, Op::TextExtract, Op::TextImport]
    }
    fn describe(&self, root: &Path) -> String {
        if root.join("nscript.dat").exists() { "nscript.dat".into() } else { String::new() }
    }
    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        let src = ctx.root.join("nscript.dat");
        if !src.exists() {
            return OpOutcome::fail("未找到 nscript.dat");
        }
        let data = fs::read(&src).unwrap_or_default();
        let out = ctx.out_dir.join("nscript.txt");
        if let Err(e) = fs::write(&out, nscript::decode(&data)) {
            return OpOutcome::fail(format!("写出 {} 失败: {e}", out.display()));
        }
        OpOutcome::ok(format!("脚本已解密 → {}（若乱码说明是特殊加密变体）", out.display()))
    }
    /// NScripter 脚本的对白行：行首为 Shift-JIS 双字节字符（>=0x80）即视为文本行，
    /// 命令行（bg/ld/print/&/*…）以 ASCII 开头，天然区分。行尾 `\` `@` 为翻页控制符。
    fn text_extract(&self, ctx: &Ctx, out_csv: &Path) -> OpOutcome {
        let (lines, _) = match nscript_lines(ctx.root) {
            Ok(v) => v,
            Err(e) => return OpOutcome::fail(format!("读取 nscript.dat 失败: {e}")),
        };
        let mut rows: Vec<[String; 4]> = Vec::new();
        for (ln, line) in lines.iter().enumerate() {
            let first = line.as_bytes().first().copied().unwrap_or(0);
            if first < 0x80 || !has_cjk(line) {
                continue; // 命令行 / 注释 / 空行 / 纯 ASCII
            }
            // 去掉行尾翻页控制符再入库，回填时自动补回
            let core = line.trim_end_matches(['\\', '@']).trim_end();
            if core.is_empty() {
                continue;
            }
            rows.push([format!("nscript.dat:{ln}"), "nscript".into(), trim_to(core, 500), String::new()]);
        }
        let n = rows.len();
        if n == 0 {
            return OpOutcome::fail("未从 nscript.dat 提取到对白行（可能是特殊加密变体或非 CJK 文本）");
        }
        match crate::features::text::write_csv(out_csv, &rows) {
            Ok(()) => OpOutcome::okn(format!("提取 {n} 行对白 → {}（回填时自动重新加密）", out_csv.display()), n),
            Err(e) => OpOutcome::fail(e),
        }
    }
    fn text_import(&self, ctx: &Ctx, csv_path: &Path) -> OpOutcome {
        let (mut lines, key) = match nscript_lines(ctx.root) {
            Ok(v) => v,
            Err(e) => return OpOutcome::fail(format!("读取 nscript.dat 失败: {e}")),
        };
        let rows = match crate::features::text::read_csv(csv_path) {
            Ok(r) => r,
            Err(e) => return OpOutcome::fail(e),
        };
        let mut applied = 0usize;
        let mut unencodable = 0usize;
        for row in &rows {
            // row: id, file, context, source, translation
            let loc = &row[0];
            let trans = row[4].trim();
            if trans.is_empty() || !loc.starts_with("nscript.dat:") {
                continue;
            }
            let ln: usize = match loc.rsplit(':').next().and_then(|s| s.parse().ok()) {
                Some(v) => v,
                None => continue,
            };
            if ln >= lines.len() {
                continue;
            }
            let orig = &lines[ln];
            let tail = if orig.ends_with('\\') { "\\" } else if orig.ends_with('@') { "@" } else { "" };
            let new_line = format!("{trans}{tail}");
            // Shift-JIS 可编码性检查（只计数，不改写 lines —— 避免 SJIS→UTF8 乱码）
            let (_, _, had_errors) = encoding_rs::SHIFT_JIS.encode(&new_line);
            if had_errors {
                unencodable += 1;
                continue;
            }
            lines[ln] = new_line;
            applied += 1;
        }
        if applied == 0 {
            return OpOutcome::fail("没有可回填的译文（translation 列为空，或全部含 Shift-JIS 之外的字符）");
        }
        // 重新加密写回（标准变体：正文第 i 字节 XOR ((key+i)&0xFF)，前缀 2 字节大端密钥）
        let body = lines.join("\r\n") + "\r\n";
        let (enc_bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(&body);
        if had_errors {
            return OpOutcome::fail("合并后的脚本含 Shift-JIS 无法表示的字符，已中止");
        }
        let mut out = Vec::with_capacity(enc_bytes.len() + 2);
        out.push((key >> 8) as u8);
        out.push((key & 0xFF) as u8);
        for (i, b) in enc_bytes.iter().enumerate() {
            out.push(b ^ ((key as u32 + i as u32) & 0xFF) as u8);
        }
        let src = ctx.root.join("nscript.dat");
        if let Err(e) = crate::settings::backup_or_abort(&src) {
            return OpOutcome::fail(e);
        }
        if let Err(e) = fs::write(&src, out) {
            return OpOutcome::fail(format!("写回 nscript.dat 失败: {e}"));
        }
        let mut msg = format!("已回填 {applied} 行并重新加密 → nscript.dat（原文件备份为 nscript.dat.stool.bak）");
        if unencodable > 0 {
            msg.push_str(&format!("；{unencodable} 行译文含 Shift-JIS 之外字符已跳过"));
        }
        OpOutcome::okn(msg, applied)
    }
}

// ---------------- HTML / Electron ----------------

pub struct HtmlGamePlugin;

impl Engine for HtmlGamePlugin {
    fn id(&self) -> &'static str {
        "html_game"
    }
    fn name(&self) -> &'static str {
        "HTML / JS / Electron"
    }
    fn priority(&self) -> i32 {
        55
    }
    fn detect_scan(&self, scan: &ScanCtx) -> Detection {
        let mut d = Detection::new(self.id(), self.name());
        if scan.has_ext("asar") {
            d.hit(70, format!("{} 个 app.asar（Electron 封包）", scan.ext_count("asar")));
        }
        // TyranoBuilder 与 RPG Maker MV 根目录都带 index.html，但它们各有更专门的插件；
        // 这里排除掉，避免同一个目录同时被"确认"成三种 HTML 系引擎。
        let owned_by_tyrano = scan.has_root_dir("tyrano");
        let owned_by_mv = scan.has_file_named("actors.json");
        if scan.has_root_file("index.html") && !owned_by_tyrano && !owned_by_mv {
            d.hit(60, "index.html Web 入口");
            if scan.has_dir("assets") {
                d.hit(15, "assets/ 资源目录");
            }
            if scan.has_dir("js") {
                d.hit(10, "js/ 脚本目录");
            }
        }
        if owned_by_tyrano {
            d.note("检测到 tyrano/ 目录，判定交由 TyranoBuilder 插件");
        }
        d
    }
    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract, Op::Repack, Op::TextInject]
    }

    /// JSON 注入汉化：入口 HTML 挂脚本标签 + DOM 文本节点运行时替换（Canvas 除外）。
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
        if !list_files_by_ext(root, &["asar"]).is_empty() { "Electron asar".into() } else { "明文前端代码".into() }
    }
    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        let asars: Vec<PathBuf> = list_files_by_ext(ctx.root, &["asar"]).into_iter().take(5).collect();
        if asars.is_empty() {
            return OpOutcome::fail("未找到 asar（明文 HTML 游戏无需解包，直接编辑）");
        }
        let mut done = 0usize;
        let mut write_failed = 0usize;
        let mut resume = crate::engines::Resume::open(ctx.out_dir, "extract", ctx.root).configured(ctx);
        for (i, arc) in asars.iter().enumerate() {
            if ctx.cancelled() {
                resume.flush();
                return OpOutcome::fail("已取消");
            }
            let mut src = match Source::open(arc, source::MAX_ARCHIVE) {
                Ok(s) => s,
                Err(e) => {
                    resume.flush();
                    return OpOutcome::fail(e);
                }
            };
            let (files, data_start) = match asar::parse_index(&mut src) {
                Ok(f) => f,
                Err(e) => {
                    resume.flush();
                    return OpOutcome::fail(format!("{} 解析失败: {e}", file_name(arc)));
                }
            };
            let sub = ctx.out_dir.join(arc.file_stem().unwrap_or_default().to_string_lossy().as_ref());
            let mut jobs: Vec<crate::engines::Job<&asar::AsarNode>> = Vec::new();
            let mut skipped_here = 0usize;
            for (name, entry) in &files {
                // unpacked 外置条目：不写出、不计数（与旧串行逻辑一致）
                if entry.unpacked {
                    continue;
                }
                if resume.already_done(&sub, name) {
                    skipped_here += 1;
                    continue;
                }
                jobs.push(crate::engines::Job { rel: name.clone(), item: entry });
            }
            let (w, f) = crate::engines::parallel_extract(
                &jobs,
                &sub,
                crate::engines::worker_count(ctx),
                ctx,
                || Source::open(arc, source::MAX_ARCHIVE),
                |s, e| asar::read_entry(s, data_start, e),
                &mut resume,
            );
            done += w + skipped_here;
            write_failed += f;
            ctx.report(i as f32 / asars.len() as f32, &arc.display().to_string());
        }
        let skipped = resume.finish(write_failed == 0);
        let msg = crate::engines::with_fail_note(format!("解包 {done} 个文件 → {}", ctx.out_dir.display()), write_failed);
        let msg = crate::engines::with_resume_note(msg, skipped);
        OpOutcome::okn(msg, done)
    }
    fn repack(&self, ctx: &Ctx, src_dir: &Path) -> OpOutcome {
        let target = ctx
            .opt("out_asar")
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.root.join("resources").join("app.asar"));
        if target.exists() {
            // 备份已存在则保留（绝不覆盖）；备份失败即中止，不能无备份改写
            if let Err(e) = crate::settings::backup_or_abort(&target) {
                return OpOutcome::fail(e);
            }
        }
        match asar::pack(src_dir, &target) {
            Ok(n) => OpOutcome::okn(format!("已打包 {} 个文件 → {}", n, target.display()), n),
            Err(e) => OpOutcome::fail(e),
        }
    }
}

// ---------------- TyranoBuilder / TyranoScript ----------------

/// TyranoBuilder（TyranoScript）游戏。
///
/// 与 KiriKiri 同源（都用 `.tjs`/`.ks`），但目录结构不同：根目录有 `tyrano/` 运行时
/// 目录 + `index.html` 入口，剧本在 `data/scenario/*.ks`，配置在 `data/system/Config.tjs`。
/// 因为它是纯 HTML/JS 前端，文本既可从 `.ks` 提取，也可走 DOM 运行时注入。
pub struct TyranoPlugin;

impl Engine for TyranoPlugin {
    fn id(&self) -> &'static str {
        "tyrano"
    }
    fn name(&self) -> &'static str {
        "TyranoBuilder / TyranoScript"
    }
    fn priority(&self) -> i32 {
        60
    }
    fn detect_scan(&self, scan: &ScanCtx) -> Detection {
        let mut d = Detection::new(self.id(), self.name());
        if scan.has_root_dir("tyrano") {
            d.hit(60, "tyrano/ 引擎运行时目录");
        }
        if scan.has_root_file("index.html") {
            d.hit(25, "index.html 入口页");
        }
        let ks = scan.ext_count("ks");
        if ks > 0 {
            d.hit(15, format!("{ks} 个 .ks 剧本"));
        }
        if scan.has_dir("scenario") {
            d.hit(10, "data/scenario/ 剧本目录");
        }
        if scan.has_file_named("config.tjs") {
            d.hit(10, "data/system/Config.tjs");
        }
        if !scan.has_root_dir("tyrano") {
            d.note("未发现 tyrano/ 目录，可能是 KiriKiri 或其他 .ks/.tjs 引擎，请人工核对");
        }
        d
    }
    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract, Op::TextExtract, Op::TextInject]
    }

    /// 剧本提取：`data/scenario/**/*.ks`（KAG 语法，与 KiriKiri 共用解析器）。
    fn text_extract(&self, ctx: &Ctx, out_csv: &Path) -> OpOutcome {
        let base = [ctx.root.join("data").join("scenario"), ctx.root.join("scenario"), ctx.root.join("data")]
            .into_iter()
            .find(|p| p.is_dir())
            .unwrap_or_else(|| ctx.root.join("data"));
        let files = list_files_by_ext(&base, &["ks"]);
        if files.is_empty() {
            return OpOutcome::fail("未找到 data/scenario/*.ks 剧本（请确认游戏目录是否指到了根目录）");
        }
        extract_kag_ks(&files, out_csv, ctx, "tyrano")
    }

    /// 运行时 JSON 注入（HTML 变体）：挂 DOM 文本替换脚本，不改 `.ks`。
    fn text_inject(&self, ctx: &Ctx, json_path: &Path) -> OpOutcome {
        let json = match json_path.exists() {
            true => json_path.to_path_buf(),
            false => ctx.root.join("translation.json"),
        };
        if !json.exists() {
            return OpOutcome::fail(format!(
                "未找到翻译 JSON（{} 不存在）。格式：{{\"原文\":\"译文\"}}，可从“文本提取”的 CSV 生成",
                json.display()
            ));
        }
        match crate::features::inject::install(ctx.root, &json, self.id()) {
            Ok(m) => OpOutcome::ok(m),
            Err(e) => OpOutcome::fail(e),
        }
    }

    fn describe(&self, root: &Path) -> String {
        let scen = root.join("data").join("scenario");
        let n = list_files_by_ext(&scen, &["ks"]).len();
        format!("剧本: data/scenario/（{n} 个 .ks）；入口: index.html")
    }

    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        // TyranoBuilder 通常不封包，资源直接躺在 data/ 下
        OpOutcome::fail(format!(
            "TyranoBuilder 游戏一般无需解包（{} 下的 data/ 已是明文）；若确有封包请配置 GARbro 后重试",
            ctx.root.display()
        ))
    }
}

#[cfg(test)]
mod detect_tests {
    use super::*;
    use crate::engines::scan::ScanCtx;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_others_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn det(e: &dyn Engine, d: &Path) -> Detection {
        e.detect_scan(&ScanCtx::build(d))
    }

    #[test]
    fn test_rgss_unpacked_is_recognized() {
        // 旧版：解包后只剩 Data/*.rvdata2，最多 45 分 → 漏判。现在应过线。
        let d = tmp("rgss_unpacked");
        fs::create_dir_all(d.join("Data")).unwrap();
        fs::create_dir_all(d.join("Graphics")).unwrap();
        fs::create_dir_all(d.join("Audio")).unwrap();
        fs::write(d.join("Data").join("Map001.rvdata2"), b"x").unwrap();
        let r = det(&RpgMakerRgssPlugin, &d);
        assert!(r.ok(), "解包后的 RGSS 应被识别，实得 {} 分：{:?}", r.score, r.evidence);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn test_kirikiri_unpacked_is_recognized() {
        // 旧版：未打包（无 .xp3）时最多 35 分 → 漏判。现在应过线。
        let d = tmp("krkr_unpacked");
        fs::create_dir_all(d.join("scenario")).unwrap();
        fs::create_dir_all(d.join("system")).unwrap();
        for i in 0..6 {
            fs::write(d.join("scenario").join(format!("s{i}.ks")), b"x").unwrap();
        }
        fs::write(d.join("system").join("Config.tjs"), b"x").unwrap();
        let r = det(&KirikiriPlugin, &d);
        assert!(r.ok(), "未打包的 KiriKiri 应被识别，实得 {} 分：{:?}", r.score, r.evidence);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn test_tyrano_detected_and_not_stolen_by_kirikiri() {
        let d = tmp("tyrano");
        fs::create_dir_all(d.join("tyrano")).unwrap();
        fs::create_dir_all(d.join("data").join("scenario")).unwrap();
        fs::write(d.join("index.html"), b"<html></html>").unwrap();
        for i in 0..3 {
            fs::write(d.join("data").join("scenario").join(format!("s{i}.ks")), b"x").unwrap();
        }
        let tyrano = det(&TyranoPlugin, &d);
        assert!(tyrano.ok(), "TyranoBuilder 应被识别，实得 {} 分", tyrano.score);
        // KiriKiri 不应把 tyrano/ 目录的游戏抢走
        let kiri = det(&KirikiriPlugin, &d);
        assert!(!kiri.ok(), "有 tyrano/ 时不该同时确认成 KiriKiri，实得 {} 分", kiri.score);
        // HTML 插件也应让位
        let html = det(&HtmlGamePlugin, &d);
        assert!(!html.ok(), "有 tyrano/ 时 HTML 插件不该抢，实得 {} 分", html.score);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn test_godot_unpacked_project_file() {
        let d = tmp("godot_proj");
        fs::write(d.join("project.godot"), b"config_version=5").unwrap();
        let r = det(&GodotPlugin, &d);
        assert!(r.ok(), "仅 project.godot 也应识别为 Godot，实得 {} 分", r.score);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn test_html_index_alone_and_mv_not_stolen() {
        let d = tmp("html_plain");
        fs::write(d.join("index.html"), b"<html></html>").unwrap();
        let r = det(&HtmlGamePlugin, &d);
        assert!(r.ok(), "纯 index.html Web 游戏应被识别，实得 {} 分", r.score);
        let _ = fs::remove_dir_all(&d);

        // MV 游戏同样有 index.html，但不该被 HTML 插件抢走
        let m = tmp("mv_html");
        fs::create_dir_all(m.join("www").join("data").join("js")).unwrap();
        fs::write(m.join("www").join("index.html"), b"<html></html>").unwrap();
        fs::write(m.join("www").join("data").join("Actors.json"), b"[]").unwrap();
        let mv = det(&RpgMakerMvPlugin, &m);
        let html = det(&HtmlGamePlugin, &m);
        assert!(mv.ok(), "MV 应被识别，实得 {} 分", mv.score);
        assert!(!html.ok(), "MV 目录不该被 HTML 插件抢走，实得 {} 分", html.score);
        let _ = fs::remove_dir_all(&m);
    }

    #[test]
    fn test_nscripter_and_sar() {
        let d = tmp("nscripter");
        fs::write(d.join("nscript.dat"), b"\x00\x01xxxx").unwrap();
        fs::write(d.join("arc.sar"), b"x").unwrap();
        let r = det(&NscripterPlugin, &d);
        assert!(r.ok(), "nscript.dat 应识别为 NScripter，实得 {} 分", r.score);
        let _ = fs::remove_dir_all(&d);
    }
}
