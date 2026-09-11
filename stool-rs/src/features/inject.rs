//! 运行时 JSON 注入汉化（MTool 兼容；按引擎分派，不修改任何原始资源文件）。
//!
//! 支持的引擎与机制（目标范围 = "文本走 JS/DOM/脚本，且引擎提供运行时替换入口"的引擎）：
//! - **RPG Maker MV / MZ**：往游戏里安装小型插件 `js/plugins/stool_translate.js`
//!   （备份 `js/plugins.js` 后登记），启动后读取翻译 JSON，在内存里替换数据库与显示文本；
//! - **Ren'Py**：新增 `game/stool_translate.rpy`，利用引擎自带的 `config.replace_text`
//!   在文本显示前做整句替换（只新增文件，原有脚本不动；删除该文件即还原）；
//! - **TyranoBuilder / TyranoScript**：在 `index.html` 末尾追加 `stool_translate.js`
//!   （原文件备份）。TyranoScript 的 KAG 把文本渲染进 DOM（非 Canvas），因此
//!   DOM 文本节点替换即可覆盖；Hook 额外挂了 jQuery `html/text` 兜底（Tyrano 是 jQuery 系）；
//! - **HTML / Electron**：在入口 HTML 末尾追加 `<script src="stool_translate.js">`
//!   （原文件备份），DOM 文本节点经 MutationObserver 运行时替换（Canvas 渲染的内容除外）。
//!
//! 不在范围内（以及原因）：KiriKiri / Siglus / BGI / Majesty 等的脚本与文本封在私有封包里，
//! 且 xp3/pak 解包后常为加密 `.ks/.tjs`，运行时没有稳定的替换注入点 —— 这类引擎请走
//! "解包 → 文本提取 → 翻译 → 封包回填"（`Op::TextImport` / `Op::Repack`）。
//! 其中 **KiriKiri 系另有更省事的做法**：`features::xp3patch` 的运行时补丁包
//! （打成 `patchN.xp3`，引擎按搜索顺序优先加载，原封包一个字节都不用动、删除即还原），
//! CLI 为 `stool xp3-patch`，GUI 在「运行时修改」页。
//!
//! JSON 格式约定（与 MTool 兼容，全引擎统一）：
//! - 扁平映射：`{ "原文": "译文", ... }`；也允许一层分组嵌套（如 `{ "对话": { "原文": "译文" } }`），
//!   加载时自动拍平、组名忽略；
//! - UTF-8（允许 BOM）；值为空字符串的条目按"未翻译"跳过（保留原文）；
//! - 存放路径：注入时把翻译 JSON 复制为 `<游戏目录>/stool_translate.json`（Ren'Py 例外：
//!   映射直接写进 .rpy，游戏目录不落 JSON）。
//!
//! 未命中映射的文本一律保留原文。
//! 幂等与可还原：所有会改动的原文件（`plugins.js`、入口 HTML）首次注入前都做
//! `*.stool.bak` 备份，卸载时优先按备份字节级还原。

use std::path::{Path, PathBuf};

pub const HOOK_NAME: &str = "stool_translate";
pub const JSON_IN_GAME: &str = "stool_translate.json";

/// 支持注入的引擎插件 id（engines 注册表里的 id）。
pub const SUPPORTED: [&str; 4] = ["rpgmaker_mv", "renpy", "html_game", "tyrano"];

/// 单个引擎的注入适配说明（供 GUI/CLI 明确展示"支持谁、怎么注入、有什么限制"）。
pub struct InjectSupport {
    pub plugin_id: &'static str,
    pub engine: &'static str,
    /// 注入适配方式
    pub mechanism: &'static str,
    /// 兼容性/稳定性边界
    pub limits: &'static str,
}

/// 注入能力总表：UI 用它来解释注入范围，避免用户对"支持哪些引擎"产生误解。
///
/// 排序即推荐展示顺序（覆盖面 / 稳定性由高到低）。
pub const SUPPORT_TABLE: &[InjectSupport] = &[
    InjectSupport {
        plugin_id: "renpy",
        engine: "Ren'Py",
        mechanism: "新增 game/stool_translate.rpy，走引擎原生 config.replace_text 在显示前整句替换",
        limits: "只新增文件、不改原脚本；不改动游戏资源，删除该文件（含 .rpyc）即完全还原，兼容性最好",
    },
    InjectSupport {
        plugin_id: "rpgmaker_mv",
        engine: "RPG Maker MV / MZ",
        mechanism: "安装 js/plugins/stool_translate.js 并登记进 plugins.js，运行时在内存里替换数据库与显示文本",
        limits: "首次注入会备份 plugins.js 后追加一条；仅覆盖 JS 文本，图片内文字不在范围内",
    },
    InjectSupport {
        plugin_id: "tyrano",
        engine: "TyranoBuilder / TyranoScript",
        mechanism: "在 index.html 追加 stool_translate.js；DOM 文本节点 + jQuery html/text 双层替换（KAG 渲染到 DOM，非 Canvas）",
        limits: "只改入口 HTML（有备份）；Canvas/WebGL 里画的文字无法替换；已打包的 .ks 剧本请改用文本提取/回填",
    },
    InjectSupport {
        plugin_id: "html_game",
        engine: "HTML / Electron",
        mechanism: "在入口 HTML 追加 stool_translate.js，MutationObserver 替换 DOM 文本节点",
        limits: "只改入口 HTML（有备份）；Canvas 渲染的文字无法替换；Electron 需对 app 目录有写权限",
    },
];

/// 该引擎是否支持运行时注入。
pub fn is_supported(plugin_id: &str) -> bool {
    SUPPORTED.contains(&plugin_id)
}

/// 取某个引擎的注入适配说明。
pub fn support_of(plugin_id: &str) -> Option<&'static InjectSupport> {
    SUPPORT_TABLE.iter().find(|s| s.plugin_id == plugin_id)
}

/// 一个游戏的 js 目录（MV/MZ：游戏根 或 www/ 下）。
fn js_dir(root: &Path) -> Option<PathBuf> {
    [root.join("www").join("js"), root.join("js")]
        .into_iter()
        .find(|c| c.join("plugins.js").exists())
}

/// Ren'Py 的 game 目录。
fn renpy_game_dir(root: &Path) -> Option<PathBuf> {
    let g = root.join("game");
    g.is_dir().then_some(g)
}

/// HTML 游戏的入口 html（优先 index.html，其次根目录第一个 .html）。
fn html_entry(root: &Path) -> Option<PathBuf> {
    let idx = root.join("index.html");
    if idx.exists() {
        return Some(idx);
    }
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|d| d.path())
        .find(|p| {
            p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("html")).unwrap_or(false)
        })
}

/// 加载并校验翻译 JSON，返回 (拍平后的映射, 总条目数, 空译文条目数)。
/// 格式：扁平 {原文:译文}，或一层（任意层）分组嵌套（自动拍平）。
pub fn load_map(path: &Path) -> Result<(serde_json::Map<String, serde_json::Value>, usize, usize), String> {
    let txt = std::fs::read_to_string(path).map_err(|e| format!("读取翻译 JSON 失败: {e}"))?;
    let txt = txt.trim_start_matches('\u{feff}');
    let v: serde_json::Value = serde_json::from_str(txt).map_err(|e| format!("翻译 JSON 不是合法 JSON: {e}"))?;
    let obj = v.as_object().ok_or("翻译 JSON 顶层必须是对象 {\"原文\":\"译文\"}")?;
    let mut flat = serde_json::Map::new();
    let mut skipped = 0usize;
    flatten_into(obj, &mut flat, &mut skipped, 0);
    let total = flat.len() + skipped;
    if flat.is_empty() {
        return Err("翻译 JSON 里没有任何有效条目（键=原文，值=非空译文）".into());
    }
    Ok((flat, total, skipped))
}

fn flatten_into(
    obj: &serde_json::Map<String, serde_json::Value>,
    out: &mut serde_json::Map<String, serde_json::Value>,
    skipped: &mut usize,
    depth: usize,
) {
    if depth > 4 {
        return; // 防御性：嵌套过深视为异常数据
    }
    for (k, v) in obj {
        match v {
            serde_json::Value::String(s) => {
                if s.trim().is_empty() {
                    *skipped += 1; // 空译文 = 未翻译，保留原文
                } else {
                    out.insert(k.clone(), serde_json::json!(s));
                }
            }
            serde_json::Value::Object(inner) => flatten_into(inner, out, skipped, depth + 1),
            _ => *skipped += 1, // 数字/布尔等非法值忽略
        }
    }
}

/// 向 plugins.js 文本追加插件条目（幂等由调用方保证）。
/// 兼容 `var $plugins = [ {...}, {...} ];` 的任意空白/换行风格。
pub fn patch_plugins_js(content: &str) -> Result<String, String> {
    let close = content.rfind(']').ok_or("plugins.js 结构异常：未找到结尾的 ]")?;
    let head = &content[..close];
    let tail = &content[close + 1..];
    let last_char = head.chars().rev().find(|c| !c.is_whitespace());
    let entry = format!(
        "    {{\"name\":\"{HOOK_NAME}\",\"status\":true,\"description\":\"STool MTool-style runtime translation (auto-generated)\",\"parameters\":{{}}}}"
    );
    let sep = match last_char {
        Some('[') | Some(',') => "\n", // 空数组或已有尾逗号，直接接条目
        _ => ",\n",                    // 常规情况：先补逗号
    };
    Ok(format!("{head}{sep}{entry}\n]{tail}"))
}

fn registered(content: &str) -> bool {
    content.contains(HOOK_NAME)
}

/// 注入：按引擎分派。plugin_id 为 engines 注册表里的插件 id。
pub fn install(root: &Path, json_path: &Path, plugin_id: &str) -> Result<String, String> {
    match plugin_id {
        "rpgmaker_mv" => install_mv(root, json_path),
        "renpy" => install_renpy(root, json_path),
        "html_game" => install_html(root, json_path),
        "tyrano" => install_tyrano(root, json_path),
        _ => Err(format!(
            "「{plugin_id}」暂不支持运行时 JSON 注入。当前注入范围：Ren'Py、RPG Maker MV/MZ、\
             TyranoBuilder、HTML/Electron（文本在 JS/DOM/脚本层、引擎留了运行时替换入口）。\
             其他引擎（KiriKiri/Siglus/BGI/RPG Maker 2000 等）的文本封在私有封包里，\
             请改用「文本提取 → 翻译 → 翻译回填/封包回写」这套离线流程；\
             其中 KiriKiri 系还可用运行时补丁包（CLI `stool xp3-patch`）免去重写原封包。"
        )),
    }
}

/// 卸载：按引擎分派，尽量字节级还原。
pub fn uninstall(root: &Path, plugin_id: &str) -> Result<String, String> {
    match plugin_id {
        "rpgmaker_mv" => uninstall_mv(root),
        "renpy" => uninstall_renpy(root),
        // TyranoBuilder 与 HTML 系的注入痕迹相同（入口 HTML + stool_translate.js + JSON）
        "html_game" | "tyrano" => uninstall_html(root),
        _ => Err("该引擎不支持注入，无需卸载".into()),
    }
}

/// 注入状态（供 GUI / CLI 展示）。
pub struct Status {
    pub supported: bool,   // 该引擎支持注入
    pub installed: bool,   // 已注入
    pub entries: usize,    // 翻译 JSON 的条目数
    pub json_path: String, // 当前生效的翻译 JSON 路径
}

pub fn status(root: &Path, plugin_id: &str) -> Status {
    let mut st = Status { supported: is_supported(plugin_id), installed: false, entries: 0, json_path: String::new() };
    match plugin_id {
        "rpgmaker_mv" => {
            let jsd = js_dir(root);
            st.installed = jsd
                .as_ref()
                .map(|d| std::fs::read_to_string(d.join("plugins.js")).map(|c| registered(&c)).unwrap_or(false))
                .unwrap_or(false);
            for f in [root.join(JSON_IN_GAME), root.join("translation.json")] {
                if f.exists() {
                    st.json_path = f.display().to_string();
                    if let Ok((_, total, _)) = load_map(&f) {
                        st.entries = total;
                    }
                    break;
                }
            }
        }
        "renpy" => {
            let f = renpy_game_dir(root).map(|g| g.join("stool_translate.rpy"));
            if let Some(f) = f {
                st.installed = f.exists();
                if st.installed {
                    // 从 rpy 里数条目（每个条目一行 "k": "v"）
                    if let Ok(c) = std::fs::read_to_string(&f) {
                        st.entries = c.lines().filter(|l| l.trim_start().starts_with('"')).count();
                    }
                    st.json_path = f.display().to_string();
                }
            }
        }
        "html_game" | "tyrano" => {
            let hook = root.join(format!("{HOOK_NAME}.js"));
            st.installed = hook.exists();
            let f = root.join(JSON_IN_GAME);
            if f.exists() {
                st.json_path = f.display().to_string();
                if let Ok((_, total, _)) = load_map(&f) {
                    st.entries = total;
                }
            }
        }
        _ => {}
    }
    st
}

// ---------------------------------------------------------------------------
// RPG Maker MV / MZ
// ---------------------------------------------------------------------------

/// 注入：写 Hook 插件 + 复制翻译 JSON 到游戏目录 + 注册到 plugins.js。
pub fn install_mv(root: &Path, json_path: &Path) -> Result<String, String> {
    let jsd = js_dir(root).ok_or("未找到 js/plugins.js（请确认这是 RPG Maker MV / MZ 游戏）")?;

    let (map, total, skipped) = load_map(json_path)?;

    // 1) Hook 插件
    let plugins_dir = jsd.join("plugins");
    std::fs::create_dir_all(&plugins_dir).map_err(|e| format!("创建 js/plugins 目录失败: {e}"))?;
    std::fs::write(plugins_dir.join(format!("{HOOK_NAME}.js")), HOOK_JS).map_err(|e| format!("写入插件失败: {e}"))?;

    // 2) 翻译 JSON 复制到游戏根目录（游戏端固定读游戏目录，便于整个文件夹携带）
    let json_text = serde_json::to_string_pretty(&serde_json::Value::Object(map)).map_err(|e| e.to_string())?;
    std::fs::write(root.join(JSON_IN_GAME), json_text).map_err(|e| format!("写入游戏目录翻译 JSON 失败: {e}"))?;

    // 3) 注册到 plugins.js（首次注入前备份原件）
    let plugins_js = jsd.join("plugins.js");
    let content = std::fs::read_to_string(&plugins_js).map_err(|e| format!("读取 plugins.js 失败: {e}"))?;
    if registered(&content) {
        return Ok(format!("注入已更新：{total} 条翻译（{skipped} 条空译文跳过）。重新启动游戏生效。"));
    }
    // 首次注入前备份原件（已存在则保留，绝不覆盖 → 保证备份永远是"原始状态"）
    crate::settings::backup_once(&plugins_js).map_err(|e| format!("备份 plugins.js 失败: {e}"))?;
    let patched = patch_plugins_js(&content)?;
    std::fs::write(&plugins_js, patched).map_err(|e| format!("写入 plugins.js 失败: {e}"))?;
    Ok(format!("注入完成：{total} 条翻译已挂载（{skipped} 条空译文跳过，保留原文）。重新启动游戏生效。"))
}

/// 卸载：从备份还原 plugins.js，删除 Hook 与注入进游戏目录的翻译 JSON。
pub fn uninstall_mv(root: &Path) -> Result<String, String> {
    let jsd = js_dir(root).ok_or("未找到 js/plugins.js")?;
    let plugins_js = jsd.join("plugins.js");
    let bak = crate::settings::backup_path_for(&plugins_js);

    let mut msgs: Vec<String> = Vec::new();
    if bak.exists() {
        let orig = std::fs::read_to_string(&bak).map_err(|e| format!("读取备份失败: {e}"))?;
        std::fs::write(&plugins_js, orig).map_err(|e| format!("还原 plugins.js 失败: {e}"))?;
        std::fs::remove_file(&bak).map_err(|e| format!("删除备份失败: {e}"))?;
        msgs.push("已从备份还原 plugins.js".into());
    } else if let Ok(content) = std::fs::read_to_string(&plugins_js) {
        // 无备份时退化为逐行剔除我们的条目
        let cleaned: Vec<&str> = content.lines().filter(|l| !l.contains(HOOK_NAME)).collect();
        std::fs::write(&plugins_js, cleaned.join("\n")).map_err(|e| format!("写入 plugins.js 失败: {e}"))?;
        msgs.push("已从 plugins.js 移除注入条目".into());
    }
    for p in [jsd.join("plugins").join(format!("{HOOK_NAME}.js")), root.join(JSON_IN_GAME)] {
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
    if msgs.is_empty() {
        Ok("本来就未注入".into())
    } else {
        Ok(format!("{}。重新启动游戏生效。", msgs.join("；")))
    }
}

// ---------------------------------------------------------------------------
// Ren'Py
// ---------------------------------------------------------------------------

/// Ren'Py 双引号字符串转义。
fn rpy_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\r', "").replace('\n', "\\n")
}

/// 注入：生成 game/stool_translate.rpy，利用 Ren'Py 的 config.replace_text 在
/// 文本显示前整句替换。只新增文件，不修改任何原始脚本；删除该文件即还原。
pub fn install_renpy(root: &Path, json_path: &Path) -> Result<String, String> {
    let game = renpy_game_dir(root).ok_or("未找到 game/ 目录（请确认这是 Ren'Py 游戏）")?;
    let (map, total, skipped) = load_map(json_path)?;
    let mut out = String::from(
        "# STool 运行时汉化注入（MTool 式）—— 由 STool 自动生成，删除本文件（及 .rpyc）即还原。\n\
         # 利用 Ren'Py 的 config.replace_text 在文本显示前整句替换；未命中的文本保留原文。\n\
         init python:\n    config.replace_text = {\n",
    );
    for (k, v) in &map {
        let v = v.as_str().unwrap_or_default();
        out.push_str(&format!("        \"{}\": \"{}\",\n", rpy_escape(k), rpy_escape(v)));
    }
    out.push_str("    }\n");
    std::fs::write(game.join("stool_translate.rpy"), out).map_err(|e| format!("写入 stool_translate.rpy 失败: {e}"))?;
    Ok(format!(
        "注入完成：{total} 条翻译已写入 game/stool_translate.rpy（{skipped} 条空译文跳过）。\
         首次启动游戏时 Ren'Py 会自动编译生效；删除文件或点“移除注入”即还原。"
    ))
}

/// 卸载：删除注入的 .rpy 及其编译产物 .rpyc。
pub fn uninstall_renpy(root: &Path) -> Result<String, String> {
    let game = renpy_game_dir(root).ok_or("未找到 game/ 目录")?;
    let mut removed = false;
    for f in [game.join("stool_translate.rpy"), game.join("stool_translate.rpyc")] {
        if f.exists() {
            std::fs::remove_file(&f).map_err(|e| format!("删除 {} 失败: {e}", f.display()))?;
            removed = true;
        }
    }
    if removed {
        Ok("已删除 stool_translate.rpy(.rpyc)，游戏还原为原始状态。".into())
    } else {
        Ok("本来就未注入".into())
    }
}

// ---------------------------------------------------------------------------
// HTML / Electron
// ---------------------------------------------------------------------------

/// 注入：写 stool_translate.js + 翻译 JSON，并在入口 HTML 末尾挂上脚本标签（原文件备份）。
pub fn install_html(root: &Path, json_path: &Path) -> Result<String, String> {
    install_html_like(root, json_path, HTML_HOOK_JS, "刷新/重启游戏生效。注意：Canvas 画面内的文本无法替换。")
}

/// 注入（TyranoBuilder）：机制与 HTML 系相同，但 Hook 额外挂了 jQuery `html/text` 兜底
/// —— TyranoScript 的 KAG 消息是用 jQuery 写进 DOM 的（不是 Canvas），DOM 替换即可覆盖。
pub fn install_tyrano(root: &Path, json_path: &Path) -> Result<String, String> {
    if !root.join("tyrano").is_dir() {
        return Err(format!(
            "未找到 tyrano/ 目录（{}）—— 请把游戏目录选成 TyranoBuilder 游戏的根目录（应同时含 index.html 与 tyrano/）",
            root.display()
        ));
    }
    install_html_like(
        root,
        json_path,
        TYRANO_HOOK_JS,
        "刷新/重启游戏生效。Tyrano 的对话文本在 DOM 里，可替换；若个别文本画在 Canvas 上则无法替换（可改用“文本提取 → 回填 .ks”）。",
    )
}

/// HTML 系（HTML/Electron 与 TyranoBuilder）的共用注入流程。
fn install_html_like(root: &Path, json_path: &Path, hook_js: &str, tail_hint: &str) -> Result<String, String> {
    let entry = html_entry(root).ok_or("未找到入口 HTML（index.html）")?;
    let (map, total, skipped) = load_map(json_path)?;

    // 1) Hook 与翻译 JSON
    let json_text = serde_json::to_string_pretty(&serde_json::Value::Object(map)).map_err(|e| e.to_string())?;
    std::fs::write(root.join(JSON_IN_GAME), json_text).map_err(|e| format!("写入翻译 JSON 失败: {e}"))?;
    std::fs::write(root.join(format!("{HOOK_NAME}.js")), hook_js).map_err(|e| format!("写入 Hook 失败: {e}"))?;

    // 2) 入口 HTML 挂脚本（幂等；首次备份）
    let html = std::fs::read_to_string(&entry).map_err(|e| format!("读取 {} 失败: {e}", entry.display()))?;
    if html.contains(&format!("{HOOK_NAME}.js")) {
        return Ok(format!("注入已更新：{total} 条翻译（{skipped} 条空译文跳过）。{tail_hint}"));
    }
    // 首次注入前备份入口 HTML（已存在则保留，绝不覆盖）
    crate::settings::backup_once(&entry).map_err(|e| format!("备份入口 HTML 失败: {e}"))?;
    let tag = format!("<script src=\"{HOOK_NAME}.js\"></script>");
    let patched = match html.to_ascii_lowercase().rfind("</body>") {
        Some(pos) => format!("{}{}{}", &html[..pos], tag, &html[pos..]),
        None => format!("{html}{tag}"),
    };
    std::fs::write(&entry, patched).map_err(|e| format!("写入入口 HTML 失败: {e}"))?;
    Ok(format!("注入完成：{total} 条翻译（{skipped} 条空译文跳过）。{tail_hint}"))
}

/// 卸载：还原入口 HTML，删除 Hook 与翻译 JSON。
pub fn uninstall_html(root: &Path) -> Result<String, String> {
    let entry = html_entry(root).ok_or("未找到入口 HTML")?;
    let bak = crate::settings::backup_path_for(&entry);
    let mut msgs: Vec<String> = Vec::new();
    if bak.exists() {
        let orig = std::fs::read_to_string(&bak).map_err(|e| format!("读取备份失败: {e}"))?;
        std::fs::write(&entry, orig).map_err(|e| format!("还原入口 HTML 失败: {e}"))?;
        std::fs::remove_file(&bak).map_err(|e| format!("删除备份失败: {e}"))?;
        msgs.push("已从备份还原入口 HTML".into());
    } else if let Ok(html) = std::fs::read_to_string(&entry) {
        let tag = format!("<script src=\"{HOOK_NAME}.js\"></script>");
        let cleaned = html.replace(&tag, "");
        std::fs::write(&entry, cleaned).map_err(|e| format!("写入入口 HTML 失败: {e}"))?;
        msgs.push("已移除脚本标签".into());
    }
    for p in [root.join(format!("{HOOK_NAME}.js")), root.join(JSON_IN_GAME)] {
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
    if msgs.is_empty() {
        Ok("本来就未注入".into())
    } else {
        Ok(format!("{}。刷新/重启游戏生效。", msgs.join("；")))
    }
}

/// 游戏端 Hook 插件（由 Rust 端原样写入 js/plugins/stool_translate.js）。
const HOOK_JS: &str = r#"/* STool 运行时汉化注入（MTool 式）—— 由 STool 自动生成，请勿手改
 * 读取顺序：<游戏目录>/stool_translate.json → translation.json
 * 格式：{ "原文": "译文" }（也允许分组嵌套，加载时自动拍平）
 * 规则：整句精确匹配；未命中映射的文本一律保留原文；不修改任何游戏文件。
 */
(function () {
    "use strict";
    var MAP = null;

    function flatten(obj, out) {
        out = out || {};
        for (var k in obj) {
            if (!Object.prototype.hasOwnProperty.call(obj, k)) continue;
            var v = obj[k];
            if (v && typeof v === "object") flatten(v, out);
            else if (typeof v === "string") out[k] = v;
        }
        return out;
    }

    function parseJsonText(txt) {
        try { return flatten(JSON.parse(txt.replace(/^\uFEFF/, ""))); } catch (e) { return null; }
    }

    function loadMap() {
        var files = ["stool_translate.json", "translation.json"];
        try {
            if (typeof require === "function" && typeof process !== "undefined" &&
                process.versions && process.versions.node && typeof location !== "undefined") {
                var fs = require("fs");
                var p = require("path");
                var root = decodeURIComponent(location.pathname).replace(/\\/g, "/");
                root = root.slice(0, root.lastIndexOf("/"));
                if (/^\/[A-Za-z]:/.test(root)) root = root.slice(1); // Windows 下 XHR 风格路径去掉开头斜杠
                for (var i = 0; i < files.length; i++) {
                    var f = p.join(root, files[i]);
                    if (fs.existsSync(f)) {
                        MAP = parseJsonText(fs.readFileSync(f, "utf8"));
                        if (MAP) { console.log("[STool] 汉化映射已加载: " + files[i] + " (" + Object.keys(MAP).length + " 条)"); return; }
                    }
                }
            }
        } catch (e) { console.warn("[STool] 本地读取失败，改用 XHR", e); }
        for (var j = 0; j < files.length; j++) {
            try {
                var xhr = new XMLHttpRequest();
                xhr.open("GET", files[j], false);
                xhr.overrideMimeType("application/json");
                xhr.send(null);
                if (xhr.status === 200 || (xhr.status === 0 && xhr.responseText)) {
                    MAP = parseJsonText(xhr.responseText);
                    if (MAP) { console.log("[STool] 汉化映射已加载(XHR): " + files[j]); return; }
                }
            } catch (e2) { /* 忽略，试下一个 */ }
        }
        console.warn("[STool] 未找到翻译 JSON，本次运行不做替换");
    }

    function tr(s) {
        if (!MAP || typeof s !== "string") return s;
        var m = MAP[s];
        return (typeof m === "string" && m.length > 0) ? m : s; // 未命中 → 保留原文
    }

    // 这些字段的字符串多半是文件路径/备注，替换会破坏资源加载，跳过
    var SKIP_KEYS = { characterName: 1, faceName: 1, battlerName: 1, note: 1 };
    function looksLikePath(s) {
        return /[\/\\]/.test(s) ||
            /\.(png|ogg|m4a|wav|mp3|jpg|jpeg|webp|json|txt|rvdata2?|rpgmvp|rpgmvo|rpgmvm|exe)$/i.test(s);
    }

    function walk(v, depth, seen) {
        if (depth > 8 || !v || typeof v !== "object") return;
        for (var i = 0; i < seen.length; i++) if (seen[i] === v) return; // 循环引用保护
        seen.push(v);
        if (Array.isArray(v)) {
            for (var a = 0; a < v.length; a++) {
                var e = v[a];
                if (e && typeof e === "object") walk(e, depth + 1, seen);
                else if (typeof e === "string") v[a] = tr(e);
            }
        } else {
            for (var k in v) {
                if (!Object.prototype.hasOwnProperty.call(v, k)) continue;
                var val = v[k];
                if (val && typeof val === "object") walk(val, depth + 1, seen);
                else if (typeof val === "string" && !SKIP_KEYS[k] && !looksLikePath(val)) v[k] = tr(val);
            }
        }
        seen.pop();
    }

    function walkDatabase() {
        var names = ["$dataActors", "$dataClasses", "$dataSkills", "$dataItems", "$dataWeapons",
            "$dataArmors", "$dataEnemies", "$dataTroops", "$dataStates", "$dataSystem",
            "$dataMapInfos", "$dataCommonEvents"];
        for (var i = 0; i < names.length; i++) {
            var d = window[names[i]];
            if (d) walk(d, 0, []);
        }
    }

    function installDataHooks() {
        // Scene_Boot.start 时全部数据库已加载（MV/MZ 一致）
        if (typeof Scene_Boot !== "undefined" && Scene_Boot.prototype && Scene_Boot.prototype.start) {
            var _sb = Scene_Boot.prototype.start;
            Scene_Boot.prototype.start = function () {
                var r = _sb.apply(this, arguments);
                try { walkDatabase(); } catch (e) { console.warn("[STool] 数据库替换出错", e); }
                return r;
            };
        }
        // 每张地图的事件文本（对话 401 / 选项 102 / 显示滚动文字等）
        if (typeof Scene_Map !== "undefined" && Scene_Map.prototype && Scene_Map.prototype.start) {
            var _sms = Scene_Map.prototype.start;
            Scene_Map.prototype.start = function () {
                var r = _sms.apply(this, arguments);
                try { if (typeof $dataMap !== "undefined" && $dataMap) walk($dataMap, 0, []); } catch (e) {}
                return r;
            };
        }
    }

    // 动态文本（脚本临时拼出来的对话等）显示层兜底；MV/MZ 同签名，改 arguments[0] 即可
    function installTextHooks() {
        if (typeof Window_Base === "undefined") return;
        if (Window_Base.prototype.drawText) {
            var _dt = Window_Base.prototype.drawText;
            Window_Base.prototype.drawText = function () {
                if (arguments.length > 0) arguments[0] = tr(arguments[0]);
                return _dt.apply(this, arguments);
            };
        }
        if (Window_Base.prototype.drawTextEx) {
            var _dtx = Window_Base.prototype.drawTextEx;
            Window_Base.prototype.drawTextEx = function () {
                if (arguments.length > 0) arguments[0] = tr(arguments[0]);
                return _dtx.apply(this, arguments);
            };
        }
    }

    loadMap();
    if (MAP) {
        if (typeof DataManager !== "undefined" || typeof Scene_Boot !== "undefined") installDataHooks();
        installTextHooks();
    }
})();
"#;

/// HTML/Electron 游戏端 Hook：DOM 文本节点运行时替换（MutationObserver）。
const HTML_HOOK_JS: &str = r#"/* STool HTML 运行时汉化注入 —— 由 STool 自动生成，请勿手改
 * 读取：<入口 HTML 同目录>/stool_translate.json（{"原文":"译文"}，允许分组嵌套）
 * 规则：整句精确匹配；未命中保留原文。注意：Canvas 画面内的文本无法替换。
 */
(function () {
    "use strict";
    var MAP = null;

    function flatten(obj, out) {
        out = out || {};
        for (var k in obj) {
            if (!Object.prototype.hasOwnProperty.call(obj, k)) continue;
            var v = obj[k];
            if (v && typeof v === "object") flatten(v, out);
            else if (typeof v === "string") out[k] = v;
        }
        return out;
    }

    function parseJsonText(txt) {
        try { return flatten(JSON.parse(txt.replace(/^\uFEFF/, ""))); } catch (e) { return null; }
    }

    function loadMap() {
        var files = ["stool_translate.json", "translation.json"];
        try {
            if (typeof require === "function" && typeof process !== "undefined" &&
                process.versions && process.versions.node && typeof location !== "undefined") {
                var fs = require("fs");
                var p = require("path");
                var root = decodeURIComponent(location.pathname).replace(/\\/g, "/");
                root = root.slice(0, root.lastIndexOf("/"));
                if (/^\/[A-Za-z]:/.test(root)) root = root.slice(1);
                for (var i = 0; i < files.length; i++) {
                    var f = p.join(root, files[i]);
                    if (fs.existsSync(f)) {
                        MAP = parseJsonText(fs.readFileSync(f, "utf8"));
                        if (MAP) { console.log("[STool] 汉化映射已加载: " + files[i] + " (" + Object.keys(MAP).length + " 条)"); return; }
                    }
                }
            }
        } catch (e) { /* 转XHR */ }
        for (var j = 0; j < files.length; j++) {
            try {
                var xhr = new XMLHttpRequest();
                xhr.open("GET", files[j], true);
                xhr.overrideMimeType("application/json");
                xhr.onload = function () {
                    if (xhr.status === 200 || (xhr.status === 0 && xhr.responseText)) {
                        MAP = parseJsonText(xhr.responseText);
                        if (MAP && document.readyState !== "loading") start();
                    }
                };
                xhr.send(null);
                break; // 异步只挂第一个候选，未命中就不再尝试
            } catch (e2) { /* 忽略 */ }
        }
        if (!MAP) console.warn("[STool] 未找到翻译 JSON，本次运行不做替换");
    }

    function tr(s) {
        if (!MAP || typeof s !== "string") return s;
        var m = MAP[s];
        return (typeof m === "string" && m.length > 0) ? m : s; // 未命中 → 保留原文
    }

    function translateNode(n) {
        var t = n.nodeValue;
        if (typeof t !== "string" || !t.trim()) return;
        var m = MAP[t] !== undefined ? MAP[t] : MAP[t.trim()];
        if (typeof m === "string" && m.length > 0 && n.nodeValue !== m) n.nodeValue = m;
    }

    function walkDom(root) {
        try {
            var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null, false);
            var nodes = [];
            while (walker.nextNode()) nodes.push(walker.currentNode);
            nodes.forEach(translateNode);
        } catch (e) { /* 忽略 */ }
    }

    function start() {
        if (!MAP) return;
        walkDom(document.body || document.documentElement || document);
        var mo = new MutationObserver(function (muts) {
            muts.forEach(function (m) {
                if (m.type === "characterData" && m.target.nodeType === 3) translateNode(m.target);
                m.addedNodes.forEach(function (n) {
                    if (n.nodeType === 3) translateNode(n);
                    else if (n.nodeType === 1) walkDom(n);
                });
            });
        });
        mo.observe(document.documentElement || document, { childList: true, subtree: true, characterData: true });
    }

    loadMap();
    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", function () { start(); });
    } else {
        start();
    }
})();
"#;

/// TyranoBuilder / TyranoScript 游戏端 Hook。
///
/// 与 HTML Hook 的差别（Tyrano 是 jQuery + KAG 体系）：
/// 1. 保留 MutationObserver 的 DOM 文本节点替换（KAG 消息最终落到 DOM，不是 Canvas）；
/// 2. **额外**包装 jQuery 的 `$.fn.html` / `$.fn.text`，在 Tyrano 用 jQuery 写消息时做整串替换
///    —— 这能覆盖"整段文本被一次性 html() 写入"导致文本节点被 `<br>` 拆开、单节点匹配不中的情况；
/// 3. 对带 `message`/`text` 类名的元素做元素级 textContent 兜底匹配。
const TYRANO_HOOK_JS: &str = r#"/* STool TyranoBuilder 运行时汉化注入 —— 由 STool 自动生成，请勿手改
 * 读取：<index.html 同目录>/stool_translate.json（{"原文":"译文"}，允许分组嵌套）
 * 规则：整句精确匹配；未命中保留原文；不修改任何游戏文件（仅本脚本 + JSON 是新增的）。
 * 覆盖：DOM 文本节点（MutationObserver）+ jQuery html/text + message 元素 textContent。
 * 注意：Tyrano 若把文字画进 Canvas，则不在覆盖范围内。
 */
(function () {
    "use strict";
    var MAP = null;
    var DONE = false;

    function flatten(obj, out) {
        out = out || {};
        for (var k in obj) {
            if (!Object.prototype.hasOwnProperty.call(obj, k)) continue;
            var v = obj[k];
            if (v && typeof v === "object") flatten(v, out);
            else if (typeof v === "string") out[k] = v;
        }
        return out;
    }

    function parseJsonText(txt) {
        try { return flatten(JSON.parse(txt.replace(/^\uFEFF/, ""))); } catch (e) { return null; }
    }

    function loadMap() {
        var files = ["stool_translate.json", "translation.json"];
        // Electron / node 环境：直接用 fs 同步读，兼容路径里的中文
        try {
            if (typeof require === "function" && typeof process !== "undefined" &&
                process.versions && process.versions.node && typeof location !== "undefined") {
                var fs = require("fs");
                var p = require("path");
                var root = decodeURIComponent(location.pathname).replace(/\\/g, "/");
                root = root.slice(0, root.lastIndexOf("/"));
                if (/^\/[A-Za-z]:/.test(root)) root = root.slice(1);
                for (var i = 0; i < files.length; i++) {
                    var f = p.join(root, files[i]);
                    if (fs.existsSync(f)) {
                        MAP = parseJsonText(fs.readFileSync(f, "utf8"));
                        if (MAP) { console.log("[STool] Tyrano 汉化映射已加载: " + files[i]); return; }
                    }
                }
            }
        } catch (e) { /* 转 XHR */ }
        for (var j = 0; j < files.length; j++) {
            try {
                var xhr = new XMLHttpRequest();
                xhr.open("GET", files[j], false);
                xhr.overrideMimeType("application/json");
                xhr.send(null);
                if (xhr.status === 200 || (xhr.status === 0 && xhr.responseText)) {
                    MAP = parseJsonText(xhr.responseText);
                    if (MAP) return;
                }
            } catch (e2) { /* 试下一个 */ }
        }
        if (!MAP) console.warn("[STool] 未找到翻译 JSON，本次运行不做替换");
    }

    function tr(s) {
        if (!MAP || typeof s !== "string") return s;
        var m = MAP[s];
        if (typeof m === "string" && m.length > 0) return m;
        var t = s.trim();
        if (t !== s) {
            var m2 = MAP[t];
            if (typeof m2 === "string" && m2.length > 0) return m2;
        }
        return s;
    }

    // ---- 1) DOM 文本节点 ----

    function translateNode(n) {
        var t = n.nodeValue;
        if (typeof t !== "string" || !t.trim()) return;
        var m = tr(t);
        if (m !== t) n.nodeValue = m;
    }

    function walkDom(root) {
        if (!root) return;
        try {
            var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null, false);
            var nodes = [];
            while (walker.nextNode()) nodes.push(walker.currentNode);
            nodes.forEach(translateNode);
        } catch (e) { /* 忽略 */ }
    }

    // ---- 2) message 元素级兜底（文本被 <br> 拆成多节点时） ----

    function translateElem(el) {
        if (!el || el.nodeType !== 1) return;
        var cls = (el.className && typeof el.className === "string") ? el.className : "";
        if (!/(^|\s|-)(message|text|name|serif|glink|tyrano)(\s|-|$)/i.test(cls)) return;
        var t = el.textContent;
        if (typeof t !== "string" || !t.trim()) return;
        var m = tr(t);
        if (m !== t) el.textContent = m;
    }

    function walkElems(root) {
        if (!root || root.nodeType !== 1) return;
        var list = root.querySelectorAll ? root.querySelectorAll(".message,[class*=message]") : [];
        for (var i = 0; i < list.length; i++) translateElem(list[i]);
    }

    // ---- 3) jQuery html/text 包装（Tyrano KAG 写消息的主通道） ----

    function installJqHook() {
        if (typeof window.jQuery !== "function") return;
        var fn = window.jQuery.fn;
        ["html", "text"].forEach(function (name) {
            if (typeof fn[name] !== "function" || fn[name].__stool) return;
            var orig = fn[name];
            var wrapped = function (v) {
                // 只有"写入"（带参数）且是字符串时才尝试替换；读取原样透传
                if (arguments.length > 0 && typeof v === "string") {
                    var m = tr(v);
                    if (m !== v) arguments[0] = m;
                }
                return orig.apply(this, arguments);
            };
            wrapped.__stool = true;
            fn[name] = wrapped;
        });
    }

    function start() {
        if (DONE || !MAP) return;
        DONE = true;
        walkDom(document.body || document.documentElement || document);
        walkElems(document.body || document.documentElement || document);
        var mo = new MutationObserver(function (muts) {
            muts.forEach(function (m) {
                if (m.type === "characterData" && m.target.nodeType === 3) translateNode(m.target);
                m.addedNodes.forEach(function (n) {
                    if (n.nodeType === 3) translateNode(n);
                    else if (n.nodeType === 1) { walkDom(n); walkElems(n); }
                });
            });
        });
        mo.observe(document.documentElement || document, { childList: true, subtree: true, characterData: true });
    }

    loadMap();
    // jQuery 可能比本脚本晚加载（脚本顺序不保证），DOMContentLoaded 再补挂一次
    installJqHook();
    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", function () { installJqHook(); start(); });
    } else {
        installJqHook();
        start();
    }
})();
"#;

/// 从文本提取的 CSV 生成注入 JSON 骨架（注入 JSON 的"原文"来源）：
/// 键 = CSV 的 source 列原文；值 = 已有译文（translation 列非空时带入），否则留空串待机翻。
/// 同一原文去重（已有译文的行优先于空译文行）。
/// 返回 (总条数, 预填译文条数)。
pub fn csv_to_json_skeleton(csv_path: &Path, json_path: &Path) -> Result<(usize, usize), String> {
    let rows = crate::features::text::read_csv(csv_path)?;
    if rows.is_empty() {
        return Err("CSV 为空（请先做文本提取）".into());
    }
    // 先空后译：先放空串占位，再被带译文的行覆盖，保证"有译文的原文"优先
    let mut map = serde_json::Map::new();
    for r in &rows {
        let src = r[3].trim();
        if !src.is_empty() {
            map.insert(src.to_string(), serde_json::json!(""));
        }
    }
    let mut filled = 0usize;
    for r in &rows {
        let src = r[3].trim();
        let trans = r[4].trim();
        if !src.is_empty() && !trans.is_empty() && map.get(src).map(|v| v.as_str() == Some("")).unwrap_or(false) {
            map.insert(src.to_string(), serde_json::json!(trans));
            filled += 1;
        }
    }
    let total = map.len();
    let txt = serde_json::to_string_pretty(&serde_json::Value::Object(map)).map_err(|e| e.to_string())?;
    crate::settings::ensure_parent(json_path);
    std::fs::write(json_path, txt).map_err(|e| format!("写入 JSON 失败: {e}"))?;
    Ok((total, filled))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "// Generated by RPG Maker.\n// Do not edit this file directly.\nvar $plugins =\n[\n    {\"name\":\"Core\",\"status\":true,\"description\":\"Core\",\"parameters\":{}},\n    {\"name\":\"MadeWithMv\",\"status\":true,\"description\":\"x\",\"parameters\":{}}\n];\n";

    #[test]
    fn test_patch_plugins_js() {
        let out = patch_plugins_js(SAMPLE).unwrap();
        assert!(out.contains("\"name\":\"stool_translate\""));
        assert!(out.trim_end().ends_with(';'));
        // 原有条目之间补了逗号，且仍以 ] 结尾
        let close = out.rfind(']').unwrap();
        assert!(out[..close].trim_end().ends_with('}'));
    }

    #[test]
    fn test_patch_plugins_js_empty_array() {
        let empty = "var $plugins =\n[\n];\n";
        let out = patch_plugins_js(empty).unwrap();
        assert!(out.contains("stool_translate"));
        assert!(!out.contains(",\n    {\"name\":\"stool_translate\"") || out.contains("[\n"));
    }

    #[test]
    fn test_patch_plugins_js_no_array() {
        assert!(patch_plugins_js("var x = 1;").is_err());
    }

    #[test]
    fn test_load_map_flat_and_nested() {
        let dir = std::env::temp_dir().join(format!("stool_inject_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.json");
        std::fs::write(
            &p,
            "\u{feff}{\n  \"你好\": \"你好呀\",\n  \"战斗\": { \"攻击\": \"打击\" },\n  \"空译\": \"\",\n  \"数字\": 42\n}",
        )
        .unwrap();
        let (map, total, skipped) = load_map(&p).unwrap();
        assert_eq!(map.get("你好").unwrap(), "你好呀");
        assert_eq!(map.get("攻击").unwrap(), "打击"); // 嵌套拍平，组名忽略
        assert_eq!(map.len(), 2);
        assert_eq!(total, 4);
        assert_eq!(skipped, 2); // 空译文 + 非字符串
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_map_invalid() {
        let dir = std::env::temp_dir().join(format!("stool_inject_test2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.json");
        std::fs::write(&p, "[1,2,3]").unwrap();
        assert!(load_map(&p).is_err()); // 顶层必须是对象
        std::fs::write(&p, "{\"a\":\"\"}").unwrap();
        assert!(load_map(&p).is_err()); // 全是空译文 = 无有效条目
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_install_uninstall_roundtrip() {
        let game = std::env::temp_dir().join(format!("stool_inject_game_{}", std::process::id()));
        let jsd = game.join("www").join("js");
        std::fs::create_dir_all(&jsd).unwrap();
        std::fs::write(jsd.join("plugins.js"), SAMPLE).unwrap();
        let json = game.join("my_trans.json");
        std::fs::write(&json, "{\"Hello\":\"你好\"}").unwrap();

        // 安装
        let msg = install(&game, &json, "rpgmaker_mv").unwrap();
        assert!(msg.contains("1 条"));
        assert!(jsd.join("plugins").join("stool_translate.js").exists());
        assert!(game.join(JSON_IN_GAME).exists());
        let patched = std::fs::read_to_string(jsd.join("plugins.js")).unwrap();
        assert!(patched.contains("stool_translate"));
        assert!(patched.trim_end().ends_with(';'));
        assert!(patched.contains(']'));
        // 备份 = 原始内容
        let bak = std::fs::read_to_string(jsd.join("plugins.js.stool.bak")).unwrap();
        assert_eq!(bak, SAMPLE);
        // 幂等：重复注入不追加第二条
        let again = install(&game, &json, "rpgmaker_mv").unwrap();
        assert!(again.contains("已更新"));
        let patched2 = std::fs::read_to_string(jsd.join("plugins.js")).unwrap();
        assert_eq!(patched2.matches("stool_translate").count(), 1);

        // 状态
        let st = status(&game, "rpgmaker_mv");
        assert!(st.supported && st.installed && st.entries == 1);

        // 卸载：字节级还原
        uninstall(&game, "rpgmaker_mv").unwrap();
        assert_eq!(std::fs::read_to_string(jsd.join("plugins.js")).unwrap(), SAMPLE);
        assert!(!jsd.join("plugins").join("stool_translate.js").exists());
        assert!(!jsd.join("plugins.js.stool.bak").exists());

        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn test_renpy_install_uninstall() {
        let game = std::env::temp_dir().join(format!("stool_inject_renpy_{}", std::process::id()));
        std::fs::create_dir_all(game.join("game")).unwrap();
        let json = game.join("trans.json");
        std::fs::write(&json, "{\"Hello \\\"world\\\"\":\"你好\",\"line1\\nline2\":\"一\\n二\"}").unwrap();

        let msg = install(&game, &json, "renpy").unwrap();
        assert!(msg.contains("2 条"));
        let rpy = std::fs::read_to_string(game.join("game").join("stool_translate.rpy")).unwrap();
        assert!(rpy.contains("config.replace_text"));
        // 转义正确：反斜杠与引号都被转义，换行变成 \n 字面量
        assert!(rpy.contains("\"Hello \\\"world\\\"\": \"你好\""));
        assert!(rpy.contains("\"line1\\nline2\""));
        // 幂等（覆盖写）
        install(&game, &json, "renpy").unwrap();
        let st = status(&game, "renpy");
        assert!(st.supported && st.installed && st.entries == 2);

        // 不支持引擎的报错
        assert!(install(&game, &json, "kirikiri").unwrap_err().contains("暂不支持"));

        uninstall(&game, "renpy").unwrap();
        assert!(!game.join("game").join("stool_translate.rpy").exists());
        let st2 = status(&game, "renpy");
        assert!(!st2.installed);

        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn test_html_install_uninstall() {
        let game = std::env::temp_dir().join(format!("stool_inject_html_{}", std::process::id()));
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("index.html"), "<html><body><div>hello</div></body></html>").unwrap();
        let json = game.join("trans.json");
        std::fs::write(&json, "{\"hello\":\"你好\"}").unwrap();

        let msg = install(&game, &json, "html_game").unwrap();
        assert!(msg.contains("1 条"));
        assert!(game.join("stool_translate.js").exists());
        assert!(game.join(JSON_IN_GAME).exists());
        let html = std::fs::read_to_string(game.join("index.html")).unwrap();
        assert!(html.contains("<script src=\"stool_translate.js\"></script></body>"));
        // 备份为原始 HTML
        assert_eq!(std::fs::read_to_string(game.join("index.html.stool.bak")).unwrap(), "<html><body><div>hello</div></body></html>");
        // 幂等
        install(&game, &json, "html_game").unwrap();
        let html2 = std::fs::read_to_string(game.join("index.html")).unwrap();
        assert_eq!(html2.matches("stool_translate.js").count(), 1);
        let st = status(&game, "html_game");
        assert!(st.supported && st.installed && st.entries == 1);

        uninstall(&game, "html_game").unwrap();
        assert_eq!(std::fs::read_to_string(game.join("index.html")).unwrap(), "<html><body><div>hello</div></body></html>");
        assert!(!game.join("index.html.stool.bak").exists());
        assert!(!game.join("stool_translate.js").exists());

        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn test_csv_to_json_skeleton() {
        let dir = std::env::temp_dir().join(format!("stool_csv2json_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let csv = dir.join("text.csv");
        let out = dir.join("translation.json");
        crate::features::text::write_csv_rows(
            &csv,
            &[
                ["1".into(), "f".into(), "ctx".into(), "こんにちは".into(), "你好".into()],
                ["2".into(), "f".into(), "ctx".into(), "選択肢".into(), "".into()],
                ["3".into(), "f".into(), "ctx".into(), "こんにちは".into(), "再次翻译".into()],
                ["4".into(), "f".into(), "ctx".into(), "".into(), "空原文跳过".into()],
            ],
        )
        .unwrap();
        let (total, filled) = csv_to_json_skeleton(&csv, &out).unwrap();
        assert_eq!((total, filled), (2, 1)); // 去重后 2 条原文；こんにちは 已带译文
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(v["こんにちは"], "你好"); // 已有译文优先带入
        assert_eq!(v["選択肢"], ""); // 空译文留空待机翻
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tyrano_install_uninstall() {
        let game = std::env::temp_dir().join(format!("stool_inject_tyrano_{}", std::process::id()));
        std::fs::create_dir_all(game.join("tyrano").join("plugins")).unwrap();
        std::fs::create_dir_all(game.join("data").join("scenario")).unwrap();
        std::fs::write(
            game.join("index.html"),
            "<html><body><div class=\"message\">こんにちは</div></body></html>",
        )
        .unwrap();
        let json = game.join("trans.json");
        std::fs::write(&json, "{\"こんにちは\":\"你好\"}").unwrap();

        // 目录选错（缺 tyrano/）应明确报错，而不是往错的目录注入
        let wrong = game.join("data");
        let err = install(&wrong, &json, "tyrano").unwrap_err();
        assert!(err.contains("tyrano/"), "报错应点明缺 tyrano/ 目录，实得: {err}");

        let msg = install(&game, &json, "tyrano").unwrap();
        assert!(msg.contains("1 条"));
        assert!(game.join("stool_translate.js").exists());
        assert!(game.join(JSON_IN_GAME).exists());
        let hook = std::fs::read_to_string(game.join("stool_translate.js")).unwrap();
        assert!(hook.contains("jQuery"), "Tyrano Hook 应带 jQuery 兜底");
        let html = std::fs::read_to_string(game.join("index.html")).unwrap();
        assert!(html.contains("<script src=\"stool_translate.js\"></script></body>"));
        assert!(std::fs::read_to_string(game.join("index.html.stool.bak")).unwrap().contains("こんにちは"));
        // 幂等：重复注入不重复挂标签
        install(&game, &json, "tyrano").unwrap();
        let html2 = std::fs::read_to_string(game.join("index.html")).unwrap();
        assert_eq!(html2.matches("stool_translate.js").count(), 1);
        let st = status(&game, "tyrano");
        assert!(st.supported && st.installed && st.entries == 1);

        uninstall(&game, "tyrano").unwrap();
        assert!(!game.join("stool_translate.js").exists());
        assert!(!game.join("index.html.stool.bak").exists());
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn test_support_table_consistent() {
        for id in SUPPORTED {
            assert!(support_of(id).is_some(), "{id} 在 SUPPORTED 里却不在 SUPPORT_TABLE");
        }
        assert_eq!(SUPPORT_TABLE.len(), SUPPORTED.len(), "两表条目数应一致");
        assert!(is_supported("tyrano") && is_supported("renpy"));
        // 封包型引擎明确不在注入范围内
        assert!(!is_supported("kirikiri") && !is_supported("rpgmaker_rgss"));
    }
}
