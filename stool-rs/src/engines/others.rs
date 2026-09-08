//! 其余引擎插件：RPG Maker MV/MZ、RGSS、KiriKiri、Godot、NScripter、HTML/Electron。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{safe_out_path, Ctx, Detection, Engine, Op, OpOutcome};
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

// ---------------- RPG Maker MV / MZ ----------------

pub struct RpgMakerMvPlugin;

impl RpgMakerMvPlugin {
    pub fn data_dir(root: &Path) -> Option<PathBuf> {
        for c in [root.join("www").join("data"), root.join("data")] {
            if c.join("Actors.json").exists() {
                return Some(c);
            }
        }
        None
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
    fn detect(&self, root: &Path) -> Detection {
        let mut score = 0;
        let mut ev = Vec::new();
        if Self::data_dir(root).is_some() {
            score += 60;
            ev.push("data/Actors.json 游戏数据".into());
        }
        let enc = list_files_by_ext(root, &["rpgmvp", "rpgmvo", "rpgmvm"]).len();
        let enc_mz = list_files_by_ext(root, &["png_", "ogg_", "m4a_"]).len();
        if enc + enc_mz > 0 {
            score += 25;
            ev.push(format!("{} 个加密素材", enc + enc_mz));
        }
        if root.join("Game.rpgproject").exists() {
            score += 15;
            ev.push("Game.rpgproject".into());
        }
        if root.join("www").join("js").is_dir() || root.join("js").is_dir() {
            score += 10;
            ev.push("js/ 脚本目录".into());
        }
        Detection { plugin_id: self.id().into(), name: self.name().into(), score, evidence: ev, notes: String::new() }
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
            let _ = fs::write(out_dir.join(fname), serde_json::to_string_pretty(&v).unwrap_or_default());
        }
        OpOutcome::okn(format!("回填 {done} 条 → {}（重命名为 data 前请备份原目录）", out_dir.display()), done)
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
    fn detect(&self, root: &Path) -> Detection {
        let mut score = 0;
        let mut ev = Vec::new();
        let mut kind = String::new();
        for (ext, k) in [("rgssad", "RGSS1"), ("rgss2a", "RGSS1"), ("rgss3a", "RGSS3")] {
            if !list_files_by_ext(root, &[ext]).is_empty() {
                score += 70;
                kind = k.into();
                ev.push(format!("*.{ext} 封包"));
            }
        }
        for d in ["Data", "Graphics", "Audio"] {
            if root.join(d).is_dir() {
                score += 10;
                ev.push(format!("{d}/ 目录"));
            }
        }
        if !list_files_by_ext(&root.join("Data"), &["rxdata", "rvdata", "rvdata2"]).is_empty() {
            score += 15;
            ev.push("Data/Scripts 数据脚本".into());
        }
        Detection { plugin_id: self.id().into(), name: self.name().into(), score, evidence: ev, notes: kind }
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
        for (i, arc) in archives.iter().enumerate() {
            if ctx.cancelled() {
                return OpOutcome::fail("已取消");
            }
            let data = fs::read(arc).unwrap_or_default();
            let is_v3 = arc.extension().map(|e| e == "rgss3a").unwrap_or(false);
            if is_v3 {
                let entries = match rgss::parse_v3(arc) {
                    Ok(e) => e,
                    Err(er) => return OpOutcome::fail(er),
                };
                for (j, e) in entries.iter().enumerate() {
                    let blob = rgss::extract_v3_file(&data, e.offset, e.size, e.filekey);
                    let _ = fs::write(safe_out_path(ctx.out_dir, &e.name), blob);
                    done += 1;
                    ctx.report(j as f32 / entries.len().max(1) as f32, &e.name);
                }
            } else {
                let entries = match rgss::parse_v1(arc) {
                    Ok(e) => e,
                    Err(er) => return OpOutcome::fail(er),
                };
                for (j, (name, (off, size, key))) in entries.iter().enumerate() {
                    let blob = rgss::extract_v1_file(&data, *off, *size, *key);
                    let _ = fs::write(safe_out_path(ctx.out_dir, name), blob);
                    done += 1;
                    ctx.report(j as f32 / entries.len().max(1) as f32, name);
                }
            }
            ctx.report(i as f32 / archives.len() as f32, &arc.display().to_string());
        }
        OpOutcome::okn(format!("解包 {done} 个文件 → {}", ctx.out_dir.display()), done)
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
        // 备份原封包
        let bak = arc.with_extension(format!("{ext}.stool.bak"));
        let _ = fs::copy(&arc, &bak);
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
                for (i, raw) in codes.iter().enumerate() {
                    let _ = fs::write(out.join(format!("{i:03}.rb")), raw);
                }
                let n = codes.len();
                OpOutcome::okn(format!("提取 {n} 个 Ruby 脚本 → {}", out.display()), n)
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
    fn detect(&self, root: &Path) -> Detection {
        let mut score = 0;
        let mut ev = Vec::new();
        let xp3s = list_files_by_ext(root, &["xp3"]);
        if !xp3s.is_empty() {
            score += 70;
            ev.push(format!("{} 个 .xp3 封包", xp3s.len()));
        }
        if fs::read_dir(root).map(|rd| rd.flatten().any(|d| d.file_name().to_string_lossy().to_lowercase().starts_with("krkr") && d.path().extension().map(|e| e == "exe").unwrap_or(false))).unwrap_or(false) {
            score += 20;
            ev.push("krkr*.exe".into());
        }
        if !list_files_by_ext(root, &["tjs", "ks"]).is_empty() {
            score += 15;
            ev.push("明文 .tjs/.ks 脚本".into());
        }
        Detection { plugin_id: self.id().into(), name: self.name().into(), score, evidence: ev, notes: String::new() }
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
            let bak = arc.with_extension("xp3.stool.bak");
            let _ = fs::copy(arc, &bak);
            match xp3::write_paths(arc, &files) {
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
        let mut failed = Vec::new();
        for (i, arc) in xp3s.iter().enumerate() {
            if ctx.cancelled() {
                return OpOutcome::fail("已取消");
            }
            let data = fs::read(arc).unwrap_or_default();
            let files = match xp3::parse_bytes(&data) {
                Ok(f) => f,
                Err(e) => {
                    failed.push(format!("{}: {e}", file_name(arc)));
                    continue;
                }
            };
            let sub = ctx.out_dir.join(arc.file_stem().unwrap_or_default().to_string_lossy().as_ref());
            let total = files.len().max(1);
            for (j, (name, entry)) in files.iter().enumerate() {
                if let Ok(blob) = xp3::read_file(&data, entry) {
                    let _ = fs::write(safe_out_path(&sub, name), blob);
                    done += 1;
                }
                ctx.report(j as f32 / total as f32, name);
            }
            ctx.report(i as f32 / xp3s.len() as f32, &arc.display().to_string());
        }
        let mut msg = format!("解包 {done} 个文件 → {}", ctx.out_dir.display());
        if !failed.is_empty() {
            msg.push_str(&format!("；失败封包（可能自定义加密）: {}", failed.join("; ")));
        }
        OpOutcome { success: done > 0, message: msg, files_done: done, logs: vec![] }
    }
    fn text_extract(&self, ctx: &Ctx, out_csv: &Path) -> OpOutcome {
        let ks_files = list_files_by_ext(ctx.root, &["ks"]);
        if ks_files.is_empty() {
            return OpOutcome::fail("未找到 .ks 脚本（请先解包 .xp3）");
        }
        let mut rows: Vec<[String; 4]> = Vec::new();
        let total = ks_files.len();
        for (i, p) in ks_files.iter().enumerate() {
            let bytes = fs::read(p).unwrap_or_default();
            let text = decode_sjis_or_utf8(&bytes);
            for (ln, line) in text.lines().enumerate() {
                let s = line.trim();
                if s.is_empty() || s.starts_with(';') || s.starts_with('@') || s.starts_with('*') {
                    continue;
                }
                let stripped = strip_tags(s);
                if !stripped.is_empty() {
                    rows.push([format!("{}:{}", file_name(p), ln + 1), "kag".into(), stripped, String::new()]);
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
    fn detect(&self, root: &Path) -> Detection {
        let mut score = 0;
        let mut ev = Vec::new();
        let pcks = list_files_by_ext(root, &["pck"]);
        if !pcks.is_empty() {
            score += 70;
            ev.push(format!("{} 个 .pck", pcks.len()));
        }
        if root.join("project.godot").exists() || root.join(".godot").is_dir() {
            score += 30;
            ev.push("project.godot".into());
        }
        Detection { plugin_id: self.id().into(), name: self.name().into(), score, evidence: ev, notes: String::new() }
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
        for (i, pckf) in pcks.iter().enumerate() {
            let data = fs::read(pckf).unwrap_or_default();
            let entries = match pck::parse_bytes(&data) {
                Ok(e) => e,
                Err(e) => return OpOutcome::fail(format!("{} 解析失败: {e}", file_name(pckf))),
            };
            let sub = ctx.out_dir.join(pckf.file_stem().unwrap_or_default().to_string_lossy().as_ref());
            for (j, e) in entries.iter().enumerate() {
                let rel = e.path.trim_start_matches("res://");
                let _ = fs::write(safe_out_path(&sub, rel), pck::read_file(&data, e));
                done += 1;
                ctx.report(j as f32 / entries.len().max(1) as f32, &e.path);
            }
            ctx.report(i as f32 / pcks.len() as f32, &pckf.display().to_string());
        }
        OpOutcome::okn(format!("解包 {done} 个文件 → {}", ctx.out_dir.display()), done)
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
        let bak = pckf.with_extension("pck.stool.bak");
        let _ = fs::copy(&pckf, &bak);
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
    fn detect(&self, root: &Path) -> Detection {
        let mut score = 0;
        let mut ev = Vec::new();
        if root.join("nscript.dat").exists() {
            score += 70;
            ev.push("nscript.dat 加密脚本".into());
        }
        if !list_files_by_ext(root, &["nsa"]).is_empty() {
            score += 20;
            ev.push("*.nsa 资源包".into());
        }
        if fs::read_dir(root).map(|rd| rd.flatten().any(|d| d.file_name().to_string_lossy().to_lowercase().contains("ons") && d.path().extension().map(|e| e == "exe").unwrap_or(false))).unwrap_or(false) {
            score += 15;
            ev.push("ONScripter 运行时".into());
        }
        Detection { plugin_id: self.id().into(), name: self.name().into(), score, evidence: ev, notes: String::new() }
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
        let _ = fs::write(&out, nscript::decode(&data));
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
        let _ = fs::copy(&src, ctx.root.join("nscript.dat.stool.bak"));
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
    fn detect(&self, root: &Path) -> Detection {
        let mut score = 0;
        let mut ev = Vec::new();
        let asars = list_files_by_ext(root, &["asar"]);
        if !asars.is_empty() {
            score += 70;
            ev.push(format!("{} 个 app.asar", asars.len()));
        }
        if root.join("index.html").exists() {
            score += 50;
            ev.push("index.html".into());
        }
        Detection { plugin_id: self.id().into(), name: self.name().into(), score, evidence: ev, notes: String::new() }
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
        for (i, arc) in asars.iter().enumerate() {
            let data = fs::read(arc).unwrap_or_default();
            let (files, data_start) = match asar::parse_bytes(&data) {
                Ok(f) => f,
                Err(e) => return OpOutcome::fail(format!("{} 解析失败: {e}", file_name(arc))),
            };
            let sub = ctx.out_dir.join(arc.file_stem().unwrap_or_default().to_string_lossy().as_ref());
            for (j, (name, entry)) in files.iter().enumerate() {
                if let Ok(blob) = asar::read_file(&data, data_start, entry) {
                    let _ = fs::write(safe_out_path(&sub, name), blob);
                    done += 1;
                }
                ctx.report(j as f32 / files.len().max(1) as f32, name);
            }
            ctx.report(i as f32 / asars.len() as f32, &arc.display().to_string());
        }
        OpOutcome::okn(format!("解包 {done} 个文件 → {}", ctx.out_dir.display()), done)
    }
    fn repack(&self, ctx: &Ctx, src_dir: &Path) -> OpOutcome {
        let target = ctx
            .opt("out_asar")
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.root.join("resources").join("app.asar"));
        if target.exists() {
            let _ = fs::copy(&target, target.with_extension("asar.stool.bak"));
        }
        match asar::pack(src_dir, &target) {
            Ok(n) => OpOutcome::okn(format!("已打包 {} 个文件 → {}", n, target.display()), n),
            Err(e) => OpOutcome::fail(e),
        }
    }
}
