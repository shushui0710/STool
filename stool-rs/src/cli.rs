//! 命令行入口：detect / extract / repack / decompile / text-extract / text-import / save / unlock / mod-* / gui。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::engines::{Op, Registry};

fn print_progress(frac: f32, msg: &str) {
    eprintln!("[{:>5.1}%] {}", (frac * 100.0).min(100.0), msg);
}

fn run_op(reg: &Registry, det_id: &str, op: Op, root: &Path, out: &Path, opts: HashMap<String, String>) -> crate::engines::OpOutcome {
    let engine = match reg.get(det_id) {
        Some(e) => e,
        None => return crate::engines::OpOutcome::fail("插件不存在"),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let ctx = crate::engines::Ctx {
        root,
        out_dir: out,
        options: &opts,
        progress: &print_progress,
        cancel: &cancel,
    };
    match op {
        Op::Extract => engine.extract(&ctx),
        Op::Repack => {
            let src = PathBuf::from(opts.get("src_dir").map(|s| s.as_str()).unwrap_or("patch_src"));
            engine.repack(&ctx, &src)
        }
        Op::Decompile => engine.decompile(&ctx),
        Op::TextExtract => engine.text_extract(&ctx, out),
        Op::TextImport => engine.text_import(&ctx, out),
        Op::Save => engine.save(&ctx),
        Op::Unlock => engine.unlock(&ctx),
    }
}

fn opts_from(pairs: &[String]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for kv in pairs {
        if let Some((k, v)) = kv.split_once('=') {
            m.insert(k.to_string(), v.to_string());
        } else {
            m.insert(kv.clone(), "1".to_string());
        }
    }
    m
}

/// save-edit 子命令：命令行搜索/修改存档。
/// 用法:
///   stool save-edit <存档文件> --search 关键词 [--scope keys|values|all]
///   stool save-edit <存档文件> --set /路径=新值 [--set /路径=新值 ...] [--out 导出.json]
fn save_edit(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("用法: stool save-edit <存档文件> [--search 关键词] [--scope keys|values|all] [--set /路径=值 ...] [--out 导出.json]");
        return 1;
    }
    let path = PathBuf::from(&args[0]);
    let mut search: Option<String> = None;
    let mut scope = crate::features::saves::SearchScope::All;
    let mut sets: Vec<(String, String)> = Vec::new();
    let mut out: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--search" => {
                i += 1;
                search = args.get(i).cloned();
            }
            "--scope" => {
                i += 1;
                scope = match args.get(i).map(|s| s.as_str()) {
                    Some("keys") => crate::features::saves::SearchScope::Keys,
                    Some("values") => crate::features::saves::SearchScope::Values,
                    _ => crate::features::saves::SearchScope::All,
                };
            }
            "--set" => {
                i += 1;
                if let Some(kv) = args.get(i) {
                    if let Some((k, v)) = kv.split_once('=') {
                        sets.push((k.to_string(), v.to_string()));
                    }
                }
            }
            "--out" => {
                i += 1;
                out = args.get(i).map(PathBuf::from);
            }
            other => eprintln!("未知参数: {other}"),
        }
        i += 1;
    }
    let mut doc = match crate::features::saves::SaveDoc::load(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("✘ {e}");
            return 1;
        }
    };
    println!("格式: {}", doc.format.label());
    if let Some(q) = &search {
        let hits = doc.search(q, scope);
        println!("搜索 \"{q}\"（{}）命中 {} 处:", scope.label(), hits.len());
        for p in hits.iter().take(100) {
            let v = doc.get(p).map(|v| v.to_string()).unwrap_or_default();
            println!("  {p} = {v}");
        }
    }
    if out.is_none() && sets.is_empty() {
        return 0;
    }
    if !doc.format.writable() {
        eprintln!("✘ 该格式（{}）为只读，无法修改", doc.format.label());
        return 1;
    }
    for (ptr, val) in &sets {
        let jsonv = if val == "true" {
            serde_json::json!(true)
        } else if val == "false" {
            serde_json::json!(false)
        } else if let Ok(n) = val.parse::<i64>() {
            serde_json::json!(n)
        } else if let Ok(f) = val.parse::<f64>() {
            serde_json::json!(f)
        } else {
            serde_json::json!(val)
        };
        match doc.set(ptr, jsonv) {
            Ok(()) => println!("✔ 已修改 {ptr} = {val}"),
            Err(e) => {
                eprintln!("✘ 修改 {ptr} 失败: {e}");
                return 1;
            }
        }
    }
    if let Some(outp) = out {
        let text = serde_json::to_string_pretty(&doc.root).unwrap_or_default();
        if let Err(e) = std::fs::write(&outp, text) {
            eprintln!("✘ 导出失败: {e}");
            return 1;
        }
        println!("✔ 已导出 JSON → {}", outp.display());
    }
    if !sets.is_empty() {
        match doc.save() {
            Ok(m) => println!("✔ {m}"),
            Err(e) => {
                eprintln!("✘ {e}");
                return 1;
            }
        }
    }
    0
}

pub fn main_args(args: Vec<String>) -> i32 {
    if args.is_empty() {
        return crate::gui::run();
    }
    match args[0].as_str() {
        "gui" => crate::gui::run(),
        "save-edit" => save_edit(&args[1..]),
        "detect" => {
            let root = PathBuf::from(&args[1]);
            let reg = Registry::new();
            for d in reg.detect_all(&root) {
                if d.ok() {
                    println!("★ {} [{}] 置信度 {} — {}", d.name, d.plugin_id, d.score, d.evidence.join("; "));
                }
            }
            0
        }
        "extract" | "decompile" | "save" | "unlock" | "text-extract" | "text-import" => {
            let root = PathBuf::from(&args[1]);
            let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)).map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(format!("{}_stool_out", root.display())));
            let plugin = args.iter().position(|a| a == "-p").and_then(|i| args.get(i + 1)).cloned();
            let pairs: Vec<String> = args.iter().filter(|a| a.starts_with("--opt:")).map(|a| a.trim_start_matches("--opt:").to_string()).collect();
            let reg = Registry::new();
            let det = match plugin {
                Some(id) => reg.get(&id).map(|e| e.detect(&root)).ok_or_else(|| "未知插件".to_string()),
                None => reg.best(&root).ok_or_else(|| "未匹配到引擎（用 detect 查看）".to_string()),
            };
            let det = match det {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("✘ {e}");
                    return 1;
                }
            };
            println!("判定引擎: {} ({})", det.name, det.plugin_id);
            let op = match args[0].as_str() {
                "extract" => Op::Extract,
                "decompile" => Op::Decompile,
                "save" => Op::Save,
                "unlock" => Op::Unlock,
                "text-extract" => Op::TextExtract,
                _ => Op::TextImport,
            };
            let res = run_op(&reg, &det.plugin_id, op, &root, &out, opts_from(&pairs));
            println!("{} {}", if res.success { "✔" } else { "✘" }, res.message);
            if res.success { 0 } else { 1 }
        }
        "repack" => {
            let root = PathBuf::from(&args[1]);
            let pairs: Vec<String> = args.iter().filter(|a| a.starts_with("--opt:")).map(|a| a.trim_start_matches("--opt:").to_string()).collect();
            let reg = Registry::new();
            let det = reg.best(&root);
            let det = match det {
                Some(d) => d,
                None => {
                    eprintln!("✘ 未匹配到引擎");
                    return 1;
                }
            };
            let src = PathBuf::from(pairs.iter().find_map(|kv| kv.strip_prefix("src_dir=").map(|s| s.to_string())).unwrap_or_default());
            let res = run_op(&reg, &det.plugin_id, Op::Repack, &root, &PathBuf::from("."), opts_from(&pairs));
            let _ = src;
            println!("{} {}", if res.success { "✔" } else { "✘" }, res.message);
            if res.success { 0 } else { 1 }
        }
        "mod-install" => {
            let root = PathBuf::from(&args[1]);
            let patch = PathBuf::from(&args[2]);
            let name = args.iter().position(|a| a == "-n").and_then(|i| args.get(i + 1)).cloned().unwrap_or_default();
            match crate::features::mods::install_mod(&root, &patch, &name, &print_progress) {
                Ok(e) => {
                    println!("✔ 已安装 MOD '{}': {} 文件，覆盖 {} 原文件", e.name, e.files.len(), e.overwritten.len());
                    0
                }
                Err(e) => {
                    println!("✘ {e}");
                    1
                }
            }
        }
        "mod-list" => {
            for m in crate::features::mods::list_mods(&PathBuf::from(&args[1])) {
                println!("{} {} — {} 文件", if m.enabled { "[启用]" } else { "[停用]" }, m.name, m.files.len());
            }
            0
        }
        "mod-uninstall" | "mod-enable" | "mod-disable" => {
            let root = PathBuf::from(&args[1]);
            let name = &args[2];
            let r = match args[0].as_str() {
                "mod-uninstall" => crate::features::mods::uninstall_mod(&root, name).map(|_| "已卸载并还原".to_string()),
                "mod-enable" => crate::features::mods::toggle_mod(&root, name, true).map(|_| "已启用".to_string()),
                _ => crate::features::mods::toggle_mod(&root, name, false).map(|_| "已停用".to_string()),
            };
            match r {
                Ok(m) => {
                    println!("✔ {m}");
                    0
                }
                Err(e) => {
                    println!("✘ {e}");
                    1
                }
            }
        }
        _ => {
            eprintln!("未知命令: {}。可用: gui / detect / extract / repack / decompile / text-extract / text-import / save / unlock / mod-*", args[0]);
            2
        }
    }
}

// 静态引用避免 unused import 警告
#[allow(dead_code)]
fn _t(_: Arc<Mutex<()>>) {}
