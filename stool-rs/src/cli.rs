//! 命令行入口：detect / batch / engines / precheck / doctor / selfcheck / inject-support / unlock-support /
//! cg-candidates / extract / repack / decompile / text-extract / text-import / text-inject / text-uninject /
//! save / unlock / mod-* / gui。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::engines::{Op, Registry};

fn print_progress(frac: f32, msg: &str) {
    eprintln!("[{:>5.1}%] {}", (frac * 100.0).min(100.0), msg);
}

fn run_op(reg: &Registry, det_id: &str, op: Op, root: &Path, out: &Path, opts: HashMap<String, String>) -> crate::engines::OpOutcome {
    // 环境预检（P1-2）：写操作前置检查 目录可写 / 文件占用 / 磁盘空间，
    // 失败直接返回「原因 + 修法」，而不是跑到一半才失败（几 GB 解包尤其致命）。
    let scope = crate::features::precheck::scope_for_op(op, &opts);
    let rep = crate::features::precheck::run(root, Some(out), scope);
    if rep.worst() != crate::features::precheck::Level::Ok {
        for line in rep.lines() {
            eprintln!("[预检] {line}");
        }
    }
    if !rep.ok() {
        return crate::engines::OpOutcome::fail(rep.fail_summary());
    }
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
    // 解包并行化（P2-2）：每次操作前清空「已建目录」缓存，避免上一操作删过目录后缓存失真。
    crate::engines::clear_dir_cache();
    match op {
        Op::Extract => engine.extract(&ctx),
        Op::Repack => {
            let src = PathBuf::from(opts.get("src_dir").map(|s| s.as_str()).unwrap_or("patch_src"));
            engine.repack(&ctx, &src)
        }
        Op::Decompile => engine.decompile(&ctx),
        Op::TextExtract => engine.text_extract(&ctx, out),
        Op::TextImport => engine.text_import(&ctx, out),
        Op::TextInject => engine.text_inject(&ctx, out),
        Op::Save => engine.save(&ctx),
        Op::Unlock => engine.unlock(&ctx),
    }
}

/// cg-candidates 子命令：**只读**列出 Unity 全 CG 解锁的候选键名。
///
/// 与 `unlock` 的区别：完全不碰注册表（不打开、不创建、不写入），只报告
/// 「精确来源（Mono 程序集 / IL2CPP 元数据）+ 场景启发式」扫出来的候选。
/// 用途：写入前先看清候选是什么，或排查"为什么一个键都没扫到"。
///
/// 用法:
///   stool cg-candidates <Unity 游戏目录> [--opt:filter=<子串>] [--opt:max=<n>]
fn cg_candidates(args: &[String]) -> i32 {
    use crate::features::gallery;

    let root = PathBuf::from(&args[0]);
    if !root.is_dir() {
        eprintln!("✘ 目录不存在: {}", root.display());
        return 2;
    }
    let pairs: Vec<String> = args
        .iter()
        .filter(|a| a.starts_with("--opt:"))
        .map(|a| a.trim_start_matches("--opt:").to_string())
        .collect();
    let opts = opts_from(&pairs);
    let filter = opts.get("filter").map(|s| s.as_str());
    let max: usize = opts.get("max").and_then(|s| s.parse().ok()).unwrap_or(3000);

    println!("只读扫描: {}", root.display());

    // 精确定位信息
    let dlls = gallery::find_assemblies(&root);
    println!(
        "Mono 程序集: {}",
        if dlls.is_empty() {
            "未找到（非 Mono 版）".to_string()
        } else {
            dlls.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        }
    );
    match gallery::find_il2cpp_metadata(&root) {
        Some(p) => println!("IL2CPP 元数据: {}", p.display()),
        None => println!("IL2CPP 元数据: 未找到"),
    }
    if let Some(info) = gallery::read_app_info(&root) {
        println!("PlayerPrefs 位置: HKCU\\Software\\{}\\{}", info.company, info.product);
    } else {
        println!("PlayerPrefs 位置: 未知（未读到 app.info）");
    }
    let known = gallery::known_keys(&root);
    if !known.is_empty() {
        let preview: Vec<&String> = known.iter().take(10).collect();
        println!("注册表已有键 {} 个: {preview:?}", known.len());
    }

    // 候选
    let (mut candidates, scan) = gallery::collect_candidate_keys_detailed(&root);
    if scan.usable() {
        for (p, kind) in &scan.sources {
            println!("精确来源 [{}]: {}", kind.label(), p.display());
        }
        println!("  字面量候选 {} 条", scan.keys.len());
        if !scan.gallery_types.is_empty() {
            let preview: Vec<&String> = scan.gallery_types.iter().take(12).collect();
            println!("  画廊类型/字段 {} 个: {preview:?}", scan.gallery_types.len());
        }
    }
    for (p, e) in &scan.failures {
        println!("  ⚠ 解析失败 {}: {e}", p.display());
    }
    let (filtered, prefix) = gallery::prefer_same_family(&candidates, &known);
    if let Some(p) = &prefix {
        candidates = filtered;
        println!("已按现存键族前缀 `{p}` 收敛候选");
    }
    if let Some(f) = filter {
        candidates.retain(|c| c.contains(f));
        println!("已按子串 `{f}` 过滤");
    }
    // 去重 → 按相关性排序 → 最后截断（与 unlock 同一套口径）
    candidates.sort();
    candidates.dedup();
    gallery::sort_candidates(&mut candidates);
    let high_conf = gallery::high_confidence_count(&candidates);
    let truncated = candidates.len() > max;
    candidates.truncate(max);

    println!(
        "\n候选键名 {} 个（其中高置信 {high_conf} 个，已排在前面）:",
        candidates.len()
    );
    for c in &candidates {
        println!("  {c}");
    }
    if truncated {
        println!("（候选过多已截断到 {max}，用 --opt:max= 放宽）");
    }
    if candidates.is_empty() {
        println!(
            "  （无）——该作品可能不在注册表里存画廊状态：\
             可考虑「替换自带全CG存档」或「游戏内全开开关」，见 stool unlock-support 与 stool unlock。"
        );
    }
    0
}

/// xp3-patch 子命令：KiriKiri / 吉里吉里 运行时补丁包（不改原封包、删除即还原）。
///
/// 用法:
///   stool xp3-patch <游戏目录> --list
///   stool xp3-patch <游戏目录> --src <改动目录> [--name patchN.xp3]
///   stool xp3-patch <游戏目录> --remove <patchN.xp3>
fn xp3_patch(args: &[String]) -> i32 {
    let root = PathBuf::from(&args[0]);
    let mut list = false;
    let mut remove: Option<String> = None;
    let mut src: Option<PathBuf> = None;
    let mut from_extract: Option<PathBuf> = None;
    let mut name: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--list" => list = true,
            "--remove" => {
                i += 1;
                remove = args.get(i).cloned();
            }
            "--src" => {
                i += 1;
                src = args.get(i).map(PathBuf::from);
            }
            "--from-extract" => {
                i += 1;
                from_extract = args.get(i).map(PathBuf::from);
            }
            "--name" => {
                i += 1;
                name = args.get(i).cloned();
            }
            other => eprintln!("未知参数: {other}"),
        }
        i += 1;
    }

    if let Some(n) = remove {
        return match crate::features::xp3patch::remove(&root, &n) {
            Ok(m) => {
                println!("✔ {m}");
                0
            }
            Err(e) => {
                eprintln!("✘ {e}");
                1
            }
        };
    }

    if let Some(src) = src {
        return match crate::features::xp3patch::build(&root, &src, name.as_deref()) {
            Ok(m) => {
                println!("✔ {m}");
                0
            }
            Err(e) => {
                eprintln!("✘ {e}");
                1
            }
        };
    }

    // 闭环用法：给出解包目录，自动只打包与现有封包不同的文件
    if let Some(dir) = from_extract {
        return match crate::features::xp3patch::build_changed(&root, &dir, name.as_deref()) {
            Ok(m) => {
                println!("✔ {m}");
                0
            }
            Err(e) => {
                eprintln!("✘ {e}");
                1
            }
        };
    }

    // 默认（含 --list）：列出封包与搜索顺序
    let _ = list;
    let items = crate::features::xp3patch::list(&root);
    if items.is_empty() {
        eprintln!("✘ {} 下没有找到 data*.xp3 / patch*.xp3", root.display());
        return 1;
    }
    println!("{} 下的封包（引擎搜索顺序，越靠后优先级越高）：", root.display());
    for it in &items {
        let tag = if it.ours { "本工具创建" } else { "非本工具创建" };
        let mut extra = String::new();
        if let Some(n) = it.entries {
            extra.push_str(&format!("，{n} 条目"));
        }
        if it.encrypted {
            extra.push_str("，内容加密");
        }
        println!("  · {}（{}，{}{})", it.name, crate::features::precheck::human_bytes(it.bytes), tag, extra);
    }
    println!("下一个可用的补丁包名: {}", crate::features::xp3patch::next_name(&root));
    0
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

/// 机翻子命令：翻译文本提取 CSV 或注入 JSON 的空译文。
/// 用法:
///   stool text-mtl <文件.csv|.json> [--preset deepseek|zhipu|ollama]
///                  [--base-url URL --model 模型名 --key KEY] [--batch 20] [--jobs 4]
fn text_mtl(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!(
            "用法: stool text-mtl <文件.csv|.json> [--preset deepseek|zhipu|ollama] [--base-url URL --model 名 --key KEY] [--batch 20] [--jobs 4]"
        );
        eprintln!("预设: deepseek=https://api.deepseek.com deepseek-chat | zhipu=https://open.bigmodel.cn/api/paas/v4 glm-4-flash | ollama=http://127.0.0.1:11434/v1 本地模型名");
        return 1;
    }
    let path = PathBuf::from(&args[0]);
    let (mut base_url, mut model, mut key) = (String::new(), String::new(), String::new());
    let mut glossary = String::new();
    let mut batch = 20usize;
    let mut jobs = 4usize;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--preset" => {
                i += 1;
                match args.get(i).map(|s| s.as_str()) {
                    Some("deepseek") => {
                        base_url = "https://api.deepseek.com".into();
                        model = "deepseek-chat".into();
                    }
                    Some("zhipu") => {
                        base_url = "https://open.bigmodel.cn/api/paas/v4".into();
                        model = "glm-4-flash".into();
                    }
                    Some("ollama") => {
                        base_url = "http://127.0.0.1:11434/v1".into();
                        model = std::env::var("STOOL_OLLAMA_MODEL").unwrap_or_else(|_| "qwen2.5:7b".into());
                    }
                    other => eprintln!("未知预设: {other:?}（可选 deepseek/zhipu/ollama）"),
                }
            }
            "--base-url" => {
                i += 1;
                base_url = args.get(i).cloned().unwrap_or_default();
            }
            "--model" => {
                i += 1;
                model = args.get(i).cloned().unwrap_or_default();
            }
            "--key" => {
                i += 1;
                key = args.get(i).cloned().unwrap_or_default();
            }
            "--glossary" => {
                i += 1;
                if let Some(gp) = args.get(i) {
                    glossary = std::fs::read_to_string(gp).unwrap_or_else(|e| {
                        eprintln!("✘ 读取术语表失败: {e}");
                        String::new()
                    });
                }
            }
            "--batch" => {
                i += 1;
                batch = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(20);
            }
            "--jobs" => {
                i += 1;
                jobs = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(4).clamp(1, 32);
            }
            other => eprintln!("未知参数: {other}"),
        }
        i += 1;
    }
    if base_url.is_empty() || model.is_empty() {
        eprintln!("✘ 缺少 --preset 或 --base-url/--model 配置");
        return 1;
    }
    let tr = crate::features::translate::OpenAiCompat { base_url, api_key: key, model, glossary };
    println!("机翻引擎: {} ({}) 并发 {} 批", tr.model, tr.base_url, jobs);
    let cancel = AtomicBool::new(false);
    let r = if path.extension().map(|e| e == "json").unwrap_or(false) {
        crate::features::translate::translate_json(&path, &tr, batch, jobs, &print_progress, &cancel)
    } else {
        crate::features::translate::translate_csv(&path, &tr, batch, jobs, &print_progress, &cancel)
    };
    match r {
        Ok((0, 0)) => {
            println!("✔ 没有待翻条目（译文已全部就绪）");
            0
        }
        Ok((applied, total)) => {
            println!("✔ 已机翻 {applied}/{total} 条 → {}", path.display());
            0
        }
        Err(e) => {
            eprintln!("✘ {e}");
            1
        }
    }
}

/// 校验各子命令的必填位置参数。返回 `Some(用法)` 表示参数不足，
/// 调用方据此打印用法并返回退出码 2，避免直接索引 `args[n]` 触发 panic。
fn missing_arg_usage(args: &[String]) -> Option<&'static str> {
    let n = args.len();
    match args[0].as_str() {
        "detect" => (n < 2).then_some("stool detect <游戏目录>"),
        "batch" => (n < 2).then_some("stool batch <根目录> [--depth N] [--op detect|extract|decompile|text-extract|save|unlock] [-o 输出基目录] [--csv 报告.csv]"),
        "precheck" => (n < 2).then_some("stool precheck <游戏目录> [-o 输出目录] [--out-only|--root-only|--read-only]"),
        "doctor" => (n < 2).then_some("stool doctor <游戏目录>"),
        "selfcheck" => (n < 2).then_some("stool selfcheck <游戏目录|封包文件> [-o 工作目录]"),
        "extract" | "decompile" | "save" | "unlock" | "text-extract" | "text-import"
        | "text-inject" => (n < 2).then_some(
            "stool <命令> <游戏目录> [-o 输出目录] [-p 插件id] [--opt:key=value]",
        ),
        "repack" => (n < 2).then_some("stool repack <游戏目录> [--opt:src_dir=<解包目录>]"),
        "text-uninject" => (n < 2).then_some("stool text-uninject <游戏目录>"),
        "restore" => (n < 2).then_some("stool restore <文件|.stool.bak 路径|目录>"),
        "pack-apply" => (n < 3).then_some("stool pack-apply <原封包> <新封包>"),
        "archive-toggle" => (n < 2).then_some("stool archive-toggle <游戏目录>"),
        "mod-install" => (n < 3).then_some("stool mod-install <游戏目录> <补丁目录> [-n 名称] [--force]"),
        "mod-list" | "mod-conflicts" => (n < 2).then_some("stool mod-list|mod-conflicts <游戏目录>"),
        "mod-uninstall" | "mod-enable" | "mod-disable" => {
            (n < 3).then_some("stool mod-<动作> <游戏目录> <MOD名称>")
        }
        "xp3-patch" => (n < 2).then_some(
            "stool xp3-patch <游戏目录> --list | --src <改动目录> [--name patchN.xp3] | --from-extract <解包目录> [--name patchN.xp3] | --remove <patchN.xp3>",
        ),
        "cg-candidates" => (n < 2).then_some(
            "stool cg-candidates <Unity 游戏目录> [--opt:filter=<子串>] [--opt:max=<n>]",
        ),
        _ => None,
    }
}

pub fn main_args(args: Vec<String>) -> i32 {
    if args.is_empty() {
        return crate::gui::run();
    }
    if let Some(usage) = missing_arg_usage(&args) {
        eprintln!("✘ 参数不足。用法: {usage}");
        return 2;
    }
    match args[0].as_str() {
        "xp3-patch" => xp3_patch(&args[1..]),
        "cg-candidates" => cg_candidates(&args[1..]),
        "gui" => crate::gui::run(),
        "save-edit" => save_edit(&args[1..]),
        "text-mtl" => text_mtl(&args[1..]),
        "detect" => {
            let root = PathBuf::from(&args[1]);
            let reg = Registry::new();
            let all = reg.detect_all(&root);
            // 选定引擎（已按 (分数, 优先级) 降序，取第一条），未达判定线时明确标注
            let chosen = Registry::pick_from(&all);
            let confirmed: Vec<_> = all.iter().filter(|d| d.ok()).collect();
            for d in &confirmed {
                println!(
                    "★ {} [{}] {} 分 · {} — {}",
                    d.name, d.plugin_id, d.score, d.confidence().label(), d.evidence.join("; ")
                );
                if !d.notes.is_empty() {
                    println!("   备注: {}", d.notes);
                }
            }
            if confirmed.is_empty() {
                let sus: Vec<_> = all.iter().filter(|d| d.score > 0).take(3).collect();
                if sus.is_empty() {
                    println!("○ 未识别：没有任何引擎特征命中。请确认目录选到了含主程序 exe 的那一层（不是上一级、也不是子目录）。");
                } else {
                    println!("○ 未确认（无引擎达到 {} 分判定线），分数最高的疑似：", crate::engines::DETECT_LINE);
                    for d in sus {
                        println!("  · {} [{}] {} 分 — {}", d.name, d.plugin_id, d.score, d.evidence.join("; "));
                    }
                }
            }
            if let Some((d, confident)) = &chosen {
                println!(
                    "\n→ 选定: {} [{}] {} 分（{}）",
                    d.name,
                    d.plugin_id,
                    d.score,
                    if *confident {
                        "已确认"
                    } else {
                        "未达判定线，仅供预览；可用 -p <id> 强制指定"
                    }
                );
            }
            0
        }
        "batch" => {
            // 用法: stool batch <根目录> [--depth N] [--op detect|...] [-o 输出基目录] [--csv 报告.csv] [--opt:key=value]
            let base = PathBuf::from(&args[1]);
            let mut depth = 3usize;
            let mut op_name = "detect".to_string();
            let mut out_base: Option<PathBuf> = None;
            let mut csv: Option<PathBuf> = None;
            let mut pairs: Vec<String> = Vec::new();
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--depth" => {
                        i += 1;
                        depth = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(3);
                    }
                    "--op" => {
                        i += 1;
                        op_name = args.get(i).cloned().unwrap_or_else(|| "detect".into());
                    }
                    "--csv" => {
                        i += 1;
                        csv = args.get(i).map(PathBuf::from);
                    }
                    "-o" => {
                        i += 1;
                        out_base = args.get(i).map(PathBuf::from);
                    }
                    s if s.starts_with("--opt:") => pairs.push(s.trim_start_matches("--opt:").to_string()),
                    other => eprintln!("未知参数: {other}"),
                }
                i += 1;
            }
            let roots = crate::features::batch::discover(&base, depth);
            if roots.is_empty() {
                println!("✘ 在 {} 下未发现候选游戏目录（深度 {depth}）", base.display());
                return 1;
            }
            println!("发现 {} 个候选游戏目录（根: {}）", roots.len(), base.display());
            let prog = |idx: usize, n: usize, p: &Path| eprintln!("[{}/{n}] {}", idx + 1, p.display());
            let items = if op_name == "detect" {
                crate::features::batch::detect_batch(&roots, &prog)
            } else {
                let op = match op_name.as_str() {
                    "extract" => Op::Extract,
                    "decompile" => Op::Decompile,
                    "text-extract" => Op::TextExtract,
                    "save" => Op::Save,
                    "unlock" => Op::Unlock,
                    other => {
                        eprintln!("✘ 不支持的 --op: {other}（可选 detect/extract/decompile/text-extract/save/unlock）");
                        return 2;
                    }
                };
                let out_base = out_base.unwrap_or_else(|| base.join("stool_batch_out"));
                crate::features::batch::run_batch(&roots, op, &out_base, &opts_from(&pairs), &prog)
            };
            for it in &items {
                let mark = if it.ok { "✔" } else { "✘" };
                let eng = if it.engine_id.is_empty() { "未识别" } else { &it.engine_id };
                println!("{mark} {} [{eng}] {} — {}", it.root.display(), it.confidence, it.message);
            }
            println!("{}", crate::features::batch::summary(&items));
            if let Some(c) = csv {
                match std::fs::write(&c, crate::features::batch::to_csv(&items)) {
                    Ok(_) => println!("报告已写出: {}", c.display()),
                    Err(e) => eprintln!("✘ 写 CSV 失败: {e}"),
                }
            }
            0
        }
        "inject-support" => {
            let t = crate::features::inject::SUPPORT_TABLE;
            println!("运行时 JSON 注入支持范围（共 {} 个引擎）：", t.len());
            for s in t {
                println!("  · {} [{}]", s.engine, s.plugin_id);
                println!("      适配: {}", s.mechanism);
                println!("      边界: {}", s.limits);
            }
            println!("未列出的引擎（KiriKiri / Siglus / BGI / RPG Maker 2000 等）：文本封在私有封包或编译脚本里，");
            println!("请走「解包 → 文本提取 → 翻译 → 翻译回填 / 封包回写」这套离线流程；");
            println!("KiriKiri 系还可以走「运行时补丁包」（xp3-patch），见下：");
            let pt = crate::features::xp3patch::PATCH_TARGETS;
            println!("\n运行时补丁包支持范围（共 {} 个引擎）：", pt.len());
            for s in pt {
                println!("  · {} [{}]", s.engine, s.plugin_id);
                println!("      机制: {}", s.mechanism);
                println!("      边界: {}", s.limits);
                println!("      注意: {}", s.caveat);
            }
            0
        }
        "engines" => {
            let reg = Registry::new();
            let matrix = reg.capability_matrix();
            println!("引擎能力矩阵（{} 个插件；能力 = 插件声明 ∪ 解锁策略表推导）", matrix.len());
            for (id, name, caps) in matrix {
                let cap_str = caps.iter().map(|c| c.label()).collect::<Vec<_>>().join(" / ");
                println!("  · {name} [{id}]");
                println!("      能力: {cap_str}");
                let spec = crate::features::unlock::spec_or_generic(&id);
                let routes = spec
                    .routes
                    .iter()
                    .map(|r| format!("{}({})", r.label(), r.key()))
                    .collect::<Vec<_>>()
                    .join(" > ");
                println!("      识别依据: {}", spec.basis);
                println!("      解锁动作: {}", spec.action);
                println!("      解锁手段（优先序）: {routes}");
            }
            0
        }
        "unlock-support" => {
            let table = crate::features::unlock::UNLOCK_CATALOG;
            println!("全 CG 解锁策略表（{} 个引擎；未列出的走兜底策略）", table.len());
            for s in table {
                let routes = s
                    .routes
                    .iter()
                    .map(|r| format!("{}({})", r.label(), r.key()))
                    .collect::<Vec<_>>()
                    .join(" > ");
                println!("  · {} [{}]", s.engine_id, s.basis);
                println!("      手段: {routes}");
                println!("      动作: {}", s.action);
                if !s.note.is_empty() {
                    println!("      提示: {}", s.note);
                }
            }
            println!(
                "\n用法: stool unlock <游戏目录> [--opt:apply=1] [--opt:route=<{}>] [--opt:save_dir=<目录>]",
                crate::features::unlock::route_keys()
            );
            println!("缺省为只读预览；覆盖写盘前自动备份为 .stool.bak，可用 stool restore 还原。");
            println!("Unity 可先用 `stool cg-candidates <游戏目录>` 只读查看候选键名（完全不碰注册表）。");
            0
        }
        "precheck" => {
            // 用法: stool precheck <游戏目录> [-o 输出目录] [--out-only|--root-only|--read-only]
            use crate::features::precheck::{self, Scope};
            let root = PathBuf::from(&args[1]);
            let mut out: Option<PathBuf> = None;
            let mut scope = Scope::both();
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "-o" => {
                        i += 1;
                        out = args.get(i).map(PathBuf::from);
                    }
                    "--out-only" => scope = Scope::out_only(),
                    "--root-only" => scope = Scope::root_only(),
                    "--read-only" => scope = Scope::read_only(),
                    other => eprintln!("未知参数: {other}"),
                }
                i += 1;
            }
            let rep = precheck::run(&root, out.as_deref(), scope);
            println!("环境预检: {}", root.display());
            for line in rep.lines() {
                println!("{line}");
            }
            if rep.ok() {
                println!("✔ 预检通过，可以开始操作。");
                0
            } else {
                println!("✘ {}（退出码 2）", rep.fail_summary());
                2
            }
        }
        "doctor" => {
            // 用法: stool doctor <游戏目录>
            // D1 游戏体检（P2-8）：区域设置 / 日文字体 / 运行库 DLL / 路径 / 写权限。
            let root = PathBuf::from(&args[1]);
            let reg = Registry::new();
            let engine = reg.best(&root).map(|d| d.plugin_id).unwrap_or_default();
            let rep = crate::features::health::check(&root, &engine);
            println!(
                "游戏体检: {}（引擎: {}）",
                root.display(),
                if engine.is_empty() { "未识别" } else { engine.as_str() }
            );
            for line in rep.lines() {
                println!("{line}");
            }
            let hints = rep
                .items
                .iter()
                .filter(|i| i.level != crate::features::precheck::Level::Ok)
                .count();
            if rep.ok() {
                println!("✔ 体检完成：{hints} 项提示，未发现阻断性问题。");
                0
            } else {
                println!("✘ 体检发现问题（见上）。");
                2
            }
        }
        "selfcheck" => {
            // 用法: stool selfcheck <游戏目录|封包文件> [-o 工作目录]
            // P2-6 封包自检：解包 → 重打包 → 逐条目比对，确认对该封包的读写无损。
            let target = PathBuf::from(&args[1]);
            let mut work: Option<PathBuf> = None;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "-o" => {
                        i += 1;
                        work = args.get(i).map(PathBuf::from);
                    }
                    other => eprintln!("未知参数: {other}"),
                }
                i += 1;
            }
            let work_root = work.unwrap_or_else(crate::features::selfcheck::default_work_root);
            if target.is_file() {
                println!("封包自检: {}", target.display());
                match crate::features::selfcheck::check_archive(&target, &work_root) {
                    Ok(o) => {
                        for line in o.report().lines() {
                            println!("{line}");
                        }
                        if o.ok() {
                            println!("✔ 无损：解包→重打包→回读逐条目一致。");
                            0
                        } else {
                            println!("✘ 发现 {} 处往返不一致（见上）。", o.mismatches.len());
                            1
                        }
                    }
                    Err(e) => {
                        eprintln!("✘ {e}");
                        1
                    }
                }
            } else {
                println!("封包自检: {}", target.display());
                let rep = crate::features::selfcheck::check_dir(&target, &work_root);
                for line in rep.lines() {
                    println!("{line}");
                }
                if rep.ok() {
                    println!("✔ 自检通过（无不一致）。");
                    0
                } else {
                    println!("✘ 自检发现阻断性问题（见上）。");
                    2
                }
            }
        }
        "diag-export" => {
            // 用法: stool diag-export [游戏目录] [-o 输出.zip]
            let mut game: Option<PathBuf> = None;
            let mut out: Option<PathBuf> = None;
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "-o" => {
                        i += 1;
                        out = args.get(i).map(PathBuf::from);
                    }
                    other if !other.starts_with('-') => game = Some(PathBuf::from(other)),
                    other => eprintln!("未知参数: {other}"),
                }
                i += 1;
            }
            let out = out.unwrap_or_else(|| {
                PathBuf::from(format!("stool_diag_{}.zip", crate::diag::stamp_compact()))
            });
            match crate::features::diagpack::export(game.as_deref(), &out) {
                Ok(r) => {
                    println!(
                        "✔ 诊断包已导出 → {}（{} 个条目，含 {} 条告警）",
                        r.zip_path.display(),
                        r.entries.len(),
                        r.warnings
                    );
                    for e in &r.entries {
                        println!("    · {e}");
                    }
                    if r.warnings > 0 {
                        println!("提示：warnings.txt 里汇总了 WARN/ERROR/PANIC 行，排障先看它。");
                    }
                    0
                }
                Err(e) => {
                    eprintln!("✘ {e}");
                    1
                }
            }
        }
        "extract" | "decompile" | "save" | "unlock" | "text-extract" | "text-import" | "text-inject" => {
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
                "text-inject" => Op::TextInject,
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
        "text-uninject" => {
            let root = PathBuf::from(&args[1]);
            let reg = Registry::new();
            let plugin = reg
                .best(&root)
                .map(|d| d.plugin_id)
                .unwrap_or_default();
            match crate::features::inject::uninstall(&root, &plugin) {
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
        "pack-export" => {
            // 用法: stool pack-export -o 包.zip [--csv 文件.csv] [--json 翻译.json] [--engine 引擎id] [--game 游戏名]
            let mut out = PathBuf::from("pack.stoolpack.zip");
            let (mut csv, mut json, mut engine, mut game) = (None, None, String::new(), String::new());
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "-o" => {
                        i += 1;
                        out = args.get(i).map(PathBuf::from).unwrap_or(out);
                    }
                    "--csv" => {
                        i += 1;
                        csv = args.get(i).map(PathBuf::from);
                    }
                    "--json" => {
                        i += 1;
                        json = args.get(i).map(PathBuf::from);
                    }
                    "--engine" => {
                        i += 1;
                        engine = args.get(i).cloned().unwrap_or_default();
                    }
                    "--game" => {
                        i += 1;
                        game = args.get(i).cloned().unwrap_or_default();
                    }
                    other => eprintln!("未知参数: {other}"),
                }
                i += 1;
            }
            match crate::features::tpack::export_pack(csv.as_deref(), json.as_deref(), &out, &engine, &game) {
                Ok((n, filled)) => {
                    println!("✔ 已打包 {n} 个文件 → {}（JSON 已填 {filled} 条译文）", out.display());
                    0
                }
                Err(e) => {
                    eprintln!("✘ {e}");
                    1
                }
            }
        }
        "pack-import" => {
            // 用法: stool pack-import 包.zip <游戏目录> [--out 输出目录]
            if args.len() < 3 {
                eprintln!("用法: stool pack-import <翻译包.zip> <游戏目录> [--out 输出目录]");
                return 1;
            }
            let pack = PathBuf::from(&args[1]);
            let root = PathBuf::from(&args[2]);
            let out = args.iter().position(|a| a == "--out").and_then(|i| args.get(i + 1)).map(PathBuf::from)
                .unwrap_or_else(|| root.join("stool_output"));
            match crate::features::tpack::import_pack(&pack, &root, &out) {
                Ok(imp) => {
                    if let Some(jp) = &imp.json_path {
                        println!("✔ JSON → {}（已填 {} 条译文；可直接 stool text-inject 注入）", jp.display(), imp.json_filled);
                    }
                    if let Some(cp) = &imp.csv_path {
                        println!("✔ CSV → {}（可 text-import 回填 / text-mtl 继续机翻）", cp.display());
                    }
                    if !imp.engine.is_empty() {
                        println!("  目标引擎: {}（游戏: {}）", imp.engine, imp.game);
                    }
                    0
                }
                Err(e) => {
                    eprintln!("✘ {e}");
                    1
                }
            }
        }
        "pack-apply" => {
            // 用法: stool pack-apply <原封包> <新封包>
            // 回填一键闭环（P2-5）：用重打包产物替换原封包，覆盖前自动留底（绝不再覆盖旧备份）。
            let target = PathBuf::from(&args[1]);
            let new_archive = PathBuf::from(&args[2]);
            match crate::features::restore::apply_repack(&target, &new_archive) {
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
        "restore" => {
            // 用法: stool restore <文件或 .stool.bak 路径或目录>
            // 文件 → 从同名 .stool.bak 还原；.stool.bak 文件 → 直接还原；目录 → 只列出备份
            let p = PathBuf::from(&args[1]);
            if p.is_dir() {
                let baks = crate::features::restore::find_backups(&p);
                if baks.is_empty() {
                    println!("未发现 .stool.bak 备份");
                } else {
                    println!("发现 {} 个备份（用 stool restore <备份路径> 还原）:", baks.len());
                    for b in &baks {
                        println!("  {}", b.display());
                    }
                }
                0
            } else {
                let bak = if p.to_string_lossy().ends_with(".stool.bak") { p } else {
                    let mut n = p.file_name().map(|s| s.to_os_string()).unwrap_or_default();
                    n.push(".stool.bak");
                    p.with_file_name(n)
                };
                match crate::features::restore::restore_one(&bak) {
                    Ok(m) => {
                        println!("✔ {m}");
                        0
                    }
                    Err(e) => {
                        eprintln!("✘ {e}");
                        1
                    }
                }
            }
        }
        "archive-toggle" => {
            let root = PathBuf::from(&args[1]);
            match crate::features::restore::archive_toggle(&root) {
                Ok(m) => {
                    println!("✔ {m}");
                    0
                }
                Err(e) => {
                    eprintln!("✘ {e}");
                    1
                }
            }
        }
        "mod-install" => {
            let root = PathBuf::from(&args[1]);
            let patch = PathBuf::from(&args[2]);
            let name = args.iter().position(|a| a == "-n").and_then(|i| args.get(i + 1)).cloned().unwrap_or_default();
            let force = args.iter().any(|a| a == "--force" || a == "--allow-conflict");
            match crate::features::mods::install_mod(&root, &patch, &name, &print_progress, force) {
                Ok(e) => {
                    println!("✔ 已安装 MOD '{}': {} 文件，覆盖 {} 原文件", e.name, e.files.len(), e.overwritten.len());
                    let c = crate::features::mods::conflicts(&root);
                    if !c.is_empty() {
                        println!("⚠ 当前存在 {} 处 MOD 冲突（mod-conflicts 可单独查看）：", c.len());
                        for x in c.iter().take(8) {
                            println!("   · {} ← {}", x.rel, x.mods.join(" / "));
                        }
                    }
                    0
                }
                Err(e) => {
                    println!("✘ {e}");
                    1
                }
            }
        }
        "mod-list" => {
            let root = PathBuf::from(&args[1]);
            for m in crate::features::mods::list_mods(&root) {
                println!("{} {} — {} 文件", if m.enabled { "[启用]" } else { "[停用]" }, m.name, m.files.len());
            }
            let c = crate::features::mods::conflicts(&root);
            if !c.is_empty() {
                println!("⚠ 检测到 {} 处冲突（多个已启用 MOD 覆盖同一文件）：", c.len());
                for x in &c {
                    println!("   · {} ← {}", x.rel, x.mods.join(" / "));
                }
            }
            0
        }
        "mod-conflicts" => {
            let root = PathBuf::from(&args[1]);
            let c = crate::features::mods::conflicts(&root);
            if c.is_empty() {
                println!("✔ 无 MOD 冲突");
                0
            } else {
                for x in &c {
                    println!("⚠ {} ← {}", x.rel, x.mods.join(" / "));
                }
                println!("共 {} 处冲突（退出码 1）", c.len());
                1
            }
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
            eprintln!("未知命令: {}。可用: gui / detect / batch / engines / precheck / doctor / selfcheck / inject-support / unlock-support / cg-candidates / extract / repack / decompile / text-extract / text-mtl / text-import / text-inject / text-uninject / save / save-edit / unlock / restore / pack-apply / archive-toggle / pack-export / pack-import / xp3-patch / mod-install / mod-list / mod-conflicts / mod-uninstall / mod-enable / mod-disable / diag-export", args[0]);
            2
        }
    }
}

// 静态引用避免 unused import 警告
#[allow(dead_code)]
fn _t(_: Arc<Mutex<()>>) {}
