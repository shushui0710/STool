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
//!   发行版常把整个前端打进 `resources/app.asar`（游戏根目录只剩 Electron 运行时）—— 此时
//!   会**直接改写 asar**：在其入口页尾部挂标签，并把 hook 与翻译 JSON 写进 asar 根
//!   （原档案整文件备份，可一键还原）。asar 的数据区是各条目紧凑拼接的，所以改写只让入口页
//!   之后的条目整体平移，其余字节原样保留（实现见 `formats::asar::patch_archive`）。
//!   入口页的挑选是**收敛的**：优先 `index.html`，其次 asar 内的入口页，最后根目录里
//!   「像入口」的 `.html`（排除许可证/说明类，且必须加载脚本）—— 找不到就报错，绝不随便挑一个
//!   文件注入（曾经的兜底是「根目录第一个 .html」，在 Electron 包上会命中
//!   `LICENSES.chromium.html`，表现就是「提示成功但界面毫无变化」）。
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
//!
//! **只替换「给人看的文字」**（MV/MZ 侧口径，见 HOOK_JS 的 SKIP_KEYS / TEXT_PARAM）：
//! - 资源名字段一律跳过：`title1Name`/`title2Name`/`faceName`/`characterName`/`battlerName`/
//!   `battleback*Name`/`parallaxName`/`tilesetName`，以及音频描述符 `{name,volume,...}` 的 `name`；
//! - 事件指令只替换文字参数（401/405 正文、102/402 选项、101 说话人、320/324/325 名字），
//!   其余指令（显示图片 231/232、播放音频 241/245/249/250、脚本 355/655 等）原样不动。
//!
//! 原因是实测出来的坑：翻译表的「原文」常与游戏资源文件名同形（本仓验证用的那个游戏里
//! 有 158 个撞名，如 `タイトル画面` → 标题画面、`スチル1` → 静态图像1）。早先的实现只按
//! 「像不像路径」判断，而 `タイトル画面` 没扩展名、没过 `looksLikePath`，于是被整句替换，
//! `$dataSystem.title1Name` 变成中文，游戏去找 `img/titles1/标题画面.png` 当场报错。
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
        mechanism: "在 index.html 追加 stool_translate.js；DOM 文本节点 + jQuery html/text + message 元素三级替换，并接管 v6 的 buildMessageHTML（逐字 span 正文的明文入口）；游戏被 Electron 打进 app.asar 时（磁盘上没有 tyrano/，特征只在包内），自动改写 asar 内的入口页并把注入文件写进包内（与 HTML/Electron 共用同一条 asar 管道，产物完全一致）",
        limits: "只改入口 HTML，或 asar 内新增的两个文件（原文件/原档案自动备份、可一键还原）；Canvas/WebGL 里画的文字无法替换；已打包的 .ks 剧本请改用文本提取/回填",
    },
    InjectSupport {
        plugin_id: "html_game",
        engine: "HTML / Electron",
        mechanism: "在入口 HTML 追加 stool_translate.js，MutationObserver 替换 DOM 文本节点；若页面是 TyranoScript v6，另接管 buildMessageHTML 处理逐字 span 正文；Electron（resources/app.asar）改写 asar 内的入口页并把 hook/JSON 写进 asar",
        limits: "只改入口 HTML 与 asar 内新增的两个文件（原档案自动备份、可一键还原）；Canvas 渲染的文字无法替换；asar 只增不减，还原靠首次注入前的整文件备份",
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

/// HTML 系注入的目标：磁盘上的入口 HTML，或 Electron `app.asar` 内的条目。
#[derive(Debug)]
enum HtmlTarget {
    /// 磁盘上的入口 HTML 文件。脚本与 JSON 写在它同目录（即游戏根）。
    File(PathBuf),
    /// Electron asar 内的入口条目。`entry` 是 asar 根下的相对路径（如 `index.html`）；
    /// 脚本与 JSON 都写进 asar 根 —— 页面本身从 asar 加载，相对路径才会落回 asar 里。
    Asar { asar: PathBuf, entry: String },
}

/// 明显是**非入口**的 HTML 名（许可证 / 说明 / 变更记录一类）。
///
/// Electron 打包的游戏会在根目录放 `LICENSES.chromium.html`。早先的兜底是
/// 「根目录第一个 .html」，于是脚本标签被注进了 Chromium 的许可证文件 —— 游戏永远不会加载它，
/// 现象就是「提示注入成功，但界面毫无变化」。所以非入口名必须先排除。
const NON_ENTRY_HTML: [&str; 12] = [
    "license", "licenses", "licence", "licences", "copying", "notice",
    "readme", "changelog", "changes", "history", "third_party", "third-party",
];

fn is_non_entry_html_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let stem = lower
        .strip_suffix(".html")
        .or_else(|| lower.strip_suffix(".htm"))
        .unwrap_or(&lower);
    let norm: String = stem.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    NON_ENTRY_HTML.iter().any(|bad| {
        let b: String = bad.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        norm == b || norm.starts_with(&b)
    })
}

fn is_html_name(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".html") || l.ends_with(".htm")
}

/// 入口 HTML 的内容判据：**必须加载脚本**。
///
/// 单看 `<html>` / `<body>` 判别力不够（许可证页也有），但游戏入口 —— Tyrano / RPG Maker web /
/// Construct / 自研前端 —— 无一例外都要 `<script>`，而 Chromium 的 `LICENSES.chromium.html`
/// 一个都没有。用它当「像不像入口」的硬门槛，实测能干净挡住那个文件。
fn looks_like_entry_html(content: &str) -> bool {
    content.to_ascii_lowercase().contains("<script")
}

/// 根目录里「像入口」的 `.html`（排除许可证/说明类，且内容须加载脚本）。
fn plain_html_entry(root: &Path) -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|d| d.path())
        .filter(|p| {
            p.is_file()
                && p.file_name().and_then(|n| n.to_str()).map(is_html_name).unwrap_or(false)
                && !p.file_name().and_then(|n| n.to_str()).map(is_non_entry_html_name).unwrap_or(false)
        })
        .collect();
    cands.sort();
    cands.into_iter().find(|p| {
        std::fs::read_to_string(p).map(|c| looks_like_entry_html(&c)).unwrap_or(false)
    })
}

/// 候选 asar（优先级实现只有一份：`engines::scan::asar_candidates`）。
///
/// 检测与注入必须共用**同一份**顺序 —— 各写一套必然出现「检测认这个包、注入改那个包」，
/// 而症状是「提示注入成功、游戏毫无变化」。
fn asar_candidates(root: &Path) -> Vec<PathBuf> {
    crate::engines::scan::asar_candidates(root)
}

/// asar 内可用的根级入口 HTML 条目名（优先 `index.html`）。
fn asar_entry_of(asar: &Path) -> Result<Option<String>, String> {
    let mut src = crate::formats::source::Source::open(asar, crate::formats::source::MAX_ARCHIVE)?;
    let (files, data_start) = crate::formats::asar::parse_index(&mut src)?;
    if files.contains_key("index.html") {
        return Ok(Some("index.html".to_string()));
    }
    let mut names: Vec<String> = files
        .keys()
        .filter(|k| !k.contains('/') && is_html_name(k) && !is_non_entry_html_name(k))
        .cloned()
        .collect();
    names.sort();
    for n in names {
        let node = &files[&n];
        if let Ok(bytes) = crate::formats::asar::read_entry(&mut src, data_start, node) {
            if looks_like_entry_html(&String::from_utf8_lossy(&bytes)) {
                return Ok(Some(n));
            }
        }
    }
    Ok(None)
}

/// 定位该游戏的 HTML 注入目标。顺序：
/// 1. 根目录 `index.html`（绝大多数明文 HTML 游戏）；
/// 2. **Electron asar 内的入口页**（发行版把整个前端打进 asar，根目录只剩 Electron 运行时，
///    此时唯一能让注入真正生效的位置就是 asar 里的入口页）；
/// 3. 根目录里其它「像入口」的 `.html`；
/// 4. 都不行则返回**带出路**的错误，而不是随便挑一个文件（旧行为正是栽在这里）。
fn resolve_html_target(root: &Path) -> Result<HtmlTarget, String> {
    let idx = root.join("index.html");
    if idx.exists() {
        return Ok(HtmlTarget::File(idx));
    }

    let mut asar_notes: Vec<String> = Vec::new();
    for a in asar_candidates(root) {
        match asar_entry_of(&a) {
            Ok(Some(entry)) => return Ok(HtmlTarget::Asar { asar: a, entry }),
            Ok(None) => asar_notes.push(format!("{} 内没有可用的根级入口 HTML", a.display())),
            Err(e) => asar_notes.push(format!("{} 读取失败（{e}）", a.display())),
        }
    }

    if let Some(f) = plain_html_entry(root) {
        return Ok(HtmlTarget::File(f));
    }

    let mut msg = String::from(
        "未找到入口 HTML：根目录没有 index.html，也没有「像入口」的 .html（已排除许可证/说明类文件）",
    );
    if !asar_notes.is_empty() {
        msg.push_str("；asar 情况：");
        msg.push_str(&asar_notes.join("；"));
    }
    msg.push_str("。若这是 Electron 打包的游戏，请确认 resources/app.asar 存在（本工具会直接改 asar 内的入口页，原档案自动备份）。");
    Err(msg)
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
        "html_game" | "tyrano" => match resolve_html_target(root) {
            Ok(HtmlTarget::File(entry)) => {
                let dir = entry.parent().unwrap_or(root);
                st.installed = dir.join(format!("{HOOK_NAME}.js")).exists();
                let f = dir.join(JSON_IN_GAME);
                if f.exists() {
                    st.json_path = f.display().to_string();
                    if let Ok((_, total, _)) = load_map(&f) {
                        st.entries = total;
                    }
                }
            }
            Ok(HtmlTarget::Asar { asar, .. }) => {
                st.installed = read_asar_named(&asar, &format!("{HOOK_NAME}.js"))
                    .map(|v| v.is_some())
                    .unwrap_or(false);
                if let Ok(Some(bytes)) = read_asar_named(&asar, JSON_IN_GAME) {
                    st.json_path = format!("{}\\{JSON_IN_GAME}", asar.display());
                    if let Ok(serde_json::Value::Object(obj)) = serde_json::from_slice(&bytes) {
                        let mut flat = serde_json::Map::new();
                        let mut skipped = 0usize;
                        flatten_into(&obj, &mut flat, &mut skipped, 0);
                        st.entries = flat.len() + skipped;
                    }
                }
            }
            Err(_) => {
                st.installed = root.join(format!("{HOOK_NAME}.js")).exists();
            }
        },
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
    let disk_tyrano = root.join("tyrano").is_dir();
    if !disk_tyrano {
        // 磁盘上没有 tyrano/ → 只可能是 Electron 把整个前端整包进了 asar
        // （检测侧靠**包内**的 tyrano/ 认出来，见 `TyranoPlugin::detect_scan`）。
        //
        // 这种情况**刻意**走与 HTML 系完全相同的管道**和同一份 hook**：
        //   · 管道本来共用（`install_html_like` 就支持 asar 目标）；
        //   · hook 也共用 —— 那份 HTML hook 已含 Tyrano v6 的逐字正文接缝，而且是**在本机
        //     真实 Electron + TyranoScript 游戏上端到端验证过**的那一份。
        //     换 hook 只会引入未验证的差异（两份 hook 有 118 行不同：jQuery 包装、trim 兜底、
        //     同步/异步 XHR），而症状会是「引擎标签修对了、游戏反而坏了」—— 正是最不该发生的事。
        // 两份 hook 的其余差异是**待收敛的债务**：真正该做的是合成一份（DOM/逐字接缝/jQuery
        // 三种管线合一），做完再统一；在那之前不动已验证的产物。
        let asar_tyrano = crate::engines::scan::probe_asar(root)
            .map(|a| a.has_prefix("tyrano/"))
            .unwrap_or(false);
        if asar_tyrano {
            return install_html(root, json_path);
        }
        return Err(format!(
            "未找到 tyrano/ 目录，也未在 Electron 封包（app.asar）里发现 TyranoScript 运行时（{}）—— \
             请把游戏目录选成游戏根目录：明文版应同时含 index.html 与 tyrano/，\
             Electron 版应在 resources/app.asar 里含 tyrano/",
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

/// 往入口 HTML 末尾挂脚本标签。返回 (新内容, 是否真的改了)；已挂过则原样返回。
fn with_hook_tag(html: &str) -> (String, bool) {
    if html.contains(&format!("{HOOK_NAME}.js")) {
        return (html.to_string(), false);
    }
    let tag = format!("<script src=\"{HOOK_NAME}.js\"></script>");
    let patched = match html.to_ascii_lowercase().rfind("</body>") {
        Some(pos) => format!("{}{}{}", &html[..pos], tag, &html[pos..]),
        None => format!("{html}{tag}"),
    };
    (patched, true)
}

/// 按目标类型取「hook 与 JSON 所在目录」——磁盘目标在游戏根，asar 目标在 asar 内。
fn read_asar_named(asar: &Path, name: &str) -> Result<Option<Vec<u8>>, String> {
    let mut src = crate::formats::source::Source::open(asar, crate::formats::source::MAX_ARCHIVE)?;
    let (files, data_start) = crate::formats::asar::parse_index(&mut src)?;
    match files.get(name) {
        Some(node) => Ok(Some(crate::formats::asar::read_entry(&mut src, data_start, node)?)),
        None => Ok(None),
    }
}

/// 把 hook + 翻译 JSON 写进 asar，并改写 asar 内的入口页挂上脚本标签。
///
/// 用**原位改写**而不是「解包 → 重打包」：asar 数据区是紧凑拼接的，某条目变大只等于
/// 它之后的条目整体平移，其余字节原样流式复制 —— 上百 MB 的封包既不落一整套临时文件，
/// 也不会重新编码游戏资源。写出的档案除补丁外与原档案逐字节相同（有单测钉死）。
fn install_into_asar(
    asar: &Path,
    entry: &str,
    json_text: String,
    hook_js: &str,
    total: usize,
    skipped: usize,
    tail_hint: &str,
) -> Result<String, String> {
    let html = read_asar_named(asar, entry)?
        .ok_or_else(|| format!("asar 内没有条目 {entry}"))?;
    let html = String::from_utf8_lossy(&html).into_owned();
    let (patched, changed) = with_hook_tag(&html);

    let mut patches: Vec<(String, Vec<u8>)> = if changed {
        vec![(entry.to_string(), patched.into_bytes())]
    } else {
        Vec::new()
    };
    // 两个汉化文件：**已存在就按「改写」处理，只有不存在才追加**。
    //
    // 为什么必须区分：asar 数据区是紧凑的，追加只能贴在末尾。若重复注入一律当"新增"，
    // 会把整份 payload 再贴一遍 —— 实测第二次注入后文件多涨 415 KB，且旧副本变成没人引用
    // 的孤儿字节（数据区出现空洞），**第三次注入就会因"数据区不连续"直接失败**。
    // 改写路径同样只动头部 offset/size，数据区照旧流式抄，不留空洞。
    let mut adds: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, bytes) in [
        (format!("{HOOK_NAME}.js"), hook_js.as_bytes().to_vec()),
        (JSON_IN_GAME.to_string(), json_text.into_bytes()),
    ] {
        if read_asar_named(asar, &name)?.is_some() {
            patches.push((name, bytes));
        } else {
            adds.push((name, bytes));
        }
    }

    let tmp = asar.with_extension("asar.stool_new");
    let _ = std::fs::remove_file(&tmp);
    crate::formats::asar::patch_archive(asar, &tmp, &patches, &adds).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("改写 asar 失败: {e}")
    })?;

    // 首次注入前整文件备份原 asar（已存在则保留，绝不覆盖），再把新档案顶上去。
    if let Err(e) = crate::settings::backup_once(asar) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("备份原 asar 失败: {e}"));
    }
    if let Err(e) = std::fs::rename(&tmp, asar) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("替换 asar 失败: {e}"));
    }

    let verb = if changed { "注入完成" } else { "注入已更新" };
    Ok(format!(
        "{verb}：{total} 条翻译（{skipped} 条空译文跳过）。{tail_hint}\
         \n已写入 Electron 封包：{} 内的 {entry} 挂上脚本，同封包内写入 {HOOK_NAME}.js 与 {JSON_IN_GAME}（已存在则原地改写）；\
         原封包已备份为 {}.stool.bak（用「还原」可一键退回）。",
        asar.display(),
        asar.display()
    ))
}

/// HTML 系（HTML/Electron 与 TyranoBuilder）的共用注入流程。
fn install_html_like(root: &Path, json_path: &Path, hook_js: &str, tail_hint: &str) -> Result<String, String> {
    let target = resolve_html_target(root)?;
    let (map, total, skipped) = load_map(json_path)?;
    let json_text =
        serde_json::to_string_pretty(&serde_json::Value::Object(map)).map_err(|e| e.to_string())?;

    match target {
        // 明文 HTML：脚本与 JSON 写在入口页同目录（即游戏根）。
        HtmlTarget::File(entry) => {
            std::fs::write(root.join(JSON_IN_GAME), json_text).map_err(|e| format!("写入翻译 JSON 失败: {e}"))?;
            std::fs::write(root.join(format!("{HOOK_NAME}.js")), hook_js)
                .map_err(|e| format!("写入 Hook 失败: {e}"))?;

            let html = std::fs::read_to_string(&entry).map_err(|e| format!("读取 {} 失败: {e}", entry.display()))?;
            let (patched, changed) = with_hook_tag(&html);
            if !changed {
                return Ok(format!("注入已更新：{total} 条翻译（{skipped} 条空译文跳过）。{tail_hint}"));
            }
            // 首次注入前备份入口 HTML（已存在则保留，绝不覆盖）
            crate::settings::backup_once(&entry).map_err(|e| format!("备份入口 HTML 失败: {e}"))?;
            std::fs::write(&entry, patched).map_err(|e| format!("写入入口 HTML 失败: {e}"))?;
            Ok(format!("注入完成：{total} 条翻译（{skipped} 条空译文跳过）。{tail_hint}"))
        }
        // Electron：入口页与脚本都必须落在 asar 内 —— 页面是从 asar 加载的，
        // 脚本和 JSON 放游戏根它一个也读不到。
        HtmlTarget::Asar { asar, entry } => {
            install_into_asar(&asar, &entry, json_text, hook_js, total, skipped, tail_hint)
        }
    }
}

/// 卸载：还原入口 HTML，删除 Hook 与翻译 JSON。
pub fn uninstall_html(root: &Path) -> Result<String, String> {
    let target = resolve_html_target(root).map_err(|e| format!("{e}（无法定位注入痕迹，可能本来就未注入）"))?;
    match target {
        HtmlTarget::Asar { asar, .. } => {
            // asar 只增不减，没法「删掉两个条目」还原；但首次注入前整文件备份过，
            // 直接拷回去是**字节级还原**，也符合「还原到注入前」的语义。
            let bak = crate::settings::backup_path_for(&asar);
            if !bak.exists() {
                return Err(format!(
                    "未找到 Electron 封包的备份（{}），无法字节级还原。\
                     若确实要丢弃注入，可手动处理 asar 内的 {HOOK_NAME}.js 与 {JSON_IN_GAME}。",
                    bak.display()
                ));
            }
            std::fs::copy(&bak, &asar).map_err(|e| format!("还原 asar 失败: {e}"))?;
            std::fs::remove_file(&bak).map_err(|e| format!("删除备份失败: {e}"))?;
            Ok("已从备份还原 Electron 封包（asar），注入痕迹全部消失。重启游戏生效。".into())
        }
        HtmlTarget::File(entry) => {
            let dir = entry.parent().unwrap_or(root).to_path_buf();
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
            for p in [dir.join(format!("{HOOK_NAME}.js")), dir.join(JSON_IN_GAME)] {
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
    }
}

/// 游戏端 Hook 插件（由 Rust 端原样写入 js/plugins/stool_translate.js）。
const HOOK_JS: &str = include_str!("../../assets/hooks/mv_mz.js");

/// HTML/Electron 游戏端 Hook：DOM 文本节点运行时替换（MutationObserver）。
const HTML_HOOK_JS: &str = include_str!("../../assets/hooks/html.js");

/// TyranoBuilder / TyranoScript 游戏端 Hook。
///
/// 与 HTML Hook 的差别（Tyrano 是 jQuery + KAG 体系）：
/// 1. 保留 MutationObserver 的 DOM 文本节点替换（KAG 消息最终落到 DOM，不是 Canvas）；
/// 2. **额外**包装 jQuery 的 `$.fn.html` / `$.fn.text`，在 Tyrano 用 jQuery 写消息时做整串替换
///    —— 这能覆盖"整段文本被一次性 html() 写入"导致文本节点被 `<br>` 拆开、单节点匹配不中的情况；
/// 3. 对带 `message`/`text` 类名的元素做元素级 textContent 兜底匹配；
/// 4. **额外**接管 `tyrano.plugin.kag.tag.text.buildMessageHTML`（TyranoScript v6 的正文入口）。
///    v6 会把整行明文逐字包成 `<span class="char">`，DOM 里每个文本节点只剩一个字，
///    前三招**全都失效**（实测：角色名翻得出、正文一片日文）。第 4 招把译文按整行替换后
///    仍交给 Tyrano 自己逐字渲染，打字动画/换行/描边/ruby 全部保留。
///    HTML Hook 里也带了同一份（asar 打包的 Tyrano 游戏按 `html_game` 处理，拿不到本 Hook）。
const TYRANO_HOOK_JS: &str = include_str!("../../assets/hooks/tyrano.js");

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

    /// hook 现在是**真 .js 文件**（`assets/hooks/`，由 `include_str!` 内嵌），所以可以做
    /// **真语法校验**——这在它还是 `const HOOK_JS: &str = r#"..."#` 时做不到（不能 lint、
    /// 不能单独跑，只能断言几个源码子串；少个括号 / 字符串没闭合，要等真机注入才暴露）。
    /// 单机快回路是 `node scripts/check_hooks.cjs`（~50ms）；这里给 `cargo test` 兜底，
    /// 免得有人绕过脚本直接跑测试。找不到 node 就**显式跳过**（不静默当通过）。
    #[test]
    fn hook_js_files_pass_node_syntax_check() {
        let node = std::env::var("STOOL_NODE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "node".to_string());
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("hooks");
        for name in ["mv_mz.js", "html.js", "tyrano.js"] {
            let path = dir.join(name);
            assert!(path.is_file(), "hook 源文件缺失: {}", path.display());
            let out = match std::process::Command::new(&node).arg("--check").arg(&path).output() {
                Ok(o) => o,
                Err(e) => {
                    eprintln!(
                        "[skip] hook 语法校验：执行 `{node}` 失败（{e}）。\
                         指定解释器：STOOL_NODE=<node 的绝对路径>"
                    );
                    return;
                }
            };
            assert!(
                out.status.success(),
                "`node --check {name}` 未通过：\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

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

    /// 回归锁：MV/MZ 运行时 Hook 必须跳过资源名字段、事件指令只走文字参数白名单。
    ///
    /// 背景（实测 bug）：`$dataSystem.title1Name = "タイトル画面"` 被整句替换成 `标题画面`，
    /// 游戏随即报 `Failed to load img/titles1/标题画面.png` 并卡在标题画面。
    /// 根因是旧实现只按 `looksLikePath` 判断，而 `タイトル画面` 既无扩展名也无斜杠。
    #[test]
    fn test_mv_hook_protects_asset_names() {
        for k in [
            "title1Name", "title2Name", "faceName", "characterName", "battlerName",
            "battleback1Name", "battleback2Name", "parallaxName", "tilesetName", "note",
        ] {
            assert!(HOOK_JS.contains(&format!("{k}: 1")), "SKIP_KEYS 缺少资源名字段 {k}");
        }
        // 文字指令进白名单
        for code in ["101", "102", "401", "402", "405", "320", "324", "325"] {
            assert!(HOOK_JS.contains(&format!("{code}: [")), "TEXT_PARAM 缺少文字指令 {code}");
        }
        // 带资源名/代码的指令绝不能进白名单
        for code in ["231", "232", "241", "245", "249", "250", "132", "236", "355", "655"] {
            assert!(!HOOK_JS.contains(&format!("{code}: [")), "TEXT_PARAM 不该包含 {code}");
        }
        // 音频描述符的 name = 文件名，要跳过
        assert!(HOOK_JS.contains("sound && k === \"name\""));
        assert!(HOOK_JS.contains("isSoundDescriptor"));
        // 旧的「只按像不像路径判断」实现已移除
        assert!(!HOOK_JS.contains("!SKIP_KEYS[k] && !looksLikePath(val)"));
    }

    /// 回归锁：MV/MZ Hook 的替换规则必须是「整句优先 → 最长片段贪心兜底」，并且显示层要挂到
    /// `Bitmap.prototype.drawText`（DTextPicture / Text2Frame 这类插件把文字直接画进位图，
    /// 只挂 `Window_Base` 会整片漏掉）。
    ///
    /// 纯 Rust 测不了 JS 行为，真正的行为验证在 `scripts/verify_inject_hook.cjs`（Node 里跑）。
    #[test]
    fn test_mv_hook_substring_fallback() {
        assert!(HOOK_JS.contains("function subst("), "缺少片段替换实现");
        assert!(HOOK_JS.contains("function buildTrie()"), "缺少前缀树构建");
        assert!(HOOK_JS.contains("TRIE_END"), "前缀树缺少存放译文的节点键");
        assert!(HOOK_JS.contains("MIN_FRAG = 2"), "1 字键必须排除在片段替换之外");
        assert!(
            HOOK_JS.contains("CACHE = Object.create(null)"),
            "缓存必须用无原型对象，否则 __proto__/constructor 会撞键"
        );
        // 索引必须在装钩子之前建好，否则首帧起就静默不替换
        let idx = HOOK_JS.find("buildTrie();").expect("未调用 buildTrie()");
        let hooks = HOOK_JS.find("installDataHooks();").expect("未调用 installDataHooks()");
        assert!(idx < hooks, "buildTrie() 必须在 installDataHooks() 之前调用");
        // 显示层：Window_Base 三个 + Bitmap 一层
        assert!(HOOK_JS.contains(r#"patchDrawText(Window_Base.prototype, "drawTextEx")"#));
        assert!(HOOK_JS.contains(r#"patchDrawText(Bitmap.prototype, "drawText")"#));
    }

    /// 回归锁：TyranoScript v6 的正文是「逐字 span」，节点级匹配永远命中不了，
    /// 必须靠接管 `buildMessageHTML`（明文入口）。**两份 hook 都要带**，别只补一份。
    ///
    /// 背景（实测 bug）：asar 打包的 Tyrano 游戏（NTRdemic）注入后角色名翻成中文、
    /// 正文全是日文 —— 因为 v6 把每个字包成 `<span class="char">`，每个文本节点只剩一个字。
    #[test]
    fn test_tyrano_v6_seam_in_both_hooks() {
        for (name, js) in [("HTML_HOOK_JS", HTML_HOOK_JS), ("TYRANO_HOOK_JS", TYRANO_HOOK_JS)] {
            assert!(js.contains("buildMessageHTML"), "{name} 缺少 v6 正文入口接管");
            assert!(js.contains("__stool"), "{name} 缺少防重复包装标记");
            assert!(
                js.contains("TY.plugin.kag.tag.text"),
                "{name} 缺少 tyrano.plugin.kag.tag.text 定位（否则非 Tyrano 页面会误判）"
            );
            // 定义了还要真的调用，否则等于没写
            assert!(js.contains("pollTyranoSeam()"), "{name} 定义了轮询却没调用");
        }
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

#[cfg(test)]
mod tests_html_target {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_inject_html_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Electron 包在根目录放的 `LICENSES.chromium.html` 必须被判为非入口。
    #[test]
    fn test_is_non_entry_html_name() {
        for bad in ["LICENSES.chromium.html", "license.html", "third_party.html", "README.html", "changelog.htm"] {
            assert!(is_non_entry_html_name(bad), "{bad} 应判为非入口");
        }
        for ok in ["index.html", "game.html", "main.htm"] {
            assert!(!is_non_entry_html_name(ok), "{ok} 不该判为非入口");
        }
    }

    /// 许可证页没有 `<script>`，据此不能当入口。
    #[test]
    fn test_looks_like_entry_html() {
        assert!(!looks_like_entry_html("<html><body><span>Credits</span></body></html>"));
        assert!(looks_like_entry_html("<html><body><script src=\"a.js\"></script></body></html>"));
    }

    #[test]
    fn test_with_hook_tag_idempotent() {
        let (p, changed) = with_hook_tag("<html><body>hi</body></html>");
        assert!(changed && p.contains("stool_translate.js"));
        assert!(p.find("stool_translate.js").unwrap() < p.find("</body>").unwrap(), "标签应在 </body> 之前");
        let (p2, changed2) = with_hook_tag(&p);
        assert!(!changed2, "已挂过就不该再改");
        assert_eq!(p, p2);
    }

    /// 有 asar 时不能被根目录的许可证 HTML 抢走 —— 这正是线上那个 bug。
    #[test]
    fn test_resolve_prefers_asar_over_license_html() {
        let g = tmpdir("prefer_asar");
        std::fs::write(g.join("LICENSES.chromium.html"), "<html><body>Credits</body></html>").unwrap();
        let src = g.join("app_src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("index.html"), "<html><body><script src=\"x.js\"></script></body></html>").unwrap();
        std::fs::create_dir_all(g.join("resources")).unwrap();
        crate::formats::asar::pack(&src, &g.join("resources/app.asar")).unwrap();

        match resolve_html_target(&g).unwrap() {
            HtmlTarget::Asar { entry, .. } => assert_eq!(entry, "index.html"),
            HtmlTarget::File(p) => panic!("应选 asar 内的入口，却选了 {}", p.display()),
        }
        let _ = std::fs::remove_dir_all(&g);
    }

    /// 只有许可证 HTML、没有 asar → 必须报错，不能随便挑个文件就注入。
    #[test]
    fn test_resolve_rejects_license_only() {
        let g = tmpdir("license_only");
        std::fs::write(g.join("LICENSES.chromium.html"), "<html><body>Credits</body></html>").unwrap();
        let e = resolve_html_target(&g).unwrap_err();
        assert!(e.contains("未找到入口 HTML"), "{e}");
        let _ = std::fs::remove_dir_all(&g);
    }

    /// 明文 HTML 游戏（根目录就有 index.html）走老路径，不受影响。
    #[test]
    fn test_resolve_plain_index() {
        let g = tmpdir("plain");
        std::fs::write(g.join("index.html"), "<html><body><script src=\"a.js\"></script></body></html>").unwrap();
        match resolve_html_target(&g).unwrap() {
            HtmlTarget::File(p) => assert_eq!(p, g.join("index.html")),
            HtmlTarget::Asar { .. } => panic!("无 asar 时不该走 asar 分支"),
        }
        let _ = std::fs::remove_dir_all(&g);
    }

    /// 端到端：注入 asar → 新档案可读、原条目字节不变、幂等、可一键还原。
    #[test]
    fn test_install_and_uninstall_into_asar() {
        let g = tmpdir("asar_e2e");
        let src = g.join("app_src");
        std::fs::create_dir_all(src.join("data")).unwrap();
        std::fs::write(src.join("index.html"), "<html><body><script src=\"boot.js\"></script></body></html>").unwrap();
        std::fs::write(src.join("data/blob.bin"), vec![9u8; 700]).unwrap();
        std::fs::create_dir_all(g.join("resources")).unwrap();
        let asar = g.join("resources/app.asar");
        crate::formats::asar::pack(&src, &asar).unwrap();
        let before = std::fs::read(&asar).unwrap();

        let map = g.join("tr.json");
        std::fs::write(&map, r#"{"こんにちは":"你好"}"#).unwrap();
        let msg = install_html(&g, &map).unwrap();
        assert!(msg.contains("注入完成"), "{msg}");
        assert!(std::fs::metadata(format!("{}.stool.bak", asar.display())).is_ok(), "首次注入应留下备份");

        // 新 asar：4 个条目；入口页挂上标签；未打补丁的条目逐字节不变
        let data = std::fs::read(&asar).unwrap();
        let (files, ds) = crate::formats::asar::parse_bytes(&data).unwrap();
        assert_eq!(files.len(), 4);
        let html = String::from_utf8(crate::formats::asar::read_file(&data, ds, &files["index.html"]).unwrap()).unwrap();
        assert!(html.contains("stool_translate.js"));
        assert_eq!(crate::formats::asar::read_file(&data, ds, &files["data/blob.bin"]).unwrap(), vec![9u8; 700]);
        assert!(files.contains_key("stool_translate.js") && files.contains_key("stool_translate.json"));

        let st = status(&g, "html_game");
        assert!(st.installed, "应报「已注入」");
        assert_eq!(st.entries, 1);

        // 幂等：再注入一次，标签不重复
        install_html(&g, &map).unwrap();
        let data2 = std::fs::read(&asar).unwrap();
        let (f2, ds2) = crate::formats::asar::parse_bytes(&data2).unwrap();
        let h2 = String::from_utf8(crate::formats::asar::read_file(&data2, ds2, &f2["index.html"]).unwrap()).unwrap();
        assert_eq!(h2.matches("stool_translate.js").count(), 1, "脚本标签不该重复：{h2}");
        // 幂等还要求**不重复追加载荷**：条目数不变、文件不涨。
        // 曾经的 bug：重复注入把整份 payload 再贴一遍（实测第二次多涨 415 KB），旧副本变成
        // 无人引用的孤儿字节 → 数据区出现空洞 → **第三次注入直接报「数据区不连续」**。
        assert_eq!(f2.len(), 4, "重复注入不该新增条目");
        assert_eq!(data2.len(), data.len(), "重复注入不该让 asar 变大");

        // 还原：asar 逐字节回到注入前
        uninstall_html(&g).unwrap();
        assert_eq!(std::fs::read(&asar).unwrap(), before, "还原后应与注入前逐字节一致");
        let _ = std::fs::remove_dir_all(&g);
    }

    /// TyranoScript 被 Electron 打进 asar 的注入/还原：根目录**没有** `tyrano/`，只能在包内看见。
    ///
    /// 这是「引擎身份」修复后的关键路径 —— 检测靠包内 `tyrano/` 认出它是 Tyrano（而不是
    /// `html_game`），注入也随之放行（旧实现硬要求磁盘上有 `tyrano/` 目录，会直接报错）。
    #[test]
    fn test_install_tyrano_into_asar() {
        let g = tmpdir("asar_tyrano");
        let src = g.join("app_src");
        std::fs::create_dir_all(src.join("tyrano").join("plugins").join("kag")).unwrap();
        std::fs::create_dir_all(src.join("data").join("scenario")).unwrap();
        std::fs::write(src.join("index.html"), "<html><body><script src=\"tyrano/tyrano.js\"></script></body></html>").unwrap();
        std::fs::write(src.join("tyrano").join("tyrano.js"), "/* tyrano runtime */").unwrap();
        std::fs::write(src.join("tyrano").join("plugins").join("kag").join("kag.tag.js"), "var x=1;").unwrap();
        std::fs::write(src.join("data").join("scenario").join("a.ks"), "[scene]\nこんにちは").unwrap();
        std::fs::create_dir_all(g.join("resources")).unwrap();
        let asar = g.join("resources/app.asar");
        crate::formats::asar::pack(&src, &asar).unwrap();
        let before = std::fs::read(&asar).unwrap();

        // 磁盘上没有 tyrano/，但包内有 → 注入应放行
        assert!(!g.join("tyrano").is_dir(), "本用例刻意不给磁盘上的 tyrano/");

        let map = g.join("tr.json");
        std::fs::write(&map, r#"{"こんにちは":"你好"}"#).unwrap();
        let msg = install_tyrano(&g, &map).unwrap();
        assert!(msg.contains("注入完成"), "{msg}");

        let data = std::fs::read(&asar).unwrap();
        let (files, ds) = crate::formats::asar::parse_bytes(&data).unwrap();
        let hook = crate::formats::asar::read_file(&data, ds, &files["stool_translate.js"]).unwrap();
        // 不变量：asar 打包下两条路线**产出完全一致**（同一管道 + 同一份 hook）。
        // 这条不变量就是「修正引擎标签不能改变已注入游戏的字节」的保险 ——
        // 已验证可用的那份 hook 没被换掉，换的只是检测结果里的引擎名。
        assert_eq!(hook, HTML_HOOK_JS.as_bytes(), "asar 路线应与 html 路线写入同一份 hook");
        assert!(
            String::from_utf8_lossy(&hook).contains("已接管 TyranoScript 逐字文本管线"),
            "该 hook 必须含 Tyrano v6 逐字正文接缝"
        );
        let html = String::from_utf8(
            crate::formats::asar::read_file(&data, ds, &files["index.html"]).unwrap(),
        )
        .unwrap();
        assert!(html.contains("stool_translate.js"), "入口页应挂上脚本标签：{html}");
        assert_eq!(status(&g, "tyrano").entries, 1);

        // 卸载：这条路线与 html_game 共用同一份 asar 还原（备份整包拷回）
        uninstall(&g, "tyrano").unwrap();
        assert_eq!(std::fs::read(&asar).unwrap(), before, "还原后应与注入前逐字节一致");
        let _ = std::fs::remove_dir_all(&g);
    }

    /// 反例：磁盘上既没有 tyrano/，包内也没有 → 必须报错并给出改法，不能静默乱注。
    #[test]
    fn test_install_tyrano_refuses_without_any_tyrano() {
        let g = tmpdir("asar_notyrano");
        std::fs::create_dir_all(g.join("resources")).unwrap();
        let src = g.join("app_src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("index.html"), "<html></html>").unwrap();
        crate::formats::asar::pack(&src, &g.join("resources/app.asar")).unwrap();

        let map = g.join("tr.json");
        std::fs::write(&map, r#"{"a":"b"}"#).unwrap();
        let e = install_tyrano(&g, &map).unwrap_err();
        assert!(e.contains("tyrano/"), "错误应说清缺什么：{e}");
        assert!(!g.join("resources/app.asar.stool.bak").exists(), "报错时不该留下备份/改动");
        let _ = std::fs::remove_dir_all(&g);
    }
}
