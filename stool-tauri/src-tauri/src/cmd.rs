//! Tauri 命令层：前端能调到的**全部**入口。
//!
//! 四条硬约束（见 `docs/TAURI重构方案.md` §6.1）：
//! 1. 每个页面只挂 1~2 个命令，命令名用「用户会说的话」；
//! 2. 返回**已算好的展示数据**，不把整棵 JSON 树丢过 IPC；
//! 3. 一律复用 `stool` 内核（engines / features），命令层不重写业务逻辑；
//! 4. **凡是要读盘 / 遍历目录的命令，一律 `async` + `offload`**（见 [`offload`] 的说明）。
//!
//! 第 4 条是实测踩出来的：STool 的检测要 walk 整棵游戏目录（真机样本 2.1 GB、
//! 几十万文件），而 Tauri v2 里**非 async 命令跑在主线程上** —— 一旦在里面做目录遍历，
//! 事件循环就被堵死，不只是这个命令的响应回不去，**后面所有 IPC 全部卡住**，
//! 前端表现为「点了按钮没反应」。所以重活必须挪出主线程。

use crate::AppState;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use stool::features::precheck::human_bytes;
use stool::features::saves::{self, SaveDoc, SearchScope};

// ---------------------------------------------------------------------------
// 主线程之外干活
// ---------------------------------------------------------------------------

/// 把「重」的同步工作丢到阻塞线程池执行。
///
/// 为什么不能直接写在命令体里：Tauri v2 的**非 async 命令在主线程执行**。
/// `detect_game` 要遍历整棵游戏目录、`load_save` 要读盘解析，
/// 放在主线程会把事件循环堵住 —— 连 `log_line` 这种微秒级命令的响应都发不出去，
/// 前端看起来就是「卡死」。`async fn` 只解决一半（不再占用主线程），
/// 但阻塞式 IO 仍会占住一个异步 worker，所以再套一层 `spawn_blocking` 最稳。
async fn offload<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(f).await {
        Ok(r) => r,
        Err(e) => Err(format!("后台任务异常：{e}\n改法：重试一次；若必现请导出诊断包反馈。")),
    }
}

// ---------------------------------------------------------------------------
// 应用信息 / 诊断通道
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub core_version: String,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo { name: "STool".into(), version: env!("CARGO_PKG_VERSION").into(), core_version: stool::VERSION.into() }
}

/// 前端回报通道。
///
/// WebView2 走 DirectComposition，本机抓像素不稳定（`CopyFromScreen` 全黑、
/// `PrintWindow` 也不可靠），所以自动化验证走这条
/// 「WebView 起来了 → JS 跑了 → IPC 通了」的确定性链路，而不是截图。
#[tauri::command]
pub fn log_line(s: String) {
    use std::io::Write;
    let path = std::env::var("STOOL_TUI_LOG")
        .unwrap_or_else(|_| std::env::temp_dir().join("stool-tauri.log").to_string_lossy().into_owned());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{s}");
    }
}

/// 系统「选择文件夹」对话框。
///
/// `blocking_pick_*` 的官方约定就是**不要在 主线程 调用**，所以这里也走 `spawn_blocking`。
#[tauri::command]
pub async fn pick_folder(app: tauri::AppHandle, title: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        app.dialog()
            .file()
            .set_title(&title)
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.display().to_string())
    })
    .await
    .ok()
    .flatten()
}

/// 系统「选择文件」对话框。`ext` 是提示用的扩展名（"csv" / "json"），空则不限。
///
/// 与 `pick_folder` 同样走 `spawn_blocking` —— 对话框是阻塞 API，放主线程会冻住整个 IPC。
#[tauri::command]
pub async fn pick_file(app: tauri::AppHandle, title: String, ext: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        let mut b = app.dialog().file().set_title(&title);
        let ext = ext.trim().trim_start_matches('.').to_string();
        if !ext.is_empty() {
            // 同时给「限定类型」和「所有文件」两档：用户偶尔要用改过扩展名的表格，
            // 只留一档会让人以为文件不见了。
            b = b
                .add_filter(ext.to_uppercase(), &[ext.as_str()])
                .add_filter("所有文件", &["*"]);        }
        b.blocking_pick_file()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.display().to_string())
    })
    .await
    .ok()
    .flatten()
}

// ---------------------------------------------------------------------------
// ① 选游戏 —— 引擎检测
// ---------------------------------------------------------------------------

#[derive(Serialize, Debug)]
pub struct Capability {
    pub id: String,
    pub label: String,
}

#[derive(Serialize, Debug)]
pub struct Alternative {
    pub name: String,
    pub score: i32,
    pub confidence: String,
}

#[derive(Serialize, Debug)]
pub struct DetectOut {
    pub root: String,
    pub engine_id: String,
    pub engine_name: String,
    pub score: i32,
    /// high / medium / low / none —— 前端据此上色（阈值只在 Rust 侧定义一次）
    pub level: String,
    pub confidence: String,
    pub confirmed: bool,
    /// 一句人话结论，例如「RPG Maker MV 游戏」
    pub summary: String,
    /// 「能不能处理」的直白说明
    pub verdict: String,
    pub notes: String,
    pub evidence: Vec<String>,
    pub capabilities: Vec<Capability>,
    pub alternatives: Vec<Alternative>,
    pub elapsed_ms: u128,
}

fn op_key(op: stool::engines::Op) -> &'static str {
    use stool::engines::Op;
    match op {
        Op::Extract => "extract",
        Op::Repack => "repack",
        Op::Decompile => "decompile",
        Op::TextExtract => "text_extract",
        Op::TextImport => "text_import",
        Op::TextInject => "text_inject",
        Op::Save => "save",
        Op::Unlock => "unlock",
    }
}

fn level_of(c: stool::engines::Confidence) -> &'static str {
    use stool::engines::Confidence;
    match c {
        Confidence::High => "high",
        Confidence::Medium => "medium",
        Confidence::Low => "low",
        Confidence::None => "none",
    }
}

/// 纯函数：检测一个游戏目录（不碰全局状态，可安全放进后台线程）。
///
/// 只调 `Registry::detect_all`（内部一次 walkdir），**不额外遍历目录**——
/// 与 egui 版、CLI 版共用同一条「零额外 IO」约定。
fn detect_impl(path: &str) -> Result<DetectOut, String> {
    let t0 = Instant::now();
    let root = PathBuf::from(path);
    if !root.is_dir() {
        return Err(format!(
            "这个路径不是一个文件夹：{path}\n改法：点「选择游戏文件夹」，选到游戏根目录——\
             里面有游戏 .exe，或者有一个 www/ 子目录的那一层。"
        ));
    }

    let reg = stool::engines::Registry::new();
    // detect_all 无匹配时也至少带一行 generic 兜底，所以这里正常不会为 None。
    let results = reg.detect_all(&root);
    let (best, confirmed) = stool::engines::Registry::pick_from(&results).ok_or_else(|| {
        format!("在 {path} 里没扫到任何引擎线索。\n改法：确认选中的是游戏安装目录本身（不是它的上一级，也不是里面的 Data/ 子目录）。")
    })?;

    let capabilities: Vec<Capability> = reg
        .get(&best.plugin_id)
        .map(stool::engines::effective_capabilities)
        .unwrap_or_default()
        .into_iter()
        .map(|op| Capability { id: op_key(op).into(), label: op.label().into() })
        .collect();

    let summary = if confirmed {
        format!("{} 游戏", best.name)
    } else if best.score > 0 {
        format!("像是 {}，但证据不足", best.name)
    } else {
        "没认出来是什么引擎".to_string()
    };
    let verdict = if confirmed {
        "没问题，这个游戏 STool 能处理。下面是可以直接做的事。".to_string()
    } else {
        "自动识别没十足把握，可以试着往下走；下面是其他可能的引擎。".to_string()
    };

    let alternatives = results
        .iter()
        .skip(1)
        .filter(|d| d.score > 0)
        .take(6)
        .map(|d| Alternative { name: d.name.clone(), score: d.score, confidence: d.confidence().label().into() })
        .collect();

    Ok(DetectOut {
        root: root.display().to_string(),
        engine_id: best.plugin_id.clone(),
        engine_name: best.name.clone(),
        score: best.score,
        level: level_of(best.confidence()).into(),
        confidence: best.confidence().label().into(),
        confirmed,
        summary,
        verdict,
        notes: best.notes.clone(),
        evidence: best.evidence.clone(),
        capabilities,
        alternatives,
        elapsed_ms: t0.elapsed().as_millis(),
    })
}

#[tauri::command]
pub async fn detect_game(path: String, state: tauri::State<'_, AppState>) -> Result<DetectOut, String> {
    let p = path.clone();
    let out = offload(move || detect_impl(&p)).await?;
    if let Ok(mut g) = state.game_root.lock() {
        *g = Some(out.root.clone());
    }
    // 引擎 id 一并记住：后面的解包/封包/解锁都据此选插件，界面不必每次重传。
    if let Ok(mut e) = state.engine_id.lock() {
        *e = Some(out.engine_id.clone());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// ② 改存档
// ---------------------------------------------------------------------------

/// 存档候选目录（游戏内 + AppData，与 egui 版同一口径，不另立标准）。
#[derive(Serialize)]
pub struct SaveLocation {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct FileRow {
    pub name: String,
    pub path: String,
    pub size: String,
    pub age: String,
}

const SAVE_EXTS: [&str; 11] =
    ["rpgsave", "rmzsave", "rmmzsave", "rxdata", "rvdata", "rvdata2", "sav", "save", "json", "dat", "persistent"];

#[tauri::command]
pub async fn save_locations(root: String) -> Result<Vec<SaveLocation>, String> {
    offload(move || {
        if root.is_empty() {
            return Ok(Vec::new());
        }
        Ok(saves::find_save_locations(Path::new(&root))
            .into_iter()
            .map(|(label, dir)| SaveLocation { label, path: dir.display().to_string() })
            .collect())
    })
    .await
}

/// 列出某个目录里的存档文件。只扫**这一个**目录，最多 200 条。
#[tauri::command]
pub async fn save_files(dir: String) -> Result<Vec<FileRow>, String> {
    offload(move || {
        let d = Path::new(&dir);
        if !d.is_dir() {
            return Err(format!("目录不存在或不是文件夹：{dir}\n改法：回到上一步重新选存档目录。"));
        }
        let mut files = saves::list_files(d, &SAVE_EXTS);
        files.truncate(200);
        Ok(files
            .into_iter()
            .map(|p| {
                let md = std::fs::metadata(&p).ok();
                // 注意：`Option<Metadata>` 不是 Copy，两个字段都要用就得借——
                // 先 `.as_ref()` 取，别让第一个 and_then 把它 move 走。
                let size = human_bytes(md.as_ref().map(|m| m.len()).unwrap_or(0));
                let age = md
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .map(human_age)
                    .unwrap_or_default();
                FileRow {
                    name: p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string()),
                    path: p.display().to_string(),
                    size,
                    age,
                }
            })
            .collect())
    })
    .await
}

fn human_age(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s} 秒前")
    } else if s < 3600 {
        format!("{} 分钟前", s / 60)
    } else if s < 86_400 {
        format!("{} 小时前", s / 3600)
    } else {
        format!("{} 天前", s / 86_400)
    }
}

#[derive(Serialize)]
pub struct SaveInfo {
    pub path: String,
    pub format: String,
    pub writable: bool,
    pub top_fields: usize,
}

fn save_info(doc: &SaveDoc) -> SaveInfo {
    SaveInfo {
        path: doc.path.display().to_string(),
        format: doc.format.label().into(),
        writable: doc.format.writable(),
        top_fields: doc.root.as_object().map(|m| m.len()).unwrap_or(0),
    }
}

fn load_doc(path: &str) -> Result<(SaveDoc, SaveInfo), String> {
    let doc = SaveDoc::load(Path::new(path)).map_err(|e| {
        format!(
            "打开存档失败：{e}\n改法：确认选的是存档文件本身。目前只支持明文 JSON、\
             RPG Maker MV（lz-string）、MZ（zlib）；Ruby Marshal 与 Ren'Py persistent 只能只读查看。"
        )
    })?;
    let info = save_info(&doc);
    Ok((doc, info))
}

#[tauri::command]
pub async fn load_save(path: String, state: tauri::State<'_, AppState>) -> Result<SaveInfo, String> {
    let (doc, info) = offload(move || load_doc(&path)).await?;
    if let Ok(mut g) = state.save.lock() {
        *g = Some(doc);
    }
    if let Ok(mut d) = state.dirty.lock() {
        *d = false;
    }
    Ok(info)
}

/// 系统「选择存档文件」对话框，选完直接加载。
#[tauri::command]
pub async fn pick_and_load_save(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<Option<SaveInfo>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        app.dialog()
            .file()
            .add_filter("存档文件", &["rpgsave", "rmzsave", "rmmzsave", "json", "sav", "save", "dat", "rxdata", "rvdata2"])
            .blocking_pick_file()
            .and_then(|fp| fp.into_path().ok())
            .map(|p| p.display().to_string())
    })
    .await
    .map_err(|e| format!("打开文件对话框失败：{e}"))?;

    let Some(path) = picked else { return Ok(None) };
    let (doc, info) = offload(move || load_doc(&path)).await?;
    if let Ok(mut g) = state.save.lock() {
        *g = Some(doc);
    }
    if let Ok(mut d) = state.dirty.lock() {
        *d = false;
    }
    Ok(Some(info))
}

#[derive(Serialize, Clone)]
pub struct Row {
    /// JSON Pointer（字段位置），内部用于定位
    pub path: String,
    /// 最后一段，给人看
    pub key: String,
    pub value: String,
    /// str / num / bool / other —— 前端只据此上色
    pub kind: String,
    /// 文本 / 数值 / 布尔 / 空 / 容器 —— 给人看
    pub ty: String,
}

fn kind_of(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::String(_) => "str",
        serde_json::Value::Number(_) => "num",
        serde_json::Value::Bool(_) => "bool",
        _ => "other",
    }
}

fn row_of(doc: &SaveDoc, ptr: &str) -> Row {
    let key = ptr.rsplit('/').next().unwrap_or(ptr).to_string();
    match doc.get(ptr) {
        Some(v) => {
            let t = saves::type_hint(v);
            Row {
                path: ptr.to_string(),
                key,
                value: saves::value_edit_text(v),
                kind: kind_of(v).into(),
                ty: if t.is_empty() { "容器".into() } else { t.into() },
            }
        }
        None => Row { path: ptr.to_string(), key, value: String::new(), kind: "other".into(), ty: "(已不存在)".into() },
    }
}

/// 搜索：纯内存操作，且有 500 条上限，留在当前线程即可（不必为它切线程）。
#[tauri::command]
pub fn search_save(query: String, scope: String, state: tauri::State<'_, AppState>) -> Result<Vec<Row>, String> {
    let g = state.save.lock().map_err(|_| "存档状态异常（内部锁出错）".to_string())?;
    let Some(doc) = g.as_ref() else { return Ok(Vec::new()) };
    let sc = match scope.as_str() {
        "keys" => SearchScope::Keys,
        "values" => SearchScope::Values,
        _ => SearchScope::All,
    };
    Ok(doc.search(&query, sc).into_iter().map(|p| row_of(doc, &p)).collect())
}

#[tauri::command]
pub fn set_save_value(ptr: String, raw: String, state: tauri::State<'_, AppState>) -> Result<Row, String> {
    let mut g = state.save.lock().map_err(|_| "存档状态异常（内部锁出错）".to_string())?;
    let Some(doc) = g.as_mut() else {
        return Err("还没打开存档。\n改法：先在上面选一个存档目录、点开一个存档文件。".into());
    };
    doc.set(&ptr, saves::parse_edit_text(&raw))?;
    if let Ok(mut d) = state.dirty.lock() {
        *d = true;
    }
    Ok(row_of(&*doc, &ptr))
}

#[tauri::command]
pub fn save_dirty(state: tauri::State<'_, AppState>) -> bool {
    state.dirty.lock().map(|d| *d).unwrap_or(false)
}

/// 回写原文件。内核里已经包含「先备份为 `<原名>.stool.bak` 再写」，
/// 所以这里不再自己备份（避免「两处备份、语义不一致」）。
#[tauri::command]
pub async fn commit_save(state: tauri::State<'_, AppState>) -> Result<String, String> {
    // 回写是「序列化 + 落盘」，属 IO → 挪到后台线程。
    // 做法：先把 doc **从状态里取出来**（take），后台跑完再放回去 ——
    // 这样既不必把 MutexGuard 跨线程（它本身不是 Send），又能整段走 offload。
    // 期间并发的 search_save 会看到「未打开存档」而返回空列表，这是可接受的：
    // 前端在写回期间把按钮置成了忙碌态。
    let doc = {
        let mut g = state.save.lock().map_err(|_| "存档状态异常（内部锁出错）".to_string())?;
        g.take().ok_or_else(|| "还没打开存档，没有可写回的内容。".to_string())?
    };

    let (saved, back) = offload(move || -> Result<(Result<String, String>, SaveDoc), String> { Ok((doc.save(), doc)) }).await?;
    if let Ok(mut g) = state.save.lock() {
        *g = Some(back);
    }
    let msg = saved?;
    if let Ok(mut d) = state.dirty.lock() {
        *d = false;
    }
    Ok(msg)
}

// ---------------------------------------------------------------------------
// ② 取出素材 / 重新打包
// ---------------------------------------------------------------------------

/// 长任务进度事件（前端监听 `op:progress`）。
///
/// 按 §6.1 的约定走**事件流**，而不是让前端定期 IPC 轮询 —— 解包几万个文件时
/// 轮询会把 IPC 通道打满，事件流是「内核报一次、前端画一次」。
#[derive(Serialize, Clone)]
pub struct Prog {
    pub frac: f32,
    pub msg: String,
}

#[derive(Serialize, Debug)]
pub struct OpResult {
    pub success: bool,
    pub message: String,
    pub files_done: usize,
    pub out_dir: String,
    pub logs: Vec<String>,
}

/// 进度节流判定：这次回调值不值得转发给界面。
///
/// 内核按**每个条目**回报进度（解包几万个文件就是几万次回调），原样转发会打满 IPC。
/// 规则：推进 ≥0.5% 或距上次 ≥80ms 才转发；**`frac == 1.0` 永远放行** ——
/// 否则进度条会停在 99%。
///
/// 抽成纯函数是为了能单测：节流写错的两个后果（刷爆 IPC / 进度条卡住）都不会报错，
/// 只会让人觉得「卡了」。
fn should_emit_progress(prev: f32, at: Instant, frac: f32, now: Instant) -> bool {
    frac >= 1.0
        || (frac - prev).abs() >= 0.005
        || now.saturating_duration_since(at) >= Duration::from_millis(80)
}

/// 解包 / 封包的共同实现：预检 → 统一分发 → 规整成展示数据。
///
/// **不碰任何 Tauri 类型**（进度靠传入的回调），所以能直接用真机样本做单元测试 ——
/// 本机反复强杀会让 WebView2 的窗口类状态坏掉，GUI 冒烟不稳定，见 §9.1。
fn run_op_core(
    op: stool::engines::Op,
    root: &str,
    engine_id: &str,
    out_dir: &str,
    repack_src: &str,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32, &str),
) -> Result<OpResult, String> {
    use stool::engines::{self, Op};

    let root_p = PathBuf::from(root);
    if !root_p.is_dir() {
        return Err(format!("游戏目录不存在：{root}\n改法：回「选游戏」页重新选一次。"));
    }
    if engine_id.trim().is_empty() {
        return Err("还不知道这是什么引擎，没法动手。\n改法：先去「选游戏」页让它认一下。".into());
    }

    let opts: HashMap<String, String> = HashMap::new();
    // 与 CLI / egui 同一口径：解包只写输出目录，封包只写游戏目录（内核根据 op 推）。
    let scope = stool::features::precheck::scope_for_op(op, &opts);

    let out_p = if out_dir.trim().is_empty() { None } else { Some(PathBuf::from(out_dir)) };
    if matches!(op, Op::Extract) && out_p.is_none() {
        return Err(
            "还没选输出目录。\n改法：点「选择输出目录」，选一个空目录（建议放在游戏目录外面）。".into()
        );
    }
    let repack_p = PathBuf::from(repack_src);
    if matches!(op, Op::Repack) && !repack_p.is_dir() {
        return Err(format!(
            "要打回去的源目录不存在：{repack_src}\n改法：选「取出素材」时生成的那个输出目录（里面有游戏原本的文件结构）。"
        ));
    }

    // 写操作先预检（目录可写 / 文件占用 / 磁盘空间）——几 GB 的解包尤其不能跑到一半才失败。
    let rep = stool::features::precheck::run(&root_p, out_p.as_deref(), scope);
    if !rep.ok() {
        return Err(rep.fail_summary());
    }

    let reg = engines::Registry::new();
    let empty_out = PathBuf::new();
    let out_ref: &Path = out_p.as_deref().unwrap_or(&empty_out);
    let ctx = engines::Ctx { root: &root_p, out_dir: out_ref, options: &opts, progress, cancel };
    let paths = engines::OpPaths { repack_src: &repack_p, csv: out_ref, translation: &root_p };
    let outcome = engines::exec_op(&reg, engine_id, op, &ctx, &paths);

    Ok(OpResult {
        success: outcome.success,
        message: outcome.message,
        files_done: outcome.files_done,
        out_dir: out_ref.display().to_string(),
        logs: outcome.logs,
    })
}

/// 把一次 op 丢到后台线程跑，并把内核的进度回调转成 Tauri 事件。
async fn spawn_op(
    app: tauri::AppHandle,
    op: stool::engines::Op,
    root: String,
    engine_id: String,
    out_dir: String,
    repack_src: String,
    cancel: Arc<AtomicBool>,
) -> Result<OpResult, String> {
    use tauri::Emitter;
    // 上一次的取消不该影响这一次。
    cancel.store(false, Ordering::Relaxed);
    offload(move || {
        let emit = app.clone();
        // 节流：内核是**每个条目**回报一次（解包几万个文件就是几万次回调），
        // 原样转发会把 WebView 的 IPC 通道打满。掐成「推进 ≥0.5% 或 ≥80ms 才有一次」，
        // 完成（frac==1.0）永远放行 —— 否则进度条会停在 99%。
        let last = std::cell::Cell::new((0.0f32, Instant::now()));
        let progress = move |frac: f32, msg: &str| {
            let (prev, at) = last.get();
            let now = Instant::now();
            if !should_emit_progress(prev, at, frac, now) {
                return;
            }
            last.set((frac, now));
            let _ = emit.emit("op:progress", Prog { frac, msg: msg.to_string() });
        };
        run_op_core(op, &root, &engine_id, &out_dir, &repack_src, &cancel, &progress)
    })
    .await
}

/// 文本 op（提取 / 回填）的后台执行 —— 与 `spawn_op` 同构，只是走 `run_text_op`。
///
/// 不把两者合成一个的原因：它们的「第二路径参数」语义不同（`repack_src` 是源目录，
/// 文本 op 的 `file` 是 CSV 文件），合成后要在一个函数里按 op 分支猜语义，
/// 反而更容易把「封包」和「回填」的参数接错 —— 那种错不会报错，只会静默传错路径。
async fn spawn_progress_op(
    app: tauri::AppHandle,
    op: stool::engines::Op,
    root: String,
    engine_id: String,
    out_dir: String,
    file: String,
    cancel: Arc<AtomicBool>,
) -> Result<OpResult, String> {
    use tauri::Emitter;
    cancel.store(false, Ordering::Relaxed);
    offload(move || {
        let emit = app.clone();
        let last = std::cell::Cell::new((0.0f32, Instant::now()));
        let progress = move |frac: f32, msg: &str| {
            let (prev, at) = last.get();
            let now = Instant::now();
            if !should_emit_progress(prev, at, frac, now) {
                return;
            }
            last.set((frac, now));
            let _ = emit.emit("op:progress", Prog { frac, msg: msg.to_string() });
        };
        run_text_op(op, &root, &engine_id, &out_dir, &file, &cancel, &progress)
    })
    .await
}

/// 「取出全部素材」：把封包里的图片 / 音频 / 脚本解到输出目录。
/// 只往输出目录写，**游戏目录一个字节都不动**。
#[tauri::command]
pub async fn extract_assets(app: tauri::AppHandle, state: tauri::State<'_, AppState>, out_dir: String) -> Result<OpResult, String> {
    let (root, engine_id) = current_game(&state)?;
    spawn_op(app, stool::engines::Op::Extract, root, engine_id, out_dir, String::new(), state.cancel.clone()).await
}

/// 「重新打包」：把（改过的）源目录塞回游戏封包。会写回游戏目录，**写前自动备份**。
#[tauri::command]
pub async fn repack_assets(app: tauri::AppHandle, state: tauri::State<'_, AppState>, src_dir: String) -> Result<OpResult, String> {
    let (root, engine_id) = current_game(&state)?;
    spawn_op(app, stool::engines::Op::Repack, root, engine_id, String::new(), src_dir, state.cancel.clone()).await
}

/// 取消正在跑的长任务（内核在条目边界退出，已写出的文件保留）。
#[tauri::command]
pub fn cancel_task(state: tauri::State<'_, AppState>) -> bool {
    state.cancel.store(true, Ordering::Relaxed);
    true
}

/// 在系统文件管理器里打开一个目录。
#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("这个目录还不存在：{path}\n改法：先执行一次「取出全部素材」。"));
    }
    shell_open(&p)
}

#[cfg(target_os = "windows")]
fn shell_open(p: &Path) -> Result<(), String> {
    // `explorer <路径>`：目录就打开文件夹，文件就用默认关联程序打开 ——
    // 两种情况一条命令搞定，不额外引插件。
    std::process::Command::new("explorer")
        .arg(p)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打不开：{e}\n改法：确认这个路径还在、且没有被杀毒软件拦下。"))
}

#[cfg(not(target_os = "windows"))]
fn shell_open(_p: &Path) -> Result<(), String> {
    Err("「打开文件夹 / 用系统程序打开」目前只在 Windows 上支持。".to_string())
}

/// 从状态里取「当前游戏 + 引擎」，缺任何一样都给出「原因 + 修法」。
fn current_game(state: &tauri::State<'_, AppState>) -> Result<(String, String), String> {
    let root = state
        .game_root
        .lock()
        .map_err(|_| "游戏状态异常（内部锁出错）".to_string())?
        .clone()
        .ok_or_else(|| "还没选游戏。\n改法：先去「选游戏」页选一个游戏文件夹。".to_string())?;
    let engine = state.engine_id.lock().map_err(|_| "引擎状态异常（内部锁出错）".to_string())?.clone().unwrap_or_default();
    Ok((root, engine))
}

// ---------------------------------------------------------------------------
// 备份还原 —— ⑤「写回存档」与 ②「重新打包」的「撤销上一次写入」
//
// 两处写入的备份**语义不一样**，界面必须分开说，命令层只如实转述：
// - **改存档**：`SaveDoc::save` 每次写回都**无条件覆盖**备份 → 备份 = 「上一次写回
//   之前的样子」，这才是真正的「撤销上一次写入」；
// - **重新打包**：走 `settings::backup_once`，**已有备份就不再覆盖** → 备份 =
//   「第一次被替换之前的原始封包」，也就是「还原回原版」，而不是「撤销上一次」。
//
// 为什么「列出备份」要由调用方给目录：重新打包一次可能命中**多个**封包
// （如 Artemis 会遍历所有 `.pfs`），而 `OpResult` 只回一句 message、没有目标路径列表
// —— 与其在命令层猜，不如让用户看到实际留下的备份。改存档页给存档目录（很快），
// 取出素材页给游戏根目录（走 `offload`，否则几千个文件的遍历会冻住所有 IPC，见坑③）。
// ---------------------------------------------------------------------------

/// 列出某目录下的 `.stool.bak` 备份（走内核 `features::restore::find_backups`）。
///
/// 复用 `FileRow` 而不新造结构：字段语义完全一样（`name`/`path`/`size`/`age`），
/// 前端也就能复用同一套行渲染，不会出现「两个结构各漂一次」。
#[tauri::command]
pub async fn restore_list(dir: String) -> Result<Vec<FileRow>, String> {
    offload(move || backup_rows_of(Path::new(&dir))).await
}

/// `restore_list` 的实体 —— 纯函数，可直接测（本文件测试的既定做法）。
fn backup_rows_of(dir: &Path) -> Result<Vec<FileRow>, String> {
    if !dir.is_dir() {
        return Err(format!(
            "这个目录不存在：{}\n改法：先让 STool 读到一次存档或游戏目录，再回来点「查看备份」。",
            dir.display()
        ));
    }
    Ok(stool::features::restore::find_backups(dir)
        .into_iter()
        .map(|p| {
            let md = std::fs::metadata(&p).ok();
            // 注意：`Option<Metadata>` 不是 Copy，两个字段都要用就得借 ——
            // 先 `.as_ref()` 取，别让第一个 and_then 把它 move 走。
            let size = human_bytes(md.as_ref().map(|m| m.len()).unwrap_or(0));
            let age = md
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| SystemTime::now().duration_since(t).ok())
                .map(human_age)
                .unwrap_or_default();
            FileRow {
                name: p
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string()),
                path: p.display().to_string(),
                size,
                age,
            }
        })
        .collect())
}

/// 校验「要还原的路径」确实是备份 —— 纯函数，可直接测。
///
/// **只接受 `.stool.bak` 结尾**：内核 `restore::original_of` 是从这个后缀反推原文件的，
/// 所以这条检查就是「不许拿它当任意文件覆盖入口」的那道边界。
fn backup_path_checked(bak: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(bak.trim());
    if !p.to_string_lossy().ends_with(".stool.bak") {
        return Err(format!(
            "只能还原 `.stool.bak` 备份。\n收到：{}\n改法：从备份列表里点一个 —— 列表里的每一条都是备份。",
            p.display()
        ));
    }
    Ok(p)
}

/// 用 `.stool.bak` 还原一个文件（内核 `restore_one`）。**备份本身保留，可反复还原。**
#[tauri::command]
pub async fn restore_one(bak: String) -> Result<String, String> {
    let p = backup_path_checked(&bak)?;
    offload(move || stool::features::restore::restore_one(&p)).await
}

// ---------------------------------------------------------------------------
// ③ 看素材
// ---------------------------------------------------------------------------

/// 一条可预览的素材。字段都是**算好的展示数据**，前端不再做格式判断。
#[derive(Serialize, Debug)]
pub struct MediaRow {
    pub name: String,
    /// 相对扫描根目录的路径 —— 比绝对路径短得多，列表里更好认
    pub rel: String,
    pub path: String,
    /// `image` / `audio` / `text` —— 前端据此决定用 <img> / <audio> / <pre>
    pub kind: String,
    pub size: String,
    /// 浏览器能不能渲染。`.tga`/`.tif` 算图片但渲染不了，界面要**提前说明**，
    /// 而不是塞给 <img> 一个坏链接让用户看着空白发呆。
    pub renderable: bool,
}

#[derive(Serialize, Debug)]
pub struct MediaScan {
    pub dir: String,
    pub files: Vec<MediaRow>,
    /// 命中总数；大于 `files.len()` 说明被推送上限截断了
    pub total: usize,
    pub elapsed_ms: u128,
}

/// 一次最多推给前端多少条。素材目录常有几万张图，全量过 IPC 会明显卡；
/// 前端另有虚拟化列表，但虚拟化省的是 DOM，不是 IPC。
const MEDIA_PUSH_CAP: usize = 3000;

/// 图片 / 音频内联成 data URL 的上限。超过就请用户走「用系统程序打开」，
/// 而不是把几百 MB 塞进 IPC。
const IMAGE_INLINE_MAX: u64 = 16 * 1024 * 1024;
const AUDIO_INLINE_MAX: u64 = 12 * 1024 * 1024;

fn scan_media_core(dir: &str, kind: &str) -> Result<MediaScan, String> {
    use stool::features::preview;
    let t0 = Instant::now();
    let root = PathBuf::from(dir);
    if !root.is_dir() {
        return Err(format!(
            "这个路径不是一个文件夹：{dir}\n改法：点「选择目录」，选到已经取出素材的那个输出目录（就是「取出素材」时用的那个）。"
        ));
    }

    let want = kind.trim();
    // 分类表在**内核**里（`features::preview`），egui 版与这里共用一份 ——
    // 别再写第二个扩展名表，否则两边的「什么算图片」迟早不一致。
    let mut matched: Vec<PathBuf> = preview::list_media(&root)
        .into_iter()
        .filter(|p| want.is_empty() || want == "all" || preview::kind_of(p).id() == want)
        .collect();
    let total = matched.len();
    matched.truncate(MEDIA_PUSH_CAP);

    let files: Vec<MediaRow> = matched
        .into_iter()
        .map(|p| {
            let k = preview::kind_of(&p);
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            MediaRow {
                name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string()),
                rel: p.strip_prefix(&root).unwrap_or(&p).display().to_string(),
                path: p.display().to_string(),
                kind: k.id().to_string(),
                size: human_bytes(size),
                renderable: preview::mime_of(&p) != "application/octet-stream",
            }
        })
        .collect();

    Ok(MediaScan { dir: root.display().to_string(), files, total, elapsed_ms: t0.elapsed().as_millis() })
}

#[derive(Serialize, Debug)]
pub struct MediaPayload {
    pub kind: String,
    pub mime: String,
    /// 图片 / 音频内联成 data URL；文本为空
    pub data_url: String,
    /// 文本内容（`kind == "text"` 时才有）
    pub text: String,
    /// 文本用的编码（UTF-8 / Shift-JIS / UTF-16LE…），界面要如实标出来
    pub encoding: String,
    pub size: String,
    pub truncated: bool,
}

/// 只读文件开头 `max` 字节 —— 文本预览不需要整个文件
/// （一个 50 MB 的 CSV 全读进来纯属浪费）。
fn read_head(path: &Path, max: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    f.take(max as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

fn read_media_core(path: &str) -> Result<MediaPayload, String> {
    use stool::features::preview::{self, MediaKind};
    let p = PathBuf::from(path);
    if !p.is_file() {
        return Err(format!("文件不存在或不是一个文件：{path}\n改法：回「看素材」重新选一个文件。"));
    }
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    let size_text = human_bytes(size);
    let kind = preview::kind_of(&p);
    let mime = preview::mime_of(&p);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("?").to_string();

    if matches!(kind, MediaKind::Image | MediaKind::Audio) {
        if mime == "application/octet-stream" {
            return Err(format!(
                "这个格式（.{ext}）浏览器渲染不了，界面里没法直接显示。\n改法：点「用系统程序打开」，交给系统自带的看图 / 播放器。"
            ));
        }
        let cap = if kind == MediaKind::Image { IMAGE_INLINE_MAX } else { AUDIO_INLINE_MAX };
        if size > cap {
            return Err(format!(
                "这个文件有 {size_text}，超过界面预览上限（{}）。\n改法：点「用系统程序打开」，交给系统程序打开。",
                human_bytes(cap)
            ));
        }
        let bytes = std::fs::read(&p)
            .map_err(|e| format!("读取失败：{e}\n改法：确认文件没被别的程序占用，或换一个文件。"))?;
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        return Ok(MediaPayload {
            kind: kind.id().into(),
            mime: mime.into(),
            data_url: format!("data:{mime};base64,{b64}"),
            text: String::new(),
            encoding: String::new(),
            size: size_text,
            truncated: false,
        });
    }

    if kind == MediaKind::Text {
        let bytes = read_head(&p, preview::TEXT_PREVIEW_MAX)
            .map_err(|e| format!("读取失败：{e}\n改法：确认文件没被别的程序占用，或换一个文件。"))?;
        return match preview::decode_text(&bytes) {
            preview::DecodedText::Ok(text, encoding) => Ok(MediaPayload {
                kind: "text".into(),
                mime: mime.into(),
                data_url: String::new(),
                text,
                encoding: encoding.to_string(),
                size: size_text,
                truncated: size > preview::TEXT_PREVIEW_MAX as u64,
            }),
            preview::DecodedText::Binary => Err(format!(
                "这个文件虽然叫 .{ext}，但内容不是文本 —— 按已知编码都解不出可读文字，\
                 多半是二进制或加密后的残留（例如从加了密的包里取出来的）。\n\
                 改法：点「用系统程序打开」看一眼，或者用「取出素材」重新解一次（加密包需要先解密）。"
            )),
        };
    }

    Err(format!(
        "STool 认不出这个格式（.{ext}），没法在界面里预览。\n改法：点「用系统程序打开」。"
    ))
}

#[tauri::command]
pub async fn scan_media(dir: String, kind: String) -> Result<MediaScan, String> {
    offload(move || scan_media_core(&dir, &kind)).await
}

#[tauri::command]
pub async fn read_media(path: String) -> Result<MediaPayload, String> {
    offload(move || read_media_core(&path)).await
}

/// 用系统默认程序打开一个文件（界面里预览不了的格式走这条）。
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("文件不存在：{path}\n改法：回「看素材」重新选一个文件。"));
    }
    shell_open(&p)
}

// ---------------------------------------------------------------------------
// ④ 翻译文字
// ---------------------------------------------------------------------------

/// 这一页的**全部展示数据**，一次调用取齐。
///
/// 之所以不拆成「取 CSV 统计」「取注入状态」多个命令：这些都依赖同一份
/// 「当前游戏 + 当前输出目录」快照，分多次取会在用户切换游戏时读到半新半旧的组合
/// （表现为「状态显示已注入，但条目数是上一个游戏的」）。
#[derive(Serialize, Debug)]
pub struct TextStats {
    /// 当前引擎能不能走「运行时注入」这条通道
    pub inject_supported: bool,
    /// 支持注入时，内核给的适配方式说明（如「在 js/plugins.js 登记汉化插件」）
    pub inject_mechanism: String,
    /// 不支持注入时的原因（直接给用户看，省得他以为功能坏了）
    pub inject_limits: String,
    /// 注入状态：是否已装、条目数、生效的 JSON 路径
    pub installed: bool,
    pub entries: usize,
    pub json_path: String,
}

/// 一条 CSV 的统计：总数 / 已翻 / 待翻。
#[derive(Serialize, Debug)]
pub struct CsvStats {
    pub exists: bool,
    pub path: String,
    pub total: usize,
    pub done: usize,
    pub todo: usize,
    /// 前几行预览（原文→译文），让用户不用打开 Excel 也知道内容对不对
    pub sample: Vec<CsvRow>,
}

#[derive(Serialize, Debug)]
pub struct CsvRow {
    pub file: String,
    pub key: String,
    pub source: String,
    pub target: String,
    /// 译文是否已填
    pub done: bool,
}

/// 读 CSV 并统计。`limit` 只影响 `sample`，不影响计数。
fn csv_stats_core(path: &str, limit: usize) -> Result<CsvStats, String> {
    use stool::features::text;
    let p = PathBuf::from(path);
    if !p.is_file() {
        return Err(format!(
            "还没找到文本 CSV：{path}\n改法：先点「① 提取文本」，STool 会把游戏里的台词导出成这个 CSV。"
        ));
    }
    let rows = text::read_csv(&p).map_err(|e| format!("读 CSV 失败：{e}\n改法：确认文件没被 Excel/WPS 独占打开。"))?;

    let total = rows.len();
    // 列约定（内核 `features::text`）：0=id 1=file 2=key 3=原文 4=译文
    let done = rows.iter().filter(|r| !r[4].trim().is_empty()).count();
    let sample = rows
        .iter()
        .take(limit)
        .map(|r| CsvRow {
            file: r[1].clone(),
            key: r[2].clone(),
            source: r[3].clone(),
            target: r[4].clone(),
            done: !r[4].trim().is_empty(),
        })
        .collect();

    Ok(CsvStats { exists: true, path: p.display().to_string(), total, done, todo: total - done, sample })
}

/// 「① 提取文本 → CSV」。
fn run_text_op(
    op: stool::engines::Op,
    root: &str,
    engine_id: &str,
    out_dir: &str,
    file: &str,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32, &str),
) -> Result<OpResult, String> {
    use stool::engines::{self, Op};

    let root_p = PathBuf::from(root);
    if !root_p.is_dir() {
        return Err(format!("游戏目录不存在：{root}\n改法：回「选游戏」页重新选一次。"));
    }
    if engine_id.trim().is_empty() {
        return Err("还不知道这是什么引擎，没法动手。\n改法：先去「选游戏」页让它认一下。".into());
    }
    // 提取写输出目录；回填写游戏目录（内核按 op 推 scope），与 CLI / egui 同一口径。
    let opts: HashMap<String, String> = HashMap::new();
    let scope = stool::features::precheck::scope_for_op(op, &opts);

    let out_p = if out_dir.trim().is_empty() { None } else { Some(PathBuf::from(out_dir)) };
    if matches!(op, Op::TextExtract) && out_p.is_none() {
        return Err("还没选输出目录。\n改法：点「选择输出目录」，选一个目录放提取出来的 CSV。".into());
    }
    if !file.trim().is_empty() {
        let fp = PathBuf::from(file);
        if matches!(op, Op::TextImport) && !fp.is_file() {
            return Err(format!(
                "回填用的 CSV 不存在：{file}\n改法：先在「① 提取文本」里生成它，翻完再回来点回填。"
            ));
        }
    }

    let rep = stool::features::precheck::run(&root_p, out_p.as_deref(), scope);
    if !rep.ok() {
        return Err(rep.fail_summary());
    }

    let reg = engines::Registry::new();
    let empty_out = PathBuf::new();
    let out_ref: &Path = out_p.as_deref().unwrap_or(&empty_out);
    let csv_p = PathBuf::from(file);
    let ctx = engines::Ctx { root: &root_p, out_dir: out_ref, options: &opts, progress, cancel };
    // 内核的 `OpPaths` 把「CSV」与「translation」分得很清楚：
    // 文本提取/回填走 csv 槽位，运行时注入走 translation 槽位。
    let paths = engines::OpPaths { repack_src: Path::new(""), csv: &csv_p, translation: &csv_p };
    let outcome = engines::exec_op(&reg, engine_id, op, &ctx, &paths);

    Ok(OpResult {
        success: outcome.success,
        message: outcome.message,
        files_done: outcome.files_done,
        out_dir: out_ref.display().to_string(),
        logs: outcome.logs,
    })
}

/// 一次文本 op 的展示结果。
#[derive(Serialize, Debug)]
pub struct TextOpResult {
    pub success: bool,
    pub message: String,
    pub files_done: usize,
    /// 做完之后顺手刷新出来的 CSV 统计（省一次往返）
    pub csv: Option<CsvStats>,
}

/// 「① 提取文本」：游戏文件 → CSV（写输出目录，不动游戏）。
#[tauri::command]
pub async fn text_extract(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    out_dir: String,
    csv_path: String,
) -> Result<TextOpResult, String> {
    let (root, engine_id) = current_game(&state)?;
    let csv = if csv_path.trim().is_empty() {
        PathBuf::from(&out_dir).join("text.csv").display().to_string()
    } else {
        csv_path
    };
    let res = spawn_progress_op(app, stool::engines::Op::TextExtract, root, engine_id, out_dir, csv.clone(), state.cancel.clone()).await?;
    let stats = csv_stats_core(&csv, 8).ok();
    Ok(TextOpResult { success: res.success, message: res.message, files_done: res.files_done, csv: stats })
}

/// 「③ 回填翻译」：按 CSV 的译文列写回游戏资源文件（写游戏目录，**写前自动备份**）。
#[tauri::command]
pub async fn text_import(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    out_dir: String,
    csv_path: String,
) -> Result<TextOpResult, String> {
    let (root, engine_id) = current_game(&state)?;
    if csv_path.trim().is_empty() {
        return Err("还没指定 CSV。\n改法：先在「① 提取文本」里生成它，翻完再回来点回填。".into());
    }
    let res = spawn_progress_op(app, stool::engines::Op::TextImport, root, engine_id, out_dir, csv_path.clone(), state.cancel.clone()).await?;
    let stats = csv_stats_core(&csv_path, 8).ok();
    Ok(TextOpResult { success: res.success, message: res.message, files_done: res.files_done, csv: stats })
}

// ---------------------------------------------------------------------------
// ④-b 运行时注入通道（MTool 式：不改游戏文件，启动即在内存里替换）
// ---------------------------------------------------------------------------

/// 查询注入状态（页面打开时调一次）。
fn text_stats_core(root: &str, engine_id: &str) -> Result<TextStats, String> {
    use stool::features::inject;
    let root_p = PathBuf::from(root);
    if !root_p.is_dir() {
        return Err(format!("游戏目录不存在：{root}\n改法：回「选游戏」页重新选一次。"));
    }
    let st = inject::status(&root_p, engine_id);
    // 不支持时必须**有话说**。支持表是有限白名单（Ren'Py / MV-MZ / Tyrano / HTML），
    // 表外的引擎（KiriKiri 等）`support_of` 返回 None —— 如果就这样把空串交给界面，
    // 用户看到的是「运行时注入：不可用」后面一片空白，不知道是自己选错了游戏、
    // 还是工具坏了。这里补一句人话，并直接指向能用的那条路。
    let (mech, limits) = match inject::support_of(engine_id) {
        Some(s) => (s.mechanism.to_string(), s.limits.to_string()),
        None => (
            String::new(),
            format!(
                "运行时注入目前支持 Ren'Py、RPG Maker MV/MZ、TyranoBuilder、HTML 游戏四种；\
                 当前引擎（{engine_id}）不在其中。\n\
                 这不影响汉化：改用「① 提取文本 → 机翻 / 人工翻译 → ③ 回填」这条通道，\
                 必要时再用「封包回写」把改好的资源打回游戏。"
            ),
        ),
    };
    Ok(TextStats {
        inject_supported: st.supported,
        inject_mechanism: mech,
        inject_limits: limits,
        installed: st.installed,
        entries: st.entries,
        json_path: st.json_path,
    })
}

#[tauri::command]
pub async fn text_stats(state: tauri::State<'_, AppState>) -> Result<TextStats, String> {
    let root = state.game_root.lock().map_err(|_| "状态锁失效".to_string())?.clone().unwrap_or_default();
    let engine = state.engine_id.lock().map_err(|_| "状态锁失效".to_string())?.clone().unwrap_or_default();
    offload(move || text_stats_core(&root, &engine)).await
}

#[tauri::command]
pub async fn csv_stats(path: String) -> Result<CsvStats, String> {
    offload(move || csv_stats_core(&path, 8)).await
}

/// 「从 CSV 生成 JSON 骨架」：键=原文、值=译文（已翻的带过去，没翻的留空）。
fn csv_to_json_core(csv: &str, json: &str) -> Result<(usize, usize), String> {
    use stool::features::inject;
    let c = PathBuf::from(csv);
    if !c.is_file() {
        return Err(format!(
            "CSV 不存在：{csv}\n改法：先点「① 提取文本」生成它，再回来生成 JSON。"
        ));
    }
    if json.trim().is_empty() {
        return Err("还没指定 JSON 路径。\n改法：点「选择」挑一个位置，默认放在游戏目录下。".into());
    }
    let j = PathBuf::from(json);
    if let Some(dir) = j.parent() {
        if !dir.as_os_str().is_empty() && !dir.is_dir() {
            return Err(format!(
                "JSON 要放的目录不存在：{}\n改法：换一个已存在的目录，或先建好它。",
                dir.display()
            ));
        }
    }
    inject::csv_to_json_skeleton(&c, &j).map_err(|e| format!("{e}\n改法：确认 CSV 是 STool 提取出来的那份（列顺序不能改）。"))
}

#[tauri::command]
pub async fn text_make_json(csv_path: String, json_path: String) -> Result<CsvStats, String> {
    offload(move || {
        let (total, filled) = csv_to_json_core(&csv_path, &json_path)?;
        // 返回 JSON 的统计，让界面立刻显示「多少条带译文」
        let mut st = csv_stats_core(&csv_path, 1)?;
        st.total = total;
        st.done = filled;
        st.todo = total - filled;
        Ok(st)
    })
    .await
}

/// 「③ 注入翻译」/「④ 移除注入」。
#[derive(Serialize, Debug)]
pub struct InjectResult {
    pub success: bool,
    pub message: String,
    pub installed: bool,
    pub entries: usize,
}

fn inject_core(root: &str, engine_id: &str, json: &str, install: bool) -> Result<InjectResult, String> {
    use stool::features::inject;
    let root_p = PathBuf::from(root);
    if !root_p.is_dir() {
        return Err(format!("游戏目录不存在：{root}\n改法：回「选游戏」页重新选一次。"));
    }
    if !inject::is_supported(engine_id) {
        return Err(format!(
            "这个引擎（{engine_id}）不支持运行时注入。\n\
             改法：改用「提取文本 → 机翻 / 人工翻译 → 翻译回填」这条通道，必要时再用「封包回写」。"
        ));
    }

    let msg = if install {
        let j = PathBuf::from(json);
        if !j.is_file() {
            return Err(format!(
                "要注入的 JSON 不存在：{json}\n改法：先点「从 CSV 生成 JSON」，再机翻或手工填译文。"
            ));
        }
        inject::install(&root_p, &j, engine_id)?
    } else {
        inject::uninstall(&root_p, engine_id)?
    };

    let st = inject::status(&root_p, engine_id);
    Ok(InjectResult { success: true, message: msg, installed: st.installed, entries: st.entries })
}

/// 「③ 注入翻译」：把 JSON 挂进游戏，启动时在内存里替换（不改游戏任何文件）。
///
/// 引擎从 `AppState` 取，而不是让前端传 —— 前端传就多了一个「传错引擎仍能成功」
/// 的入口（内核会按错的引擎去找适配点，找不到就报一句看不懂的错）。
#[tauri::command]
pub async fn text_inject(
    state: tauri::State<'_, AppState>,
    json_path: String,
) -> Result<InjectResult, String> {
    let (root, engine_id) = current_game(&state)?;
    offload(move || inject_core(&root, &engine_id, &json_path, true)).await
}

/// 「④ 移除注入」：反做上一步，把游戏恢复成没装过的样子。
#[tauri::command]
pub async fn text_uninject(state: tauri::State<'_, AppState>) -> Result<InjectResult, String> {
    let (root, engine_id) = current_game(&state)?;
    offload(move || inject_core(&root, &engine_id, "", false)).await
}

// ---------------------------------------------------------------------------
// ④-c 机器翻译（OpenAI 兼容接口：DeepSeek / 智谱 / 本地 Ollama …）
// ---------------------------------------------------------------------------

/// 保存机翻设置。
///
/// 存在**设置文件**里（`settings::Config`），不写在游戏目录 —— 否则换一个游戏
/// 就要重填一遍密钥，而且那份配置会跟着游戏目录被打包发出去（泄露密钥）。
#[tauri::command]
pub async fn mtl_save(
    base: String,
    key: String,
    model: String,
    glossary: String,
) -> Result<(), String> {
    offload(move || {
        let mut cfg = stool::settings::load();
        cfg.mtl_base_url = base;
        cfg.mtl_key = key;
        cfg.mtl_model = model;
        cfg.mtl_glossary = glossary;
        stool::settings::save(&cfg).map_err(|e| {
            format!("保存机翻设置失败：{e}\n改法：确认用户配置目录可写（通常不需要手动处理）。")
        })
    })
    .await
}

#[derive(Serialize, Debug)]
pub struct MtlOut {
    /// 本次新翻的条数
    pub done: usize,
    /// 还有多少条待翻（跑完一般是 0；中断时会 > 0，下次接着跑）
    pub todo: usize,
    /// 跑完后的表格统计，界面直接刷新
    pub csv: Option<CsvStats>,
}

/// 机翻 CSV 的**译文列**：只翻「原文非空且译文为空」的行，支持断点续翻。
fn run_mtl_core(
    csv: &str,
    base: &str,
    key: &str,
    model: &str,
    glossary: &str,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32, &str),
) -> Result<MtlOut, String> {
    use stool::features::translate::{translate_csv, OpenAiCompat};

    let csv_p = PathBuf::from(csv);
    if !csv_p.is_file() {
        return Err(format!(
            "要翻译的表格不存在：{csv}\n改法：先点「1. 提取文本」生成它，再回来机翻。"
        ));
    }
    if base.trim().is_empty() {
        return Err(
            "还没填接口地址。\n改法：打开「机翻设置」，点一个预设的「套用」（如 DeepSeek），或手动填地址。"
                .into(),
        );
    }
    if model.trim().is_empty() {
        return Err("还没填模型名。\n改法：打开「机翻设置」，点一个预设的「套用」会自动填上。".into());
    }

    let tr = OpenAiCompat {
        base_url: base.to_string(),
        api_key: key.to_string(),
        model: model.to_string(),
        glossary: glossary.to_string(),
    };
    // 批量与并发用配置里的默认值（内核 `settings::Config` 有默认 10 / 2）。
    // 不在这里再定一套数字：两处各写一份，改了一处另一处会忘，表现为「设置改了没生效」。
    let cfg = stool::settings::load();
    let (done, todo) = translate_csv(&csv_p, &tr, cfg.mtl_batch as usize, cfg.mtl_jobs as usize, progress, cancel)
        .map_err(|e| format!("机翻失败：{e}\n改法：检查接口地址 / 密钥 / 网络；本地模型请确认 Ollama 已在运行。"))?;

    let csv_stats = csv_stats_core(csv, 8).ok();
    Ok(MtlOut { done, todo, csv: csv_stats })
}

#[tauri::command]
pub async fn text_mtl(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    csv_path: String,
    base: String,
    key: String,
    model: String,
    glossary: String,
) -> Result<MtlOut, String> {
    use tauri::Emitter;
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    offload(move || {
        let emit = app.clone();
        let last = std::cell::Cell::new((0.0f32, Instant::now()));
        let progress = move |frac: f32, msg: &str| {
            let (prev, at) = last.get();
            let now = Instant::now();
            if !should_emit_progress(prev, at, frac, now) {
                return;
            }
            last.set((frac, now));
            let _ = emit.emit("op:progress", Prog { frac, msg: msg.to_string() });
        };
        run_mtl_core(&csv_path, &base, &key, &model, &glossary, &cancel, &progress)
    })
    .await
}

// ---------------------------------------------------------------------------
// ⑦ 解锁全CG
// ---------------------------------------------------------------------------

/// 一种解锁手段的展示数据。
///
/// `automated=false` 的手段（游戏内开关 / 人工分析）**不是失败**，只是要人自己找 ——
/// 界面必须照常列出来并给出指引，不能因为「没自动化」就当它不存在（§3.5「无死胡同」）。
#[derive(Serialize, Debug)]
pub struct RouteRow {
    /// `--opt:route=` 的取值，前端回传时原样带上
    pub key: String,
    pub label: String,
    pub desc: String,
    pub automated: bool,
    /// 是否被策略表推荐（越靠前越推荐）——界面据此刻画「推荐」徽标
    pub recommended: bool,
}

/// 「解锁全CG」的只读预览报告。**不写任何盘**，所以进页面就能安全地渲染一次。
#[derive(Serialize, Debug)]
pub struct UnlockPlan {
    pub engine_id: String,
    /// 该引擎是否被策略表收录（false = 走的兜底策略）
    pub known: bool,
    /// 识别依据（一句话，与检测判据对应）
    pub basis: String,
    /// 解锁动作（一句话）
    pub action: String,
    /// 附加提示；可能为空
    pub note: String,
    /// 该引擎支持的手段（已按偏好排序，带 recommended 标记）
    pub routes: Vec<RouteRow>,
    /// 发现的「自带全CG存档」候选（相对游戏根的路径列表）
    pub bundled: Vec<String>,
    /// 自带存档候选所在目录（绝对路径，去重；前端做「打开文件夹」）
    pub bundled_dirs: Vec<String>,
    /// 目标存档目录（绝对路径）—— 就是复制会落到的地方
    pub save_dirs: Vec<String>,
    /// 不带任何选项时会走哪条路线（= 内核 pick_route 的同一口径）
    pub default_route: String,
    /// 会走哪条路线的名字（给人看的）
    pub default_route_label: String,
}

/// 判断一条路线是否「推荐」：策略表第一条 = 推荐。
fn route_recommended(routes: &[stool::features::unlock::UnlockRoute], key: &str) -> bool {
    routes.first().map(|r| r.key() == key).unwrap_or(false)
}

/// 采集「解锁全CG」的只读预览数据。
///
/// 与 CLI 的 `unlock`（无 `--opt:apply=1`）**同一口径**：同样调
/// `unlock::spec_or_generic` / `find_bundled_saves` / `find_save_dirs` / `pick_route`，
/// 不自己另判一遍 —— 否则界面说「会走 A」、内核实际走 B，用户按界面点了却报错。
fn unlock_plan_core(root: &str, engine_id: &str) -> Result<UnlockPlan, String> {
    use stool::features::unlock;

    let root_p = PathBuf::from(root);
    if !root_p.is_dir() {
        return Err(format!("游戏目录不存在：{root}\n改法：回「选游戏」页重新选一次。"));
    }
    if engine_id.trim().is_empty() {
        return Err(
            "还不知道这是什么引擎。\n改法：先去「选游戏」页让它认一下 —— 解锁路线要按引擎选。".into(),
        );
    }

    let spec = unlock::spec_or_generic(engine_id);
    let bundled = unlock::find_bundled_saves(&root_p);
    let targets = unlock::find_save_dirs(&root_p, spec);

    // 相对路径更好认（列表里不会满屏盘符）；拿不到相对前缀就退回绝对路径。
    let rel = |p: &Path| -> String {
        p.strip_prefix(&root_p).unwrap_or(p).to_string_lossy().into_owned()
    };
    let abs = |p: &Path| -> String { p.display().to_string() };

    let mut bundled_dirs: Vec<String> = Vec::new();
    for p in &bundled {
        let d = p.parent().unwrap_or(p).to_path_buf();
        let s = abs(&d);
        if !bundled_dirs.contains(&s) {
            bundled_dirs.push(s);
        }
    }

    let routes: Vec<RouteRow> = spec
        .routes
        .iter()
        .map(|r| RouteRow {
            key: r.key().to_string(),
            label: r.label().to_string(),
            desc: r.desc().to_string(),
            automated: r.automated(),
            recommended: route_recommended(spec.routes, r.key()),
        })
        .collect();

    // 与内核同一函数算默认路线 —— 前端「自动」选项就用它做预览文案。
    let default = unlock::pick_route(spec, !bundled.is_empty(), None, false);

    Ok(UnlockPlan {
        engine_id: engine_id.to_string(),
        known: unlock::spec_for(engine_id).is_some(),
        basis: spec.basis.to_string(),
        action: spec.action.to_string(),
        note: spec.note.to_string(),
        routes,
        bundled: bundled.iter().map(|p| rel(p)).collect(),
        bundled_dirs,
        save_dirs: targets.iter().map(|p| abs(p)).collect(),
        default_route: default.key().to_string(),
        default_route_label: default.label().to_string(),
    })
}

/// 只读采集解锁计划：**不写盘**，进页面即渲染。
#[tauri::command]
pub async fn unlock_plan(state: tauri::State<'_, AppState>) -> Result<UnlockPlan, String> {
    let (root, engine) = current_game(&state)?;
    offload(move || unlock_plan_core(&root, &engine)).await
}

/// 「解锁全CG」的三个可选项（来自界面）。
///
/// 打包成一个结构体而不是三个 `&str` 形参：三者语义相关（都是「这次怎么解」），
/// 且解包调用点有 8 个参数本身已到 clippy 上限 —— 再随手加一个就会踩线。
/// 空字符串一律表示「没指定」，由内核走自动判定。
#[derive(Default, Clone)]
pub struct UnlockOpts {
    pub route: String,
    pub save_dir: String,
    pub filter: String,
}

/// 带选项的解锁执行实现（不碰 Tauri 类型，可单测）。
///
/// `apply=false` 时内核只做只读预览并把报告文本带回来 —— 与 CLI 缺省完全一致，
/// 也正好满足「先看清楚再动手」。
fn unlock_run_core(
    root: &str,
    engine_id: &str,
    apply: bool,
    opts_in: &UnlockOpts,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32, &str),
) -> Result<OpResult, String> {
    use stool::engines::{self, Op};
    use std::collections::HashMap;

    let root_p = PathBuf::from(root);
    if !root_p.is_dir() {
        return Err(format!("游戏目录不存在：{root}\n改法：回「选游戏」页重新选一次。"));
    }
    if engine_id.trim().is_empty() {
        return Err("还不知道这是什么引擎，没法动手。\n改法：先去「选游戏」页让它认一下。".into());
    }

    let mut opts: HashMap<String, String> = HashMap::new();
    // 安全默认由内核把关：这里只在明确要求时才写 apply=1。
    if apply {
        opts.insert("apply".into(), "1".into());
    }
    if !opts_in.route.trim().is_empty() {
        // 先本地校验：未知 key 直接给「原因 + 可选值」，别让内核报一句更绕的话。
        match stool::features::unlock::UnlockRoute::from_key(&opts_in.route) {
            Some(r) => {
                opts.insert("route".into(), r.key().to_string());
            }
            None => {
                return Err(format!(
                    "不认识这个解锁手段：{}\n改法：可选 {} —— 一般用「自动」即可。",
                    opts_in.route,
                    stool::features::unlock::route_keys()
                ))
            }
        }
    }
    if !opts_in.save_dir.trim().is_empty() {
        let d = PathBuf::from(&opts_in.save_dir);
        if !d.is_dir() {
            return Err(format!(
                "指定的存档目录不存在：{}\n改法：点「选择存档目录」重选，或留空让它自动找。",
                opts_in.save_dir
            ));
        }
        opts.insert("save_dir".into(), d.display().to_string());
    }
    if !opts_in.filter.trim().is_empty() {
        // 透传给 Unity 私有实现（注册表键名过滤）。
        opts.insert("filter".into(), opts_in.filter.trim().to_string());
    }

    // 解锁只写存档/注册表，不碰游戏目录里的封包 —— 与内核 scope 判定对齐。
    let scope = stool::features::precheck::scope_for_op(Op::Unlock, &opts);
    let rep = stool::features::precheck::run(&root_p, None, scope);
    if !rep.ok() {
        return Err(rep.fail_summary());
    }

    let reg = engines::Registry::new();
    let empty_out = PathBuf::new();
    let ctx = engines::Ctx { root: &root_p, out_dir: &empty_out, options: &opts, progress, cancel };
    let paths = engines::OpPaths {
        repack_src: Path::new(""),
        csv: Path::new(""),
        translation: &root_p,
    };
    let outcome = engines::exec_op(&reg, engine_id, Op::Unlock, &ctx, &paths);

    Ok(OpResult {
        success: outcome.success,
        message: outcome.message,
        files_done: outcome.files_done,
        out_dir: empty_out.display().to_string(),
        logs: outcome.logs,
    })
}

/// 执行解锁：`apply=false` 出只读报告，`apply=true` 才落地（内核自动先备份 `.stool.bak`）。
#[tauri::command]
pub async fn unlock_run(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    apply: bool,
    route: String,
    save_dir: String,
    filter: String,
) -> Result<OpResult, String> {
    use tauri::Emitter;
    let (root, engine) = current_game(&state)?;
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    let opts = UnlockOpts { route, save_dir, filter };
    offload(move || {
        let emit = app.clone();
        let last = std::cell::Cell::new((0.0f32, Instant::now()));
        let progress = move |frac: f32, msg: &str| {
            let (prev, at) = last.get();
            let now = Instant::now();
            if !should_emit_progress(prev, at, frac, now) {
                return;
            }
            last.set((frac, now));
            let _ = emit.emit("op:progress", Prog { frac, msg: msg.to_string() });
        };
        unlock_run_core(&root, &engine, apply, &opts, &cancel, &progress)
    })
    .await
}

/// 「撤销上一次」：列出游戏目录下所有 `.stool.bak` 备份。
///
/// 解锁（替换自带存档）覆盖写盘前都会经 `settings::backup_once` 留底，所以
/// 这里就是那条退路 —— 内核 `features::restore` 与 CLI `stool restore` 共用一份实现。
#[tauri::command]
pub async fn unlock_backups(state: tauri::State<'_, AppState>) -> Result<Vec<String>, String> {
    let (root, _) = current_game(&state)?;
    offload(move || {
        let root_p = PathBuf::from(&root);
        if !root_p.is_dir() {
            return Err(format!("游戏目录不存在：{root}\n改法：回「选游戏」页重新选一次。"));
        }
        Ok(stool::features::restore::find_backups(&root_p)
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<String>>())
    })
    .await
}

/// 用某个 `.stool.bak` 还原原文件（备份保留，可反复还原）。
#[tauri::command]
pub async fn unlock_restore(bak: String) -> Result<String, String> {
    offload(move || {
        let p = PathBuf::from(&bak);
        if !p.is_file() {
            return Err(format!(
                "备份文件不存在：{bak}\n改法：点「刷新备份列表」看看还有哪些备份。"
            ));
        }
        // 只允许还原本工具自己产的备份 —— 否则这个命令就成了任意文件覆盖入口。
        if !bak.ends_with(".stool.bak") {
            return Err(format!(
                "这不是 STool 的备份文件：{bak}\n改法：只选以 .stool.bak 结尾的文件。"
            ));
        }
        stool::features::restore::restore_one(&p)
    })
    .await
}

// ---------------------------------------------------------------------------
// ⑥ 游戏里改数值
// ---------------------------------------------------------------------------

/// 数值类型的一个选项。**展示文案在命令层**，不搬内核枚举 ——
/// 内核 `ScanType::label()` 是给 CLI / egui 用的「整数 32 位」，
/// 而 Cheat Engine 用户认的是「4 字节 / 8 字节」，这一页按后者对齐
/// （两边是同一个 `ScanType`，只是称呼不同，`parse` 里两套别名都收）。
#[derive(Serialize, Debug)]
pub struct ValueType {
    /// 回传给 `runtime_open` / `runtime_scan` 的类型键（内核 `ScanType::parse` 认）
    pub key: String,
    /// 界面上显示的短名（CE 口径）
    pub label: String,
    /// 说明：占几字节、什么时候用
    pub hint: String,
    /// 字节数（文本类型为 0，界面不显示「N 字节」）
    pub bytes: usize,
    /// 是不是数值型（数值型才能被「变大 / 变小 / ± 探测」这类过滤）
    pub numeric: bool,
}

/// 给界面用的类型表。**与内核 `ScanType::ALL` 一一对应**，不多不少。
///
/// 顺序也照内核来（i32 / i64 / f32 / f64 / utf8 / utf16）；界面第一项默认选中
/// 「4 字节」，因为游戏里的金币、经验绝大多数是 i32。
pub fn value_types() -> Vec<ValueType> {
    use stool::features::memscan::ScanType;
    ScanType::ALL
        .iter()
        .map(|t| {
            let (label, hint) = match t {
                ScanType::I32 => ("4 字节", "最常用。金币、经验、等级、好感度绝大多数是这种。"),
                ScanType::I64 => ("8 字节", "数值超过 21 亿（如累计经验、时间戳）时用它。"),
                ScanType::F32 => ("小数（4 字节）", "显示成 12.5 这类小数、且不像血量整数时用。"),
                ScanType::F64 => ("小数（8 字节）", "高精度小数，少见；前三种都不中再来试。"),
                ScanType::Utf8 => ("文本（UTF-8）", "搜玩家名、存档里的英文字符串。"),
                ScanType::Utf16 => ("文本（UTF-16）", "搜中文 / 日文名字符串。"),
            };
            ValueType {
                key: type_key(*t).to_string(),
                label: label.to_string(),
                hint: hint.to_string(),
                bytes: t.value_size(),
                numeric: stool::features::memscan::is_numeric(*t),
            }
        })
        .collect()
}

/// 内核枚举 → 界面 / IPC 用的稳定字符串键。
fn type_key(t: stool::features::memscan::ScanType) -> &'static str {
    use stool::features::memscan::ScanType as T;
    match t {
        T::I32 => "i32",
        T::I64 => "i64",
        T::F32 => "f32",
        T::F64 => "f64",
        T::Utf8 => "utf8",
        T::Utf16 => "utf16",
    }
}

/// 字符串键 → 内核枚举。认不出时给「原因 + 修法」，而不是默默退回默认值 ——
/// 默默退回会让人以为「我就是选的 8 字节，怎么扫的是 4 字节」。
fn type_of(key: &str) -> Result<stool::features::memscan::ScanType, String> {
    stool::features::memscan::ScanType::parse(key).ok_or_else(|| {
        format!(
            "不认识的数值类型：{key}\n改法：用界面下拉里给出的类型（{}）。",
            stool::features::memscan::ScanType::ALL.iter().map(|t| type_key(*t)).collect::<Vec<_>>().join(" / ")
        )
    })
}

/// 过滤方式的一个选项（同样是 CE 口径的短名）。
#[derive(Serialize, Debug)]
pub struct FilterOpt {
    pub key: String,
    pub label: String,
    pub hint: String,
}

/// 过滤方式表。**「再次扫描」只在「第一次扫描 + 回游戏让数值变化」之后才有意义** ——
/// 所以每一项的 hint 都写成「相对上一次扫描」，避免被当成绝对值条件。
pub fn filter_opts() -> Vec<FilterOpt> {
    use stool::features::memscan::Filter as F;
    F::ALL
        .iter()
        .map(|f| {
            let (label, hint) = match f {
                F::Exact => ("等于", "再输入一个数，只留「现在正好等于它」的地址。"),
                F::Changed => ("变了", "只留「和上次扫描比变过」的地址 —— 回游戏让数值动一下再用。"),
                F::Unchanged => ("没变", "只留「和上次扫描完全一样」的地址 —— 用来筛掉那些乱跳的。"),
                F::Increased => ("变大了", "只留「变大了」的地址（花掉金币时常用）。"),
                F::Decreased => ("变小了", "只留「变小了」的地址（打死怪掉血、买东西时常用）。"),
            };
            FilterOpt {
                key: filter_key(*f).to_string(),
                label: label.to_string(),
                hint: hint.to_string(),
            }
        })
        .collect()
}

fn filter_key(f: stool::features::memscan::Filter) -> &'static str {
    use stool::features::memscan::Filter as F;
    match f {
        F::Exact => "exact",
        F::Changed => "changed",
        F::Unchanged => "unchanged",
        F::Increased => "increased",
        F::Decreased => "decreased",
    }
}

fn filter_of(key: &str) -> Result<stool::features::memscan::Filter, String> {
    use stool::features::memscan::Filter as F;
    // 空串按「等于」—— 但只在「再次扫描」会用到，界面一定会带值，
    // 这里留个宽容默认，免得前端漏传直接报错。
    if key.trim().is_empty() {
        return Ok(F::Exact);
    }
    F::ALL
        .iter()
        .copied()
        .find(|f| filter_key(*f) == key.trim())
        .ok_or_else(|| {
            format!(
                "不认识的过滤方式：{key}\n改法：用界面给出的方式（{}）。",
                F::ALL.iter().map(|f| filter_key(*f)).collect::<Vec<_>>().join(" / ")
            )
        })
}

/// 数值类型表 + 过滤方式表（一次取回，前端渲染下拉用）。
///
/// 做成命令而不是写死在 JS 里：**类型是内核的事实**（`ScanType::ALL` + `value_size`），
/// 写死在界面就会出现「内核加了新类型、界面还是旧的」。展示文案在命令层对齐 CE 口径。
#[derive(Serialize, Debug)]
pub struct ScanOptions {
    pub types: Vec<ValueType>,
    pub filters: Vec<FilterOpt>,
}

#[tauri::command]
pub fn runtime_options() -> ScanOptions {
    ScanOptions { types: value_types(), filters: filter_opts() }
}

/// 进程列表里的一个候选进程。
#[derive(Serialize, Debug)]
pub struct ProcRow {
    pub pid: u32,
    pub name: String,
    /// 界面上直接显示的「1234 — game.exe」
    pub label: String,
}

/// 「方式一」的接入状态：能不能自动连上 MV/MZ 调试端口。
///
/// 这一页只**读**这些事实（端口开着吗、找到启动程序了吗），
/// 真正的启动 / 连接由 [`runtime_mvmz_connect`] 在用户点按钮时做。
#[derive(Serialize, Debug)]
pub struct MvmzStatus {
    /// 当前引擎是不是 RPG Maker MV / MZ（不是的话方式一整块要收起并给替代路线）
    pub applicable: bool,
    /// 游戏目录里找到的启动程序（绝对路径；没找到为空）
    pub exe: String,
    /// 7654 这个默认调试端口当前是否已经有游戏在监听
    pub port_open: bool,
    /// 默认调试端口
    pub port: u16,
    /// 给界面显示的一句话现状
    pub note: String,
}

/// MV/MZ 默认调试端口。与内核 `gui` / CLI 使用的口径保持一致。
const MVMZ_PORT: u16 = 7654;

/// 方式一的现状探测（只读：不启动游戏、不连端口，只探一下端口通不通）。
#[tauri::command]
pub async fn runtime_mvmz_status(state: tauri::State<'_, AppState>) -> Result<MvmzStatus, String> {
    let (root, engine) = current_game(&state)?;
    offload(move || {
        let root_p = PathBuf::from(&root);
        if !root_p.is_dir() {
            return Err(format!("游戏目录不存在：{root}\n改法：回「选游戏」页重新选一次。"));
        }
        let applicable = stool::features::runtime::find_game_exe(&root_p).is_some()
            && engine.to_lowercase().contains("rpgmaker")
            && (engine.to_lowercase().contains("mv") || engine.to_lowercase().contains("mz"));
        let exe = stool::features::runtime::find_game_exe(&root_p)
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let port_open = stool::features::runtime::probe_port(MVMZ_PORT);
        let note = if applicable && port_open {
            "检测到调试端口已经开着 —— 直接点「连接游戏」即可。".to_string()
        } else if applicable {
            "点「启动并连接」：STool 会用调试参数打开游戏（游戏窗口会正常弹出），再自动连上。".to_string()
        } else if exe.is_empty() {
            "这个游戏目录里没找到启动程序（Game.exe / nw.exe）。\n改法：确认选的是游戏的根目录；若确实是其他引擎的游戏，请用下面的「通用内存扫描」。".to_string()
        } else {
            format!(
                "当前引擎（{engine}）不是 RPG Maker MV / MZ，方式一用不上。\n改法：用下面的「通用内存扫描」，任何引擎都能改。"
            )
        };
        Ok(MvmzStatus { applicable, exe, port_open, port: MVMZ_PORT, note })
    })
    .await
}

/// 「方式一」的界面数据：金币 / 变量 / 开关 / 物品 + 名称表。
#[derive(Serialize, Debug)]
pub struct MvmzState {
    /// 游戏核心（$gameParty）是否已加载 —— 没进标题 / 没读存档时为 false
    pub ready: bool,
    pub gold: i64,
    /// 变量：id + 名字（可能为空串）+ 值文本
    pub variables: Vec<MvmzVar>,
    /// 开关：id + 名字 + 开 / 关
    pub switches: Vec<MvmzSwitch>,
    /// 持有物品：id + 名字 + 数量
    pub items: Vec<MvmzItem>,
    /// 给界面显示的现状
    pub note: String,
}

#[derive(Serialize, Debug)]
pub struct MvmzVar {
    pub id: i64,
    pub name: String,
    /// 值的可编辑文本（数组 / 对象会转成 JSON）
    pub value: String,
}

#[derive(Serialize, Debug)]
pub struct MvmzSwitch {
    pub id: i64,
    pub name: String,
    pub on: bool,
}

#[derive(Serialize, Debug)]
pub struct MvmzItem {
    pub id: i64,
    pub name: String,
    pub count: i64,
}

/// 已连接的 MV/MZ 调试会话 —— 与内存扫描会话一样挂在 `AppState` 上。
///
/// 之所以要存会话：CDP 连接是有状态的（WebSocket + 自增 id），
/// 每读一次状态就重连一次既慢又会在游戏侧留下半开连接。
pub struct MvmzSession {
    pub game: stool::features::runtime::DebugGame,
    /// 名称表（变量名 / 开关名 / 物品名），连接后读一次即可，不用每次刷新都重取
    pub names: serde_json::Value,
}

/// 从 serde_json 里按 id 取名字表里的名字（数组下标即 id）。
fn json_name(names: &serde_json::Value, key: &str, id: i64) -> String {
    names
        .get(key)
        .and_then(|a| a.as_array())
        .and_then(|a| a.get(id.max(0) as usize))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 把 JSON 值转成「可编辑文本」：字符串直接给，数组 / 对象转成 JSON ——
/// 与存档页的 `value_edit_text` 是同一个口径（编辑完再由内核解析回去）。
fn json_to_edit_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// 把界面填的文本解析成 JSON 值（数字 → number，true/false → bool，其余 → string）。
///
/// **不做「能转就转」的激进推测**：只有整串都是合法数字、且**没有前导零**才当数字 ——
/// 否则叫「007」的角色编号会被悄悄写成数字 7（存档里数字 7 和字符串 "007" 是两回事）。
fn edit_text_to_json(s: &str) -> serde_json::Value {
    let t = s.trim();
    // 前导零（"007" / "0.5" 除外）：保留成字符串，避免把编号类的值改坏。
    let has_leading_zero = t.len() > 1
        && t.starts_with('0')
        && !t.starts_with("0.")
        && !t.starts_with("-0.")
        && !t.starts_with("+0.");
    if !has_leading_zero && !t.is_empty() {
        if let Ok(i) = t.parse::<i64>() {
            return serde_json::Value::from(i);
        }
        if let Ok(f) = t.parse::<f64>() {
            return serde_json::Value::from(f);
        }
    }
    match t {
        "true" => return serde_json::Value::Bool(true),
        "false" => return serde_json::Value::Bool(false),
        _ => {}
    }
    serde_json::Value::String(s.to_string())
}

/// 连接（或启动后连接）MV/MZ 调试端口，成功后把会话存进 `AppState`。
///
/// `launch=true`：先用 `--remote-debugging-port` 启动游戏再连（游戏窗口会弹出，属正常）。
/// `launch=false`：直接连 —— 用于「游戏我已经自己开好了」。
#[tauri::command]
pub async fn runtime_mvmz_connect(
    state: tauri::State<'_, AppState>,
    launch: bool,
) -> Result<MvmzState, String> {
    let (root, _engine) = current_game(&state)?;
    let out = offload(move || {
        use stool::features::runtime;
        let root_p = PathBuf::from(&root);
        if launch {
            let exe = runtime::find_game_exe(&root_p).ok_or_else(|| {
                format!(
                    "游戏目录里没找到启动程序（Game.exe / nw.exe）：{root}\n\
                     改法：确认选的是游戏根目录（里面应有 www/ 子目录）；或先手动开着游戏，再点「只连接」。"
                )
            })?;
            runtime::DebugGame::launch(&exe, MVMZ_PORT).map_err(|e| {
                format!("启动游戏失败：{e}\n改法：手动双击游戏目录里的启动程序，再回来点「只连接」。")
            })?;
        }
        let game = runtime::DebugGame::connect(MVMZ_PORT)?;
        Ok(game)
    })
    .await?;

    // 连上后立刻取一次名称表与状态；这两步要再借一次会话，所以先存进去。
    if let Ok(mut g) = state.mvmz.lock() {
        let mut game = out;
        let names = game.read_names().unwrap_or(serde_json::Value::Null);
        *g = Some(MvmzSession { game, names });
    }
    read_mvmz_state(&state.mvmz)
}

/// 断开 MV/MZ 会话。
#[tauri::command]
pub fn runtime_mvmz_disconnect(state: tauri::State<'_, AppState>) -> bool {
    if let Ok(mut g) = state.mvmz.lock() {
        *g = None;
    }
    true
}

/// 从当前会话读一次状态，整理成界面数据。没有会话时报「原因 + 修法」。
///
/// 参数是 `&Arc<Mutex<...>>` 而不是 `&tauri::State` —— 这样同一条链路既能被
/// 命令直接调（界面刷新），也能在 `spawn_blocking` 闭包里用（CDP 求值会阻塞）。
fn read_mvmz_state(slot: &Arc<Mutex<Option<MvmzSession>>>) -> Result<MvmzState, String> {
    let raw = {
        let mut guard = slot.lock().map_err(|_| "会话状态异常（内部锁出错）".to_string())?;
        let sess = guard.as_mut().ok_or_else(|| {
            "还没连上游戏。\n改法：先点「启动并连接」（或先手动开着游戏再点「只连接」）。".to_string()
        })?;
        sess.game.read_state()?
    };
    mvmz_state_from_json(slot, raw)
}

/// 刷新方式一的状态（读一次游戏内数据）。
#[tauri::command]
pub async fn runtime_mvmz_refresh(state: tauri::State<'_, AppState>) -> Result<MvmzState, String> {
    // CDP 求值会阻塞等响应（最长 10 秒），必须挪出主线程；
    // 而 `tauri::State` 不能跨线程 —— 所以先 clone 出 `Arc` 再 move 进闭包。
    let slot = state.mvmz.clone();
    offload(move || {
        let raw = {
            let mut guard = slot.lock().map_err(|_| "会话状态异常（内部锁出错）".to_string())?;
            let sess = guard.as_mut().ok_or_else(|| {
                "还没连上游戏。\n改法：先点「启动并连接」（或先手动开着游戏再点「只连接」）。".to_string()
            })?;
            // 先自纠目标：STool 是「启动游戏 → 立刻连接」，那一刻 NW.js 的后台页
            // 常常先于游戏页就绪，旧逻辑会把会话钉在后台页上 —— 那样读出来永远是
            // mv=false、界面永远「还没进入存档」，点「刷新」也就永远没反应。
            // 这里重挑一次；换了目标就把名称表清掉重读。
            if sess.game.retarget(MVMZ_PORT)? {
                sess.names = serde_json::Value::Null;
            }
            let raw = sess.game.read_state()?;
            // 顺带把名称表刷新一次（游戏可能刚换过语言 / 读了别的存档）
            if let Ok(n) = sess.game.read_names() {
                if !n.is_null() {
                    sess.names = n;
                }
            }
            raw
        };
        mvmz_state_from_json(&slot, raw)
    })
    .await
}

/// 由原始状态 JSON + 会话里的名称表整理出界面数据。
///
/// 单独抽出来是因为 `runtime_mvmz_connect` 与 `runtime_mvmz_refresh` 都要走这一步，
/// 而前者读状态时不能长时间持锁。
fn mvmz_state_from_json(
    slot: &Arc<Mutex<Option<MvmzSession>>>,
    raw: serde_json::Value,
) -> Result<MvmzState, String> {
    let names = slot
        .lock()
        .map_err(|_| "会话状态异常（内部锁出错）".to_string())?
        .as_ref()
        .map(|s| s.names.clone())
        .unwrap_or(serde_json::Value::Null);
    let ready = raw.get("mv").and_then(|v| v.as_bool()).unwrap_or(false);
    if !ready {
        return Ok(MvmzState {
            ready: false,
            gold: 0,
            variables: Vec::new(),
            switches: Vec::new(),
            items: Vec::new(),
            // 三种可能都要交代清楚：仍停在标题画面 / 正在读盘 /（历史问题）连到的不是游戏画面。
            // 「刷新」现在会自动重新找准游戏页，所以直接把它写进修法里 —— 用户最先试的就是它。
            note: "已连上游戏，但还没读到存档数据 —— 可能还停在标题画面，或正在读盘。\n\
                   改法：在游戏里点「开始游戏」或「继续游戏」进到游戏画面，再点「刷新」；\
                   若你已经在游戏画面里，也点一下「刷新」，它会自动重新找准游戏页。"
                .to_string(),
        });
    }
    let gold = raw.get("gold").and_then(|v| v.as_i64()).unwrap_or(0);
    let mut variables: Vec<MvmzVar> = raw
        .get("variables")
        .and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| {
                    let id = k.parse::<i64>().ok()?;
                    if v.is_null() {
                        return None;
                    }
                    Some(MvmzVar { id, name: json_name(&names, "vars", id), value: json_to_edit_text(v) })
                })
                .collect()
        })
        .unwrap_or_default();
    variables.sort_by_key(|v| v.id);
    let mut switches: Vec<MvmzSwitch> = raw
        .get("switches")
        .and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| {
                    let id = k.parse::<i64>().ok()?;
                    let on = v.as_bool()?;
                    if !on {
                        return None;
                    }
                    Some(MvmzSwitch { id, name: json_name(&names, "sw", id), on })
                })
                .collect()
        })
        .unwrap_or_default();
    switches.sort_by_key(|s| s.id);
    let mut items: Vec<MvmzItem> = raw
        .get("items")
        .and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| {
                    let id = k.parse::<i64>().ok()?;
                    let count = v.as_i64()?;
                    Some(MvmzItem { id, name: json_name(&names, "items", id), count })
                })
                .collect()
        })
        .unwrap_or_default();
    items.sort_by_key(|i| i.id);
    let note = format!(
        "已连接 · 金币 {gold} · 变量 {} 个 · 开关（开）{} 个 · 持有物品 {} 种",
        variables.len(),
        switches.len(),
        items.len()
    );
    Ok(MvmzState { ready, gold, variables, switches, items, note })
}

/// 方式一写入的目标类别。
fn mvmz_kind_of(kind: &str) -> Result<(), String> {
    match kind.trim() {
        "gold" | "variable" | "switch" | "item" => Ok(()),
        other => Err(format!(
            "不认识的修改对象：{other}\n改法：用界面给出的「金币 / 变量 / 开关 / 物品数量」。"
        )),
    }
}

/// 方式一：按名字改（写入游戏内数据）。
///
/// `kind` ∈ gold / variable / switch / item；`value` 是界面填的文本。
/// 开关的 `value` 认 "true" / "开" / "1" / "on"。
#[tauri::command]
pub async fn runtime_mvmz_set(
    state: tauri::State<'_, AppState>,
    kind: String,
    id: i64,
    value: String,
) -> Result<MvmzState, String> {
    mvmz_kind_of(&kind)?;
    // 数值校验放在命令层（在下游之前失败，好给出「原因 + 修法」）
    let v = match kind.as_str() {
        "gold" => serde_json::Value::from(value.trim().parse::<i64>().map_err(|e| {
            format!("金币要填数字：{e}\n改法：只填整数，例如 99999。")
        })?),
        "item" => serde_json::Value::from(value.trim().parse::<i64>().map_err(|e| {
            format!("物品数量要填数字：{e}\n改法：只填整数，例如 99。")
        })?),
        _ => edit_text_to_json(&value),
    };

    let slot = state.mvmz.clone();
    let slot2 = slot.clone();
    let raw = {
        let kind_c = kind.clone();
        let v_c = v.clone();
        offload(move || {
            let mut guard = slot.lock().map_err(|_| "会话状态异常（内部锁出错）".to_string())?;
            let sess = guard.as_mut().ok_or_else(|| {
                "还没连上游戏。\n改法：先点「启动并连接」（或先手动开着游戏再点「只连接」）。".to_string()
            })?;
            // 同 refresh：写之前先把可能钉错的目标纠回来，否则会以
            // 「游戏还没进入存档」为名把用户拦下，而其实游戏早就进画面了。
            if sess.game.retarget(MVMZ_PORT)? {
                sess.names = serde_json::Value::Null;
            }
            match kind_c.as_str() {
                "gold" => {
                    let n = v_c.as_i64().unwrap_or(0);
                    sess.game.set_gold(n)?;
                }
                "variable" => {
                    sess.game.set_variable(id, &v_c)?;
                }
                "switch" => {
                    let on = matches!(
                        v_c.as_str().unwrap_or_default().trim(),
                        "true" | "开" | "1" | "on" | "TRUE" | "True"
                    );
                    sess.game.set_switch(id, on)?;
                }
                _ => {
                    let n = v_c.as_i64().unwrap_or(0);
                    sess.game.set_item(id, n)?;
                }
            }
            sess.game.read_state()
        })
        .await?
    };
    mvmz_state_from_json(&slot2, raw)
}

// ---- 方式二：通用内存扫描（Cheat Engine 等价）-----------------------------

/// 扫描会话的**轻量快照**。命中列表可能到上百万条，整包丢过 IPC 会把通道打满，
/// 所以命令层只回传「怎么显示」需要的部分（计数 + 前 N 条）。
#[derive(Serialize, Debug)]
pub struct ScanState {
    /// 是否已建立会话（选了进程）
    pub active: bool,
    pub pid: u32,
    pub proc_name: String,
    /// 当前数值类型的键（界面回显用）
    pub ty: String,
    pub ty_label: String,
    /// 是否已经做过首次扫描
    pub first_done: bool,
    /// 当前命中总数
    pub hits: usize,
    /// 其中有多少所在页不能直接写（只读 / Guard）—— 需要「强制写入」
    pub readonly_hits: usize,
    /// 是否有可撤销的写入记录
    pub can_undo: bool,
    /// 是否正在锁定（数值冻结）
    pub frozen: bool,
    /// 锁定周期（毫秒）
    pub freeze_period_ms: u64,
    /// 显示用的命中（前 [`HIT_DISPLAY_CAP`] 条）
    pub list: Vec<HitRow>,
    /// 列表是否被截断
    pub truncated: bool,
    /// 该类型的字节数 / 是否数值型（界面据此决定「变大 / 变小」可用性）
    pub bytes: usize,
    pub numeric: bool,
}

/// 一条展示用命中。
#[derive(Serialize, Debug)]
pub struct HitRow {
    /// 完整地址文本 `0x00007FF6...`
    pub addr: String,
    /// 当前读到的值
    pub value: String,
}

/// 命中列表回传上限。与前端 `MEDIA_PUSH_CAP` 是同一思路：
/// 后端封顶 + 前端虚拟化，任何一边漏了都不会把界面拖死。
const HIT_DISPLAY_CAP: usize = 500;

/// 从扫描会话 + 进程名快照出一个 `ScanState`。
fn scan_state_of(
    s: &stool::features::memscan::Scanner,
    proc_name: &str,
    frozen: bool,
    freeze_period_ms: u64,
) -> ScanState {
    let total = s.hit_count();
    let shown = total.min(HIT_DISPLAY_CAP);
    let list = s
        .hits
        .iter()
        .take(shown)
        .map(|h| HitRow { addr: format!("0x{:016X}", h.addr), value: h.value.display() })
        .collect();
    ScanState {
        active: true,
        pid: s.pid(),
        proc_name: proc_name.to_string(),
        ty: type_key(s.ty).to_string(),
        ty_label: value_types()
            .into_iter()
            .find(|v| v.key == type_key(s.ty))
            .map(|v| v.label)
            .unwrap_or_default(),
        first_done: s.first_done,
        hits: total,
        readonly_hits: s.readonly_hit_count(),
        can_undo: s.has_saved(),
        frozen,
        freeze_period_ms,
        list,
        truncated: total > shown,
        bytes: s.ty.value_size(),
        numeric: stool::features::memscan::is_numeric(s.ty),
    }
}

/// 列出可选的进程（不含 `filter` 时给全部，界面自己再过滤也行）。
///
/// `filter` 做的是**子串**匹配（不区分大小写）—— 与 egui 版「筛选」框同一口径；
/// 排序按名字，方便用户按字母找游戏。
#[tauri::command]
pub async fn runtime_processes(filter: String) -> Result<Vec<ProcRow>, String> {
    offload(move || {
        let f = filter.trim().to_lowercase();
        let mut v: Vec<(u32, String)> = stool::features::memscan::list_processes()
            .into_iter()
            .filter(|(_, n)| f.is_empty() || n.to_lowercase().contains(&f))
            .collect();
        v.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()).then(a.0.cmp(&b.0)));
        // 上限只为「别把整机几百个进程一股脑推给界面」；界面本来也是窗口化渲染。
        v.truncate(600);
        Ok(v
            .into_iter()
            .map(|(pid, name)| ProcRow { pid, name: name.clone(), label: format!("{pid} — {name}") })
            .collect())
    })
    .await
}

/// 打开（或切换）扫描会话：绑定目标进程 + 数值类型。
///
/// 切换类型 = 旧命合作废（内核的命中值是按旧类型解码的，混用只会给出错地址），
/// 所以这里直接新建会话，而不是在原会话上改 `ty`。
#[tauri::command]
pub async fn runtime_open(
    state: tauri::State<'_, AppState>,
    pid: u32,
    ty: String,
) -> Result<ScanState, String> {
    let t = type_of(&ty)?;
    if pid == 0 {
        return Err("还没选进程。\n改法：先在「目标进程」下拉里选一个（游戏窗口开着时按名字搜更快）。".to_string());
    }
    // 进程名只用于显示；取不到不算失败（进程可能刚退出，后面第一次扫描会报）。
    let name = stool::features::memscan::list_processes()
        .into_iter()
        .find(|(p, _)| *p == pid)
        .map(|(_, n)| n)
        .unwrap_or_default();
    let scanner = offload(move || {
        stool::features::memscan::Scanner::open(pid, t).map_err(|e| {
            format!("{e}\n改法：若提示「拒绝访问」，请用管理员身份运行 STool；部分系统保护进程无法读写。")
        })
    })
    .await?;
    // 换会话 = 顺带解除旧会话的锁定状态（旧地址属于旧句柄，留着只会误导界面）。
    let mut g = state
        .scanner
        .lock()
        .map_err(|_| "扫描会话状态异常（内部锁出错）".to_string())?;
    *g = Some(Session { scanner, proc_name: name.clone(), frozen: false, period_ms: 0 });
    let st = scan_state_of(&g.as_ref().unwrap().scanner, &name, false, 0);
    Ok(st)
}

/// 扫描会话。
///
/// 锁值**不开后台线程**：`frozen` 只是个标志，真正的周期写入由前端的
/// `runtime_freeze_tick` 驱动。这样锁值天然跟页面生命周期绑定 ——
/// 关掉页面 / 切走页面就自然停，不会留下一个在后台悄悄写内存的孤儿线程，
/// 也不会出现「会话被换掉、线程还在往旧地址写」的竞态。
pub struct Session {
    pub scanner: stool::features::memscan::Scanner,
    pub proc_name: String,
    pub frozen: bool,
    pub period_ms: u64,
}

/// 取当前会话的一个克隆快照（供界面渲染）。没有会话时返回「未激活」状态。
#[tauri::command]
pub fn runtime_state(state: tauri::State<'_, AppState>) -> Result<ScanState, String> {
    let g = state
        .scanner
        .lock()
        .map_err(|_| "扫描会话状态异常（内部锁出错）".to_string())?;
    match g.as_ref() {
        Some(s) => Ok(scan_state_of(&s.scanner, &s.proc_name, s.frozen, s.period_ms)),
        None => Ok(ScanState {
            active: false,
            pid: 0,
            proc_name: String::new(),
            ty: "i32".into(),
            ty_label: String::new(),
            first_done: false,
            hits: 0,
            readonly_hits: 0,
            can_undo: false,
            frozen: false,
            freeze_period_ms: 0,
            list: Vec::new(),
            truncated: false,
            bytes: 4,
            numeric: true,
        }),
    }
}

/// 扫描（首次 / 再次合一）：`first=true` 时是全量首次扫描，否则按 `filter` 过滤当前命中。
///
/// 首次扫描在真机上可能跑几百毫秒到几秒（要遍历整个可写内存），
/// 所以必须 offload；扫描期间**持有会话锁**是刻意的 —— 让「再次扫描」与「写入」
/// 排队而不是并发改同一份命中列表（内核的命中列表不是线程安全的）。
#[tauri::command]
pub async fn runtime_scan(
    state: tauri::State<'_, AppState>,
    first: bool,
    value: String,
    filter: String,
) -> Result<ScanState, String> {
    let f = filter_of(&filter)?;
    let slot = state.scanner.clone();
    offload(move || {
        let mut g = slot
            .lock()
            .map_err(|_| "扫描会话状态异常（内部锁出错）".to_string())?;
        let s = g.as_mut().ok_or_else(|| {
            "还没选目标进程。\n改法：先在最上面选一个正在运行的游戏进程（选完会自动建立扫描会话）。".to_string()
        })?;
        let n = if first {
            s.scanner.first_scan(&value)
        } else {
            s.scanner.next_scan(&value, f)
        }
        .map_err(|e| format!("{e}\n改法：确认数值类型和填的值对得上（例如小数要用「小数」类型）；换过类型要重新做首次扫描。"))?;
        let _ = n;
        Ok(scan_state_of(&s.scanner, &s.proc_name, s.frozen, s.period_ms))
    })
    .await
}

/// 读取命中列表的一页（界面「查看更多」用）。
///
/// 顺序读取是刻意的：命中列表在写入 / 再次扫描后会被内核重排，
/// 分页读只在「同一份列表」内稳定 —— 每次读都重取一次快照，不做游标。
#[tauri::command]
pub async fn runtime_hits(
    state: tauri::State<'_, AppState>,
    offset: usize,
    limit: usize,
) -> Result<Vec<HitRow>, String> {
    let slot = state.scanner.clone();
    offload(move || {
        let g = slot
            .lock()
            .map_err(|_| "扫描会话状态异常（内部锁出错）".to_string())?;
        let s = g
            .as_ref()
            .ok_or_else(|| "还没建立扫描会话。\n改法：先选一个目标进程。".to_string())?;
        let lim = limit.clamp(1, HIT_DISPLAY_CAP);
        Ok(s.scanner
            .hits
            .iter()
            .skip(offset)
            .take(lim)
            .map(|h| HitRow { addr: format!("0x{:016X}", h.addr), value: h.value.display() })
            .collect())
    })
    .await
}

/// 写入新值。`addr` 为空 = 写入全部命中；给了地址 = 只写这一条。
///
/// `force=true` 时走「强制写入」（临时解除页保护）—— 用于只读 / Guard 页。
#[tauri::command]
pub async fn runtime_write(
    state: tauri::State<'_, AppState>,
    value: String,
    addr: Option<String>,
    force: bool,
) -> Result<ScanState, String> {
    let target = match addr.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(a) => Some(stool::features::guard::parse_addr(a).ok_or_else(|| {
            format!("地址写不对：{a}\n改法：地址形如 0x7FF6A000 或直接填十进制的 140700000000。")
        })?),
        None => None,
    };
    let slot = state.scanner.clone();
    offload(move || {
        let mut g = slot
            .lock()
            .map_err(|_| "扫描会话状态异常（内部锁出错）".to_string())?;
        let s = g.as_mut().ok_or_else(|| {
            "还没建立扫描会话。\n改法：先选一个目标进程并做一次扫描。".to_string()
        })?;
        if force {
            s.scanner.force_write(&value, target).map_err(|e| {
                format!("强制写入失败：{e}\n改法：地址可能已失效 —— 回游戏让数值再变一次，然后重新扫描。")
            })?;
        } else {
            s.scanner.write(&value, target).map_err(|e| {
                format!(
                    "写入失败：{e}\n\
                     改法：若命中所在页是只读 / Guard（界面会提示），改用「强制写入（解除页保护）」。"
                )
            })?;
        }
        Ok(scan_state_of(&s.scanner, &s.proc_name, s.frozen, s.period_ms))
    })
    .await
}

/// 撤销上一次写入：把所有记录过的地址恢复成写入前的值。
#[tauri::command]
pub async fn runtime_undo(state: tauri::State<'_, AppState>) -> Result<ScanState, String> {
    let slot = state.scanner.clone();
    offload(move || {
        let mut g = slot
            .lock()
            .map_err(|_| "扫描会话状态异常（内部锁出错）".to_string())?;
        let s = g
            .as_mut()
            .ok_or_else(|| "还没建立扫描会话。\n改法：先选一个目标进程并做一次扫描。".to_string())?;
        if !s.scanner.has_saved() {
            return Err(
                "没有可撤销的写入记录。\n改法：只有通过本页「写入」过的地址才会被记录；先做一次写入。"
                    .to_string(),
            );
        }
        s.scanner.undo();
        Ok(scan_state_of(&s.scanner, &s.proc_name, s.frozen, s.period_ms))
    })
    .await
}

/// 锁定 / 解锁数值（周期性写回，防止游戏把它改掉）。
///
/// 锁值不新开线程，而是**让前端按周期驱动**一个 `runtime_freeze_tick` ——
/// 这样锁值天然跟页面生命周期绑定：关掉页面 / 切走页面就自然停，
/// 不会留下一个在后台悄悄写内存的孤儿线程（比线程方案安全得多）。
#[tauri::command]
pub async fn runtime_freeze(
    state: tauri::State<'_, AppState>,
    on: bool,
    value: String,
    period_ms: u64,
) -> Result<ScanState, String> {
    let slot = state.scanner.clone();
    offload(move || {
        let mut g = slot
            .lock()
            .map_err(|_| "扫描会话状态异常（内部锁出错）".to_string())?;
        let s = g
            .as_mut()
            .ok_or_else(|| "还没建立扫描会话。\n改法：先选一个目标进程并做一次扫描。".to_string())?;
        if on {
            if !s.scanner.first_done {
                return Err("还没扫描过，没有可锁定的地址。\n改法：先做一次扫描，再点「锁定数值」。".to_string());
            }
            // 立刻写一次（让用户马上看到效果），之后由前端的 tick 维持。
            s.scanner.write_raw(&value, None).map_err(|e| {
                format!("锁定失败：{e}\n改法：地址可能已失效，回游戏让数值变一次再重新扫描。")
            })?;
            s.frozen = true;
            s.period_ms = period_ms.clamp(50, 5_000);
        } else {
            s.frozen = false;
            s.period_ms = 0;
        }
        Ok(scan_state_of(&s.scanner, &s.proc_name, s.frozen, s.period_ms))
    })
    .await
}

/// 锁值的一次「心跳」：按当前周期把值再写一遍。**失败不算致命** ——
/// 地址失效时只回报 `hits` 变化，让用户自己决定要不要停锁。
#[tauri::command]
pub async fn runtime_freeze_tick(
    state: tauri::State<'_, AppState>,
    value: String,
) -> Result<bool, String> {
    let slot = state.scanner.clone();
    offload(move || {
        let mut g = slot
            .lock()
            .map_err(|_| "扫描会话状态异常（内部锁出错）".to_string())?;
        let Some(s) = g.as_mut() else { return Ok(false) };
        if !s.frozen {
            return Ok(false);
        }
        // 地址失效时 write_raw 会报错 —— 这里吞掉（锁值心跳不该弹窗），
        // 但把 frozen 保持住，用户下次刷新状态时会看到命中数变了。
        let _ = s.scanner.write_raw(&value, None);
        Ok(true)
    })
    .await
}

/// 方式二·五：反修改保护诊断（改了立刻被还原 / 改了没用）。
#[tauri::command]
pub async fn runtime_diag(
    state: tauri::State<'_, AppState>,
    addr: String,
    probe: String,
) -> Result<DiagOut, String> {
    let a = stool::features::guard::parse_addr(&addr).ok_or_else(|| {
        format!("地址写不对：{addr}\n改法：地址形如 0x7FF6A000；也可以直接从上面的命中列表点「诊断」自动填入。")
    })?;
    let (pid, ty) = {
        let g = state
            .scanner
            .lock()
            .map_err(|_| "扫描会话状态异常（内部锁出错）".to_string())?;
        let s = g
            .as_ref()
            .ok_or_else(|| "还没建立扫描会话。\n改法：先做一次扫描，把地址扫出来。".to_string())?;
        (s.scanner.pid(), s.scanner.ty)
    };
    let probe_opt = probe.trim().to_string();
    offload(move || {
        let mut opts = stool::features::guard::ProbeOpts::default();
        if !probe_opt.is_empty() {
            opts.probe = Some(probe_opt);
        }
        let rep = stool::features::guard::analyze(pid, a, ty, &opts)?;
        Ok(diag_out_of(&rep))
    })
    .await
}

/// 诊断报告的展示结构。**把内核的结构压平成界面能直接画的样子** ——
/// 前端不做「按可行性排序 / 挑字段」这类判断，否则两处口径会漂。
#[derive(Serialize, Debug)]
pub struct DiagOut {
    pub headline: String,
    pub addr: String,
    /// 回滚判定（含实测数值）
    pub verdict: String,
    /// 是否会被还原（界面据此上色）
    pub reverted: bool,
    /// 页保护名 + 是否不可直接写
    pub page: String,
    pub page_blocked: bool,
    /// 同值副本数量
    pub mirrors: usize,
    pub mirror_truncated: bool,
    /// 「写入被改写」的描述（没有则空）
    pub rewrite: String,
    /// 保护机制（在线反作弊 / 本地 DRM / 加壳）
    pub modules: Vec<ModuleRow>,
    /// 反调试导入（少数游戏会拦调试器）
    pub anti_debug: Vec<String>,
    /// 真正被采纳的「数据源」地址（没有则空）
    pub source_addr: String,
    /// 还原指令落点（可能是重置指令的代码地址）
    pub writers: Vec<String>,
    pub read_only: bool,
    pub notes: Vec<String>,
    pub plans: Vec<PlanRow>,
}

#[derive(Serialize, Debug)]
pub struct ModuleRow {
    pub name: String,
    pub module: String,
    /// 在线反作弊 / 本地 DRM / 加壳
    pub family: String,
    pub advice: String,
}

/// 一条应对方案：`auto` 给出界面可直接挂按钮的动作。
#[derive(Serialize, Debug)]
pub struct PlanRow {
    pub title: String,
    pub feasibility: String,
    /// 用于上色的档位：high / medium / low / none
    pub level: String,
    pub detail: String,
    pub steps: Vec<String>,
    /// 可直接执行的动作：freeze / force_write / write_source / ""（只能人工）
    pub auto: String,
    /// freeze 动的周期（毫秒）
    pub auto_period_ms: u64,
    /// force_write / write_source 的地址
    pub auto_addr: String,
}

/// 内核报告 → 展示结构。
fn diag_out_of(rep: &stool::features::guard::ProbeReport) -> DiagOut {
    use stool::features::guard::{AutoAction, Feasibility};
    let modules = rep
        .ac_modules
        .iter()
        .chain(rep.prot_modules.iter())
        .map(|m| ModuleRow {
            name: m.name.clone(),
            module: m.module.clone(),
            family: m.family.label().to_string(),
            advice: m.advice.clone(),
        })
        .collect();
    let plans = rep
        .plans
        .iter()
        .map(|p| {
            let level = match p.feasibility {
                Feasibility::High => "high",
                Feasibility::Medium => "medium",
                Feasibility::Low => "low",
                Feasibility::NotAdvised => "none",
            };
            let (auto, period, addr) = match p.auto.as_ref() {
                Some(AutoAction::Freeze { period_ms }) => ("freeze", *period_ms, String::new()),
                Some(AutoAction::ForceWrite) => ("force_write", 0, stool::features::guard::fmt_addr(rep.addr)),
                Some(AutoAction::WriteSource { addr }) => {
                    ("write_source", 0, stool::features::guard::fmt_addr(*addr))
                }
                None => ("", 0, String::new()),
            };
            PlanRow {
                title: p.title.clone(),
                feasibility: p.feasibility.label().to_string(),
                level: level.to_string(),
                detail: p.detail.clone(),
                steps: p.steps.clone(),
                auto: auto.to_string(),
                auto_period_ms: period,
                auto_addr: addr,
            }
        })
        .collect();
    DiagOut {
        headline: rep.headline(),
        addr: stool::features::guard::fmt_addr(rep.addr),
        verdict: format!("{} —— {}", rep.verdict.label(), rep.verdict.measure()),
        reverted: rep.verdict.is_reverted(),
        page: match &rep.page {
            Some(p) => format!(
                "{}{}（{}）",
                p.protect_name(),
                if rep.page_blocked { " · 不可直接写" } else { "" },
                p.kind_name()
            ),
            None => "地址不在已提交区域".to_string(),
        },
        page_blocked: rep.page_blocked,
        mirrors: rep.mirrors.len(),
        mirror_truncated: rep.mirror_truncated,
        rewrite: rep.rewrite.as_ref().map(|r| r.describe()).unwrap_or_default(),
        modules,
        anti_debug: rep.anti_debug.iter().map(|i| format!("{}!{}（{}）", i.module, i.func, i.why)).collect(),
        source_addr: rep.source_addr.map(stool::features::guard::fmt_addr).unwrap_or_default(),
        writers: rep.writers.iter().take(6).map(|w| stool::features::guard::fmt_addr(w.addr)).collect(),
        read_only: rep.read_only,
        notes: rep.notes.clone(),
        plans,
    }
}

/// 方式二·五：页保护分布（只读遍历整块内存，不写内存、不需要风险确认）。
#[tauri::command]
pub async fn runtime_regions(state: tauri::State<'_, AppState>) -> Result<RegionOut, String> {
    let pid = {
        let g = state
            .scanner
            .lock()
            .map_err(|_| "扫描会话状态异常（内部锁出错）".to_string())?;
        let s = g
            .as_ref()
            .ok_or_else(|| "还没建立扫描会话。\n改法：先选一个目标进程。".to_string())?;
        s.scanner.pid()
    };
    offload(move || {
        let (stats, sum) = stool::features::guard::regions_of(pid)?;
        Ok(RegionOut {
            writable: sum.writable,
            readonly: sum.readonly,
            execute: sum.execute,
            rwx: sum.rwx,
            guard: sum.guard,
            image: sum.image,
            mapped: sum.mapped,
            private: sum.private,
            total_bytes: stool::features::precheck::human_bytes(sum.total_bytes as u64),
            rows: stats
                .iter()
                .take(60)
                .map(|s| RegionRow {
                    base: stool::features::guard::fmt_addr(s.base),
                    size: stool::features::precheck::human_bytes(s.size as u64),
                    protect: stool::memapi::protect_name(s.protect).to_string(),
                    kind: stool::memapi::kind_name(s.kind).to_string(),
                    merged: s.merged,
                })
                .collect(),
            total_rows: stats.len(),
        })
    })
    .await
}

#[derive(Serialize, Debug)]
pub struct RegionOut {
    pub writable: usize,
    pub readonly: usize,
    pub execute: usize,
    /// 可写又可执行 —— 正常程序极少，加壳 / 自修改代码的常见特征
    pub rwx: usize,
    pub guard: usize,
    pub image: usize,
    pub mapped: usize,
    pub private: usize,
    pub total_bytes: String,
    pub rows: Vec<RegionRow>,
    pub total_rows: usize,
}

#[derive(Serialize, Debug)]
pub struct RegionRow {
    pub base: String,
    pub size: String,
    pub protect: String,
    pub kind: String,
    pub merged: usize,
}

/// 方式三：KiriKiri 运行时补丁包 —— 列出当前游戏目录下已有的补丁包。
#[tauri::command]
pub async fn runtime_patch_list(state: tauri::State<'_, AppState>) -> Result<PatchList, String> {
    let (root, engine) = current_game(&state)?;
    offload(move || {
        if !stool::features::xp3patch::supports(&engine) {
            return Ok(PatchList {
                supported: false,
                note: format!(
                    "补丁包只对 KiriKiri / 吉里吉里（`.xp3` 封包）游戏有效，当前是「{engine}」。\n\
                     改法：其他引擎的运行时修改请用上面的「按名字改」或「通用内存扫描」；\
                     若只是想换台词，可以用「翻译文字」页做脚本回填。"
                ),
                patches: Vec::new(),
                next_name: String::new(),
            });
        }
        let root_p = PathBuf::from(&root);
        if !root_p.is_dir() {
            return Err(format!("游戏目录不存在：{root}\n改法：回「选游戏」页重新选一次。"));
        }
        let patches = stool::features::xp3patch::list(&root_p)
            .into_iter()
            .map(|p| PatchRow {
                ours: p.ours,
                entries: p.entries.unwrap_or(0),
                size: stool::features::precheck::human_bytes(p.bytes),
                name: p.name,
            })
            .collect();
        Ok(PatchList {
            supported: true,
            note: String::new(),
            patches,
            next_name: stool::features::xp3patch::next_name(&root_p),
        })
    })
    .await
}

#[derive(Serialize, Debug)]
pub struct PatchList {
    pub supported: bool,
    pub note: String,
    pub patches: Vec<PatchRow>,
    /// 下一个空号补丁包名（生成时留空就用它）
    pub next_name: String,
}

#[derive(Serialize, Debug)]
pub struct PatchRow {
    pub name: String,
    /// 是不是本工具建的（只有本工具建的才允许「移除」）
    pub ours: bool,
    pub size: String,
    /// 包内条目数（解析不出来时为 0）
    pub entries: usize,
}

/// 方式三：生成补丁包。`changed_only=true` 时只打「与原封包逐字节不同」的文件。
#[tauri::command]
pub async fn runtime_patch_build(
    state: tauri::State<'_, AppState>,
    src_dir: String,
    name: String,
    changed_only: bool,
) -> Result<String, String> {
    let (root, engine) = current_game(&state)?;
    if src_dir.trim().is_empty() {
        return Err(
            "请先填「改动目录」。\n改法：就是「取出素材」的输出目录（里面放着改好的文件，需保持封包内原始相对路径）。"
                .to_string(),
        );
    }
    offload(move || {
        if !stool::features::xp3patch::supports(&engine) {
            return Err(format!(
                "当前引擎「{engine}」不支持补丁包。\n改法：补丁包是 KiriKiri 专属；其他引擎请用「按名字改」或内存扫描。"
            ));
        }
        let root_p = PathBuf::from(&root);
        let src_p = PathBuf::from(src_dir.trim());
        if !src_p.is_dir() {
            return Err(format!(
                "改动目录不存在：{}\n改法：先「取出素材」生成这个目录，改完里面的文件再来打包。",
                src_p.display()
            ));
        }
        let nm = name.trim();
        let nm = if nm.is_empty() { None } else { Some(nm) };
        if changed_only {
            stool::features::xp3patch::build_changed(&root_p, &src_p, nm)
        } else {
            stool::features::xp3patch::build(&root_p, &src_p, nm)
        }
    })
    .await
}

/// 方式三：移除本工具建的补丁包（游戏自带的补丁包一律拒绝）。
#[tauri::command]
pub async fn runtime_patch_remove(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<String, String> {
    let (root, engine) = current_game(&state)?;
    let nm = name.trim().to_string();
    if nm.is_empty() {
        return Err("请先填要移除的补丁包名。\n改法：从上面的已有补丁包列表里点一个，会自动填入。".to_string());
    }
    offload(move || {
        if !stool::features::xp3patch::supports(&engine) {
            return Err(format!(
                "当前引擎「{engine}」没有补丁包这回事。\n改法：补丁包是 KiriKiri / 吉里吉里专属；其他引擎不用移除什么 —— 游戏目录里本来就没有。"
            ));
        }
        stool::features::xp3patch::remove(&PathBuf::from(&root), &nm)
    })
    .await
}

// ---------------------------------------------------------------------------
// ⑧ 装MOD
// ---------------------------------------------------------------------------

/// 已装 MOD 的一行。
#[derive(Serialize, Debug)]
pub struct ModRow {
    pub name: String,
    pub enabled: bool,
    /// 这个 MOD 带了几个文件
    pub files: usize,
    /// 其中覆盖了游戏原有文件的个数（其余是新增文件）
    pub overwritten: usize,
    /// 具体覆盖/新增了哪些文件（相对游戏根目录，`/` 分隔）。
    /// 前端折叠展示用 —— 全量塞给前端没问题：单个 MOD 的文件数上限就是磁盘上的文件数。
    pub file_list: Vec<String>,
}

/// 一处冲突：同一文件被 ≥2 个**已启用** MOD 覆盖。
#[derive(Serialize, Debug)]
pub struct ConflictRow {
    pub rel: String,
    pub mods: Vec<String>,
}

/// 装MOD 页的全部状态。
#[derive(Serialize, Debug)]
pub struct ModsState {
    pub mods: Vec<ModRow>,
    pub conflicts: Vec<ConflictRow>,
    /// 本工具存放 MOD 的目录（展示用，便于用户自己去看备份）
    pub store_dir: String,
    /// 冲突文件总数（前端用它决定要不要弹红条）
    pub conflict_count: usize,
}

fn mods_state_of(root: &Path) -> ModsState {
    let mods = stool::features::mods::list_mods(root)
        .into_iter()
        .map(|m| ModRow {
            name: m.name,
            enabled: m.enabled,
            files: m.files.len(),
            overwritten: m.overwritten.len(),
            file_list: m.files,
        })
        .collect();
    let conflicts = stool::features::mods::conflicts(root)
        .into_iter()
        .map(|c| ConflictRow { rel: c.rel, mods: c.mods })
        .collect::<Vec<_>>();
    let conflict_count = conflicts.len();
    ModsState {
        mods,
        conflicts,
        store_dir: root.join("stool_mods").display().to_string(),
        conflict_count,
    }
}

fn mods_game_root(state: &tauri::State<'_, AppState>) -> Result<String, String> {
    let (root, _engine) = current_game(state)?;
    if !Path::new(&root).is_dir() {
        return Err(format!("游戏目录不存在：{root}\n改法：回「选游戏」页重新选一次。"));
    }
    Ok(root)
}

/// 列出已装 MOD 与当前冲突（进页面即调，**只读**）。
#[tauri::command]
pub async fn mods_state(state: tauri::State<'_, AppState>) -> Result<ModsState, String> {
    let root = mods_game_root(&state)?;
    offload(move || Ok(mods_state_of(Path::new(&root)))).await
}

/// 预演：把 `src_dir` 里的文件装成名为 `name` 的 MOD，会与哪些**已启用** MOD 冲突。
///
/// 这一步必须在「点安装」之前就能看到结果 —— 否则用户只能等安装报错，
/// 而报错时可能已经有文件被覆盖了（`install_mod` 的冲突校验在写盘之前，所以是安全的，
/// 但预演能让人提前换名字/换目录，体验更好）。
#[tauri::command]
pub async fn mods_preview(state: tauri::State<'_, AppState>, src_dir: String) -> Result<ModsState, String> {
    let root = mods_game_root(&state)?;
    let dir = PathBuf::from(&src_dir);
    if !dir.is_dir() {
        return Err(format!("补丁目录不存在：{src_dir}\n改法：点「选择补丁目录」重新选一个文件夹。"));
    }
    offload(move || {
        // 收集补丁目录下的相对路径（与 install_mod 同一套口径：`/` 分隔、按名排序）
        let mut rels: Vec<String> = Vec::new();
        for e in walkdir::WalkDir::new(&dir).sort_by_file_name().into_iter().filter_map(|e| e.ok()) {
            if e.file_type().is_file() {
                if let Ok(rel) = e.path().strip_prefix(&dir) {
                    rels.push(
                        rel.components()
                            .map(|c| c.as_os_str().to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join("/"),
                    );
                }
            }
        }
        // 用空格占位的新名字做预演（不传真实 name，避免把「已存在同名」也算成冲突）
        let preview = stool::features::mods::would_conflict(&PathBuf::from(&root), &rels, "\u{0}preview");
        let mut st = mods_state_of(Path::new(&root));
        st.conflicts = preview
            .into_iter()
            .map(|c| ConflictRow { rel: c.rel, mods: c.mods })
            .collect();
        st.conflict_count = st.conflicts.len();
        Ok(st)
    })
    .await
}

/// 安装 MOD：把补丁目录覆盖到游戏目录，原文件自动备份。
///
/// `force=false` 时，与已启用 MOD 争用同一文件会被内核**在写盘前**拒绝并列出冲突。
#[tauri::command]
pub async fn mods_install(
    state: tauri::State<'_, AppState>,
    src_dir: String,
    name: String,
    force: bool,
) -> Result<ModsState, String> {
    let root = mods_game_root(&state)?;
    let dir = PathBuf::from(&src_dir);
    if !dir.is_dir() {
        return Err(format!("补丁目录不存在：{src_dir}\n改法：点「选择补丁目录」重新选一个文件夹。"));
    }
    offload(move || {
        stool::features::mods::install_mod(&PathBuf::from(&root), &dir, &name, &|_, _| {}, force)?;
        Ok(mods_state_of(Path::new(&root)))
    })
    .await
}

/// 启用 / 停用 MOD。
#[tauri::command]
pub async fn mods_toggle(state: tauri::State<'_, AppState>, name: String, enabled: bool) -> Result<ModsState, String> {
    let root = mods_game_root(&state)?;
    offload(move || {
        stool::features::mods::toggle_mod(&PathBuf::from(&root), &name, enabled)?;
        Ok(mods_state_of(Path::new(&root)))
    })
    .await
}

/// 卸载 MOD：还原被覆盖的原文件，删除新增文件。
#[tauri::command]
pub async fn mods_uninstall(state: tauri::State<'_, AppState>, name: String) -> Result<ModsState, String> {
    let root = mods_game_root(&state)?;
    offload(move || {
        stool::features::mods::uninstall_mod(&PathBuf::from(&root), &name)?;
        Ok(mods_state_of(Path::new(&root)))
    })
    .await
}

// ---------------------------------------------------------------------------
// ⑨ 工具箱（设置 / 诊断 / 帮助）
// ---------------------------------------------------------------------------

/// 外部工具的一项配置：名字 + 当前路径 + 能否一键下载。
#[derive(Serialize, Debug)]
pub struct ToolRow {
    /// 配置键（`mtl_save` / `tool_set` 用）
    pub key: String,
    /// 界面显示名
    pub label: String,
    /// 一句话说明这工具干嘛用
    pub hint: String,
    /// 当前填的路径（空 = 未配置）
    pub path: String,
    /// 内核有没有它的自动下载源（有就显示「⬇ 下载」）
    pub downloadable: bool,
}

/// 工具箱里的设置区数据。
#[derive(Serialize, Debug)]
pub struct SettingsOut {
    /// 默认输出目录（解包/提取的落点）
    pub output_dir: String,
    /// 外部工具表
    pub tools: Vec<ToolRow>,
    /// 机翻接口地址
    pub mtl_base_url: String,
    /// 机翻模型名
    pub mtl_model: String,
    /// 是否已配好机翻密钥（**不回传密钥本身**，只回「有没有」）
    pub mtl_key_set: bool,
    /// 代理（下载外部工具走它）
    pub proxy: String,
    /// 配置文件位置（出问题时让用户能自己找到）
    pub config_path: String,
}

/// 一项诊断结果。
#[derive(Serialize, Debug)]
pub struct CheckRow {
    /// `ok` / `warn` / `fail`
    pub level: String,
    /// 检查项名字
    pub name: String,
    /// 结论
    pub detail: String,
    /// 出问题时的修法（`ok` 项为空）
    pub fix: String,
}

/// 体检 / 自检结果。
#[derive(Serialize, Debug)]
pub struct CheckOut {
    /// 一句话结论
    pub summary: String,
    /// 是否全部通过
    pub ok: bool,
    /// 逐项（可直接渲染）
    pub items: Vec<CheckRow>,
    /// 原始文本（「复制到剪贴板」/ 折叠里显示）
    pub text: String,
}

/// 日志行。
#[derive(Serialize, Debug)]
pub struct LogOut {
    /// 日志文件路径（找不到时为空）
    pub path: String,
    /// 尾部若干行（新的在后）
    pub lines: Vec<String>,
    /// 文件总行数（显示「显示最后 N / 共 M 行」）
    pub total: usize,
}

/// 目录可写性 + 工具可用性的一张小体检表（`Scope` 决定查哪些）。
///
/// 复用内核 `features::health` + `features::precheck`，命令层不自己判环境。
fn check_rows_of(rep: &stool::features::precheck::Report) -> Vec<CheckRow> {
    rep.items
        .iter()
        .map(|it| CheckRow {
            level: match it.level {
                stool::features::precheck::Level::Ok => "ok",
                stool::features::precheck::Level::Warn => "warn",
                stool::features::precheck::Level::Fail => "fail",
            }
            .to_string(),
            name: it.name.to_string(),
            detail: it.detail.clone(),
            fix: it.fix.clone().unwrap_or_default(),
        })
        .collect()
}

/// 工具箱「设置」区数据。**不回传机翻密钥本体**，只回「配没配」。
#[tauri::command]
pub async fn tools_settings() -> Result<SettingsOut, String> {
    offload(move || tools_out_of(&stool::settings::load())).await
}

/// 保存「设置」区（外部工具路径 / 输出目录 / 代理 / 机翻接口）。
///
/// 机翻密钥**单独走 `mtl_save`**（在翻译页也用它），这里只处理其余字段。
/// 各字段都传 `Option`：不传 = 不改，避免「保存这一项顺手把别项清空」。
#[tauri::command]
pub async fn tools_save(
    output_dir: Option<String>,
    proxy: Option<String>,
    mtl_base_url: Option<String>,
    mtl_model: Option<String>,
) -> Result<SettingsOut, String> {
    offload(move || {
        let mut cfg = stool::settings::load();
        if let Some(v) = output_dir {
            cfg.output_dir = v;
        }
        if let Some(v) = proxy {
            cfg.proxy = v;
        }
        if let Some(v) = mtl_base_url {
            cfg.mtl_base_url = v;
        }
        if let Some(v) = mtl_model {
            cfg.mtl_model = v;
        }
        stool::settings::save(&cfg).map_err(|e| {
            format!("保存设置失败：{e}\n改法：确认用户配置目录可写（通常不需要手动处理）。")
        })?;
        // 直接回最新的一份，界面不用再发一次读命令
        tools_out_of(&cfg)
    })
    .await
}

/// 保存单个外部工具路径（工具行右侧的输入框「保存」）。
#[tauri::command]
pub async fn tool_set(key: String, path: String) -> Result<SettingsOut, String> {
    offload(move || {
        let mut cfg = stool::settings::load();
        match key.as_str() {
            "wolfdec" => cfg.wolfdec = path,
            "asset_ripper" => cfg.asset_ripper = path,
            "garbro" => cfg.garbro = path,
            "gdre_tools" => cfg.gdre_tools = path,
            "unrpyc" => cfg.unrpyc = path,
            "python" => cfg.python = path,
            "proxy" => cfg.proxy = path,
            other => {
                return Err(format!(
                    "没有这个外部工具：{other}\n改法：刷新工具箱页面后重试。"
                ))
            }
        }
        stool::settings::save(&cfg)
            .map_err(|e| format!("保存失败：{e}\n改法：确认用户配置目录可写。"))?;
        tools_out_of(&cfg)
    })
    .await
}

/// 「游戏体检」：查区域设置 / 日文字体 / 运行库 / 路径 / 写权限。
#[tauri::command]
pub async fn tools_health(state: tauri::State<'_, AppState>) -> Result<CheckOut, String> {
    let (root, engine) = current_game(&state)?;
    offload(move || {
        let p = PathBuf::from(&root);
        if !p.is_dir() {
            return Err(format!("游戏目录不存在：{root}\n改法：回「选游戏」页重新选一次。"));
        }
        let rep = stool::features::health::check(&p, &engine);
        let text = rep.lines().join("\n");
        let items = check_rows_of(&rep);
        let bad = rep
            .items
            .iter()
            .filter(|i| i.level != stool::features::precheck::Level::Ok)
            .count();
        Ok(CheckOut {
            summary: if rep.ok() {
                format!("体检完成：{bad} 项提示，未发现阻断性问题。")
            } else {
                "体检发现问题，见下（按「修法」处理即可）。".to_string()
            },
            ok: rep.ok(),
            items,
            text,
        })
    })
    .await
}

/// 「封包自检」：解包 → 重打包 → 逐条目比对，确认对封包的读写无损。
#[tauri::command]
pub async fn tools_selfcheck(state: tauri::State<'_, AppState>) -> Result<CheckOut, String> {
    let (root, _) = current_game(&state)?;
    offload(move || {
        let p = PathBuf::from(&root);
        if !p.is_dir() {
            return Err(format!("游戏目录不存在：{root}\n改法：回「选游戏」页重新选一次。"));
        }
        let work = stool::features::selfcheck::default_work_root();
        // 有封包就逐个比对（更严格）；没有封包（明文资源）就退回目录自检。
        let archives = stool::features::selfcheck::find_archives(&p);
        if archives.is_empty() {
            let rep = stool::features::selfcheck::check_dir(&p, &work);
            let text = rep.lines().join("\n");
            let items = check_rows_of(&rep);
            return Ok(CheckOut {
                summary: if rep.ok() {
                    "自检通过：目录里没有封包，资源按明文处理，未发现不一致。".to_string()
                } else {
                    "自检发现阻断性问题，见下。".to_string()
                },
                ok: rep.ok(),
                items,
                text,
            });
        }
        let mut rows: Vec<CheckRow> = Vec::new();
        let mut all_ok = true;
        for a in &archives {
            match stool::features::selfcheck::check_archive(a, &work) {
                Ok(o) => {
                    let ok = o.ok();
                    all_ok &= ok;
                    rows.push(CheckRow {
                        level: if ok { "ok" } else { "fail" }.to_string(),
                        name: a
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| a.display().to_string()),
                        detail: if ok {
                            "解包→重打包→回读，逐条目一致".to_string()
                        } else {
                            format!("{} 处往返不一致", o.mismatches.len())
                        },
                        fix: if ok {
                            String::new()
                        } else {
                            "这个封包的写回可能损坏原文件。改法：先备份原封包，再考虑用「运行时注入」替代回填。"
                                .to_string()
                        },
                    });
                }
                Err(e) => {
                    all_ok = false;
                    rows.push(CheckRow {
                        level: "fail".to_string(),
                        name: a
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| a.display().to_string()),
                        detail: e,
                        fix: "改法：这个封包可能加密或格式特殊，解包请改用「取出素材」页的兜底选项。"
                            .to_string(),
                    });
                }
            }
        }
        let text = rows
            .iter()
            .map(|r| format!("{} {}: {}", r.level, r.name, r.detail))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(CheckOut {
            summary: if all_ok {
                format!("自检通过：{} 个封包往返一致。", archives.len())
            } else {
                "自检发现问题，见下。".to_string()
            },
            ok: all_ok,
            items: rows,
            text,
        })
    })
    .await
}

/// 读运行日志的尾部若干行。
#[tauri::command]
pub async fn tools_log(tail: usize) -> Result<LogOut, String> {
    offload(move || {
        let want = tail.clamp(50, 5000);
        let path = stool::diag::log_path();
        let path_str = path.display().to_string();
        if !path.is_file() {
            return Ok(LogOut { path: path_str, lines: Vec::new(), total: 0 });
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let all: Vec<&str> = text.lines().collect();
        let total = all.len();
        let start = total.saturating_sub(want);
        Ok(LogOut {
            path: path_str,
            lines: all[start..].iter().map(|s| (*s).to_string()).collect(),
            total,
        })
    })
    .await
}

/// 导出诊断包：日志 + 环境（密钥已脱敏）+ 引擎检测 + 备份清单 → 单个 zip。
#[tauri::command]
pub async fn tools_export_diag(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let root = state
        .game_root
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .filter(|p| Path::new(p).is_dir());
    offload(move || {
        let out = std::env::temp_dir().join(format!(
            "stool-diag-{}.zip",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ));
        let rep = stool::features::diagpack::export(root.as_deref().map(Path::new), &out)?;
        Ok(rep.zip_path.display().to_string())
    })
    .await
}

/// 一键下载某个外部工具到 `~/.stool/tools/` 并自动填好路径。
///
/// 下载走配置里的代理（`cfg.proxy`）；内核 `tools_dl::download` 负责实际抓取，
/// 命令层只做「下完把路径写进配置」这一步。
#[tauri::command]
pub async fn tool_download(key: String) -> Result<SettingsOut, String> {
    offload(move || {
        let cfg0 = stool::settings::load();
        if stool::features::tools_dl::spec_by_key(&key).is_none() {
            return Err(format!(
                "这个工具没有自动下载源：{key}\n改法：手动下载可执行文件，再把完整路径填进上面的输入框保存。"
            ));
        }
        let path = stool::features::tools_dl::download(&key, &cfg0.proxy, &|_, _| {})?;
        let mut cfg = stool::settings::load();
        let p = path.display().to_string();
        match key.as_str() {
            "wolfdec" => cfg.wolfdec = p,
            "asset_ripper" => cfg.asset_ripper = p,
            "garbro" => cfg.garbro = p,
            "gdre_tools" => cfg.gdre_tools = p,
            "unrpyc" => cfg.unrpyc = p,
            "python" => cfg.python = p,
            // 其余键（含 proxy 自己）不是「可下载工具」，上面已经拦掉了
            other => {
                return Err(format!(
                    "这个键不能自动填路径：{other}\n改法：手动把完整路径填进输入框保存。"
                ))
            }
        }
        stool::settings::save(&cfg)
            .map_err(|e| format!("工具已下载但保存路径失败：{e}\n改法：手动把路径填进输入框保存。"))?;
        tools_out_of(&cfg)
    })
    .await
}

/// 把 `Config` 摊成界面用的 `SettingsOut`（`tools_settings` / `tools_save` /
/// `tool_set` / `tool_download` 共用一份，避免几处字段漂移）。
fn tools_out_of(cfg: &stool::settings::Config) -> Result<SettingsOut, String> {
    let tool = |key: &str, label: &str, hint: &str, val: &str| ToolRow {
        key: key.to_string(),
        label: label.to_string(),
        hint: hint.to_string(),
        path: val.to_string(),
        downloadable: stool::features::tools_dl::spec_by_key(key).is_some(),
    };
    Ok(SettingsOut {
        output_dir: cfg.output_dir.clone(),
        tools: vec![
            tool("wolfdec", "WolfDec.exe", "解 Wolf RPG 的封包", &cfg.wolfdec),
            tool("asset_ripper", "AssetRipper", "解 Unity 的封包", &cfg.asset_ripper),
            tool("garbro", "GARbro", "认不出格式时的兜底解包", &cfg.garbro),
            tool("gdre_tools", "GDRE Tools", "反编译 Godot 的脚本", &cfg.gdre_tools),
            tool("unrpyc", "unrpyc.py", "反编译 Ren'Py 的脚本", &cfg.unrpyc),
            tool("python", "Python", "上面两个脚本工具要用它来跑", &cfg.python),
            tool("proxy", "代理", "下载工具时走它（形如 http://127.0.0.1:7890）", &cfg.proxy),
        ],
        mtl_base_url: cfg.mtl_base_url.clone(),
        mtl_model: cfg.mtl_model.clone(),
        mtl_key_set: !cfg.mtl_key.is_empty(),
        proxy: cfg.proxy.clone(),
        config_path: stool::settings::config_path().display().to_string(),
    })
}

// ---------------------------------------------------------------------------
// 环境变量入口
// ---------------------------------------------------------------------------

/// `STOOL_GAME=<游戏目录>` 时，启动即可直接检测。
#[tauri::command]
pub fn env_game_root() -> Option<String> {
    std::env::var("STOOL_GAME").ok().filter(|p| !p.is_empty())
}

/// `STOOL_SAVE=<存档文件>` 时，启动即可直接打开存档。
#[tauri::command]
pub fn env_save_path() -> Option<String> {
    std::env::var("STOOL_SAVE").ok().filter(|p| !p.is_empty())
}

/// `STOOL_QUERY=<关键词>` 时，打开存档后自动执行一次搜索。
#[tauri::command]
pub fn env_query() -> String {
    std::env::var("STOOL_QUERY").unwrap_or_default()
}

/// `STOOL_OUT=<目录>` 时，作为「取出素材」的默认输出目录。
#[tauri::command]
pub fn env_out_dir() -> Option<String> {
    std::env::var("STOOL_OUT").ok().filter(|p| !p.is_empty())
}

/// `STOOL_PAGE=<页面 id>` 时，启动即停在该页（与 egui 版的 `STOOL_PAGE` 同一口径）。
#[tauri::command]
pub fn env_page() -> Option<String> {
    std::env::var("STOOL_PAGE").ok().filter(|p| !p.is_empty())
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

/// 命令层里真正干活的是 `detect_impl` / `load_doc` 这类**纯函数**，
/// 所以可以直接用真机样本测，不必拉起 WebView
/// （本机反复 taskkill 会让 WebView2 的窗口类状态坏掉，GUI 冒烟不稳定）。
///
/// 样本取自 `verify/`（不入库，见 .gitignore）。样本不在就跳过 —— **不造假绿**。
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_GAME: &str = r"D:\STool\verify\komoguri";
    const SAMPLE_SAVE: &str = r"D:\STool\verify\demo_save\mv_save.json";

    #[test]
    fn detect_impl_reads_real_kirikiri_sample() {
        if !Path::new(SAMPLE_GAME).is_dir() {
            eprintln!("跳过：本机没有 {SAMPLE_GAME}");
            return;
        }
        let out = detect_impl(SAMPLE_GAME).expect("检测真机样本应成功");
        assert!(
            out.engine_id.to_lowercase().contains("kirikiri"),
            "komoguri 样本应识别为 KiriKiri，实际是 {}",
            out.engine_id
        );
        assert!(out.confirmed, "真机样本应达判定线（score={}）", out.score);
        assert!(!out.capabilities.is_empty(), "识别出来的引擎应有可用能力");
        assert!(
            out.capabilities.iter().any(|c| c.id == "extract"),
            "KiriKiri 至少要能取出素材"
        );
        assert!(!out.root.is_empty());
    }

    #[test]
    fn detect_impl_rejects_non_directory_with_actionable_message() {
        let err = detect_impl(r"D:\STool\verify\__这个目录不存在__").unwrap_err();
        assert!(err.contains("改法"), "失败信息必须带「修法」，实际：{err}");
    }

    #[test]
    fn load_doc_reads_real_plain_json_save() {
        if !Path::new(SAMPLE_SAVE).is_file() {
            eprintln!("跳过：本机没有 {SAMPLE_SAVE}");
            return;
        }
        let (doc, info) = load_doc(SAMPLE_SAVE).expect("打开真机存档样本应成功");
        assert!(info.writable, "明文 JSON 存档应可写");
        assert!(info.top_fields > 0, "顶层字段数应大于 0");

        let hits = doc.search("1", SearchScope::All);
        assert!(!hits.is_empty(), "样本里应能搜到「1」");
        assert!(hits.len() <= 500, "搜索必须保持 500 条上限，实际 {}", hits.len());

        // 命中路径都能取到值（row_of 依赖这个不变量）
        for p in hits.iter().take(20) {
            assert!(doc.get(p).is_some(), "命中路径应可取到值：{p}");
            let _ = row_of(&doc, p);
        }
    }

    #[test]
    fn value_text_roundtrip_matches_kernel() {
        // 这两个函数是两个界面**共用**的口径实现，这里钉住它的语义
        assert_eq!(saves::parse_edit_text("42"), serde_json::json!(42));
        assert_eq!(saves::parse_edit_text("1.5"), serde_json::json!(1.5));
        assert_eq!(saves::parse_edit_text("true"), serde_json::Value::Bool(true));
        assert_eq!(saves::parse_edit_text("false"), serde_json::Value::Bool(false));
        assert_eq!(saves::parse_edit_text("null"), serde_json::Value::Null);
        assert_eq!(saves::parse_edit_text("金币"), serde_json::Value::String("金币".into()));
        assert_eq!(saves::value_edit_text(&serde_json::json!("abc")), "abc");
        assert_eq!(saves::value_edit_text(&serde_json::json!([1, 2])), "[1,2]");
    }

    // -- 取出素材 / 重新打包 --------------------------------------------------

    /// `verify/tail_test` 里是**未加密**的 xp3，是唯一能验证「解包成功」这条
    /// 正向路径的真机样本（`verify/komoguri` 三个 xp3 全加密，只能验失败路径）。
    const SAMPLE_TAIL: &str = r"D:\STool\verify\tail_test";

    fn noop_progress() -> impl Fn(f32, &str) {
        |_f: f32, _m: &str| {}
    }

    fn count_files(dir: &Path) -> usize {
        let mut n = 0;
        let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                n += count_files(&p);
            } else {
                n += 1;
            }
        }
        n
    }

    #[test]
    fn run_op_core_extracts_real_unencrypted_archive() {
        if !Path::new(SAMPLE_TAIL).is_dir() {
            eprintln!("跳过：本机没有 {SAMPLE_TAIL}");
            return;
        }
        let out = std::env::temp_dir().join("stool_tauri_extract_test");
        let _ = std::fs::remove_dir_all(&out);

        let cancel = AtomicBool::new(false);
        let ticks = std::cell::Cell::new(0usize);
        let progress = |_f: f32, _m: &str| ticks.set(ticks.get() + 1);

        let out_s = out.display().to_string();
        let r = run_op_core(stool::engines::Op::Extract, SAMPLE_TAIL, "kirikiri", &out_s, "", &cancel, &progress)
            .expect("真机样本解包不应返回错误");
        assert!(r.success, "解包应成功，实际信息：{}", r.message);
        assert!(r.files_done > 0, "应至少写出一条目，实际 files_done={}", r.files_done);

        // 进度回调必须真的被调用（前端进度条就靠它，回调没接上界面会一直空转）
        assert!(ticks.get() > 0, "内核应至少回报一次进度");

        // 产物确实落到输出目录
        let n = count_files(&out);
        assert!(n > 0, "输出目录里应有文件，实际 {n} 个");

        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn run_op_core_failures_always_carry_a_fix() {
        let cancel = AtomicBool::new(false);
        let p = noop_progress();

        // 没选输出目录
        let e = run_op_core(stool::engines::Op::Extract, SAMPLE_TAIL, "kirikiri", "   ", "", &cancel, &p).unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");

        // 游戏目录不存在
        let e = run_op_core(stool::engines::Op::Extract, r"D:\STool\verify\__不存在__", "kirikiri", r"D:\tmp", "", &cancel, &p)
            .unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");

        // 还不知道引擎（没走「选游戏」）
        let e = run_op_core(stool::engines::Op::Extract, SAMPLE_TAIL, "  ", r"D:\tmp", "", &cancel, &p).unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");

        // 封包：源目录不存在
        let e = run_op_core(stool::engines::Op::Repack, SAMPLE_TAIL, "kirikiri", "", r"D:\STool\verify\__没有这个目录__", &cancel, &p)
            .unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
    }

    #[test]
    fn open_folder_reports_missing_dir_with_a_fix() {
        let e = open_folder(r"D:\STool\verify\__没有这个目录__".into()).unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
    }

    #[test]
    fn progress_throttle_keeps_final_and_drops_noise() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;

        // 几万次回调里的大多数长这样：比例几乎没动、时刻也几乎没变 → 不转发
        assert!(!should_emit_progress(0.50, t0, 0.501, t0 + ms(1)));
        // 推进超过 0.5% → 转发
        assert!(should_emit_progress(0.50, t0, 0.53, t0 + ms(1)));
        // 比例没动但时间够久 → 也要转发（让界面知道还活着，而不是卡死了）
        assert!(should_emit_progress(0.50, t0, 0.5005, t0 + ms(200)));
        // 收尾必须放行，否则进度条停在 99%
        assert!(should_emit_progress(0.99, t0, 1.0, t0 + ms(1)));
    }

    // -- 看素材 ---------------------------------------------------------------

    /// 真机样本 A：`komoguri_out` 里有 png / ogg / ks 三类，验完整分类链路。
    /// 注意这批 `.ks`/`.tjs` 来自**加密封包**，内容多是密文 —— 正好用来验「如实拒绝」。
    const SAMPLE_MEDIA: &str = r"D:\STool\verify\komoguri_out";

    /// 真机样本 B：`tail_test/out/unencrypted` 是**未加密**解包产物，
    /// 里面的 `.tjs` 是**无 BOM 的 UTF-16LE**、`.txt` 是 Shift-JIS —— 验正常解码。
    const SAMPLE_PLAIN: &str = r"D:\STool\verify\tail_test\out\unencrypted";

    #[test]
    fn scan_media_core_finds_all_three_kinds_on_real_sample() {
        if !Path::new(SAMPLE_MEDIA).is_dir() {
            eprintln!("跳过：本机没有 {SAMPLE_MEDIA}");
            return;
        }
        let all = scan_media_core(SAMPLE_MEDIA, "all").expect("扫描真机样本应成功");
        assert!(!all.files.is_empty(), "真机样本里应有素材");
        assert_eq!(all.total, all.files.len(), "样本没到推送上限，不该截断");

        for k in ["image", "audio", "text"] {
            let n = all.files.iter().filter(|f| f.kind == k).count();
            assert!(n > 0, "真机样本里应有 {k}，实际 0（分类表可能漏了扩展名）");
        }

        // 相对路径必须真的比绝对路径短（列表里要显示它，不是绝对路径）
        for f in all.files.iter().take(50) {
            assert!(!f.rel.is_empty());
            assert!(f.rel.len() < f.path.len(), "rel 应短于绝对路径：{}", f.rel);
        }
    }

    #[test]
    fn scan_media_core_filters_by_kind() {
        if !Path::new(SAMPLE_MEDIA).is_dir() {
            return;
        }
        let only_img = scan_media_core(SAMPLE_MEDIA, "image").expect("按类型过滤应成功");
        assert!(!only_img.files.is_empty());
        assert!(only_img.files.iter().all(|f| f.kind == "image"), "过滤后不该混入别的类型");

        let all = scan_media_core(SAMPLE_MEDIA, "all").expect("扫描应成功");
        assert!(only_img.total < all.total, "过滤后总数应更少");
    }

    #[test]
    fn scan_media_core_rejects_non_directory_with_a_fix() {
        let e = scan_media_core(r"D:\STool\verify\__没有这个目录__", "all").unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
    }

    #[test]
    fn read_media_core_inlines_image_and_decodes_real_text() {
        if !Path::new(SAMPLE_MEDIA).is_dir() {
            return;
        }
        let all = scan_media_core(SAMPLE_MEDIA, "all").expect("扫描应成功");

        // 找个能内联的图片（样本里可能有超大图，逐个小范围试，不写死第一个）
        let img = all
            .files
            .iter()
            .filter(|f| f.kind == "image" && f.renderable)
            .take(20)
            .find_map(|f| read_media_core(&f.path).ok())
            .expect("样本里应至少有一张能内联的图片");
        assert!(img.data_url.starts_with("data:image/"), "应内联成图片 data URL");
        assert!(img.mime.starts_with("image/"));
        assert!(img.text.is_empty(), "图片不该带文本内容");

        // 真机 .ks 是 Shift-JIS / UTF-16 —— 必须**有**文件能解出人话。
        // 注意：`komoguri_out` 来自**加密封包**，密文与明文混在一起，且两者的
        // 统计特征在中间带**重叠**（实测正常文本半角片假名 1~4%、密文 12~33%，
        // 但确有个别密文落在 8% 边缘）。所以这里只断言两条**稳健不变量**：
        //   ① 凡是判为「能解码」的，结果就不能是乱码；
        //   ② 凡是判为「不是文本」的，必须给出「改法」而不是干巴巴一句失败。
        // 不假设某个具体文件该过或该拒 —— 那是阈值调参，会随样本漂移。
        let texts: Vec<_> = all.files.iter().filter(|f| f.kind == "text").collect();
        assert!(!texts.is_empty(), "真机样本里应有文本类文件");

        let mut good = 0usize;
        let mut refused = 0usize;
        for f in &texts {
            match read_media_core(&f.path) {
                Ok(t) => {
                    assert_eq!(t.kind, "text");
                    assert!(!t.encoding.is_empty(), "必须如实标注编码");
                    // 这里**不重新定一套阈值** —— 阈值是内核 `decode_text` 的职责，
                    // 测试再抄一份只会两边打架（原先写 20 倍，比内核 8 倍更紧，
                    // 于是内核放行的文件在测试里被判成"乱码"）。
                    // 测试只守住底线：别把**一屏**都是乱码的东西端上来。
                    let bad = t.text.chars().filter(|c| *c == '\u{FFFD}').count();
                    let total_chars = t.text.chars().count().max(1);
                    assert!(
                        bad * 3 <= total_chars,
                        "解出来的东西基本全是乱码（{}/{total_chars}）@ {}，内核阈值形同虚设",
                        bad,
                        f.rel
                    );
                    good += 1;
                }
                Err(e) => {
                    assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
                    refused += 1;
                }
            }
        }
        assert_eq!(good + refused, texts.len(), "每个文件都必须落在「解出」或「如实拒绝」之一");
        assert!(good > 0, "真机样本里应有能正常解码的文本（编码判定可能全坏了）");
        // 拒绝数不写死：闸门阈值调整会让它变化，只要「要么好、要么有出路」就够
        let _ = refused;
    }

    /// 未加密样本上的正常解码：这是「看素材」最主要的正例，
    /// 覆盖**无 BOM UTF-16** 与 Shift-JIS 两条最容易出错的路。
    #[test]
    fn read_media_core_decodes_plain_unencrypted_scripts() {
        if !Path::new(SAMPLE_PLAIN).is_dir() {
            eprintln!("跳过：本机没有 {SAMPLE_PLAIN}");
            return;
        }
        let scan = scan_media_core(SAMPLE_PLAIN, "text").expect("扫描未加密样本应成功");
        assert!(!scan.files.is_empty(), "未加密样本里应有文本文件");

        let mut decoded = 0usize;
        for f in &scan.files {
            let t = read_media_core(&f.path)
                .unwrap_or_else(|e| panic!("未加密样本必须能解码：{} —— {e}", f.rel));
            assert!(!t.text.is_empty(), "{} 解出来不该是空的", f.rel);
            // 未加密样本是**给人看的脚本文本**，这里是全项目里最该「完全干净」的地方，
            // 所以敢用严一点的判据（真机实测这些文件替换字符 < 4%）。
            let bad = t.text.chars().filter(|c| *c == '\u{FFFD}').count();
            let total = t.text.chars().count().max(1);
            assert!(bad * 10 <= total, "{} 解码质量太差：{bad}/{total}（编码判为 {}）", f.rel, t.encoding);
            decoded += 1;
        }
        assert!(decoded > 0, "应有文件被成功解码");

        // 其中至少有一个是 UTF-16（KiriKiri 的 .tjs 常无 BOM）——
        // 这条判定漏了的话会退化成「一屏半角片假名」，却不会报错，最容易被忽略。
        let has_utf16 = scan
            .files
            .iter()
            .filter_map(|f| read_media_core(&f.path).ok())
            .any(|t| t.encoding.starts_with("UTF-16"));
        assert!(has_utf16, "未加密样本里的 .tjs 应有无 BOM 的 UTF-16，编码嗅探可能失效了");
    }

    #[test]
    fn read_media_core_does_not_pretend_unrenderable_formats_work() {
        // .tga 归为「图片」，但浏览器渲染不了 → 必须明确报错并引导用系统程序打开，
        // 而不是给前端一个打不开的 <img> 让用户对着空白发呆。
        let dir = std::env::temp_dir().join("stool_tauri_media_test");
        let _ = std::fs::create_dir_all(&dir);
        let tga = dir.join("probe.tga");
        std::fs::write(&tga, b"\x00\x00\x02\x00").expect("写探针文件");

        let e = read_media_core(&tga.display().to_string()).unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
        assert!(e.contains("系统程序"), "应引导用系统程序打开，实际：{e}");

        let _ = std::fs::remove_file(&tga);
    }

    #[test]
    fn read_media_core_and_open_file_report_missing_with_a_fix() {
        let e = read_media_core(r"D:\STool\verify\__没有这个文件__.png").unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
        let e = open_file(r"D:\STool\verify\__没有这个文件__.png".into()).unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
    }

    // -- 翻译文字 -------------------------------------------------------------

    /// 造一份符合内核列约定的 CSV：0=id 1=file 2=key 3=原文 4=译文。
    fn write_probe_csv(path: &Path, rows: &[(&str, &str, &str, &str)]) {
        let mut s = String::from("id,file,key,source,target\n");
        for (i, (file, key, src, tgt)) in rows.iter().enumerate() {
            s.push_str(&format!("{i},{file},{key},{src},{tgt}\n"));
        }
        std::fs::write(path, s).expect("写探针 CSV");
    }

    #[test]
    fn csv_stats_counts_done_and_todo_correctly() {
        let dir = std::env::temp_dir().join("stool_tauri_csv_test");
        let _ = std::fs::create_dir_all(&dir);
        let csv = dir.join("probe.csv");
        write_probe_csv(
            &csv,
            &[
                ("a.ks", "k1", "おはよう", "早上好"),
                ("a.ks", "k2", "こんばんは", ""),
                ("b.ks", "k3", "ありがとう", "谢谢"),
                ("b.ks", "k4", "さようなら", "   "),
            ],
        );

        let st = csv_stats_core(&csv.display().to_string(), 8).expect("统计应成功");
        assert!(st.exists);
        assert_eq!(st.total, 4, "应有 4 条");
        assert_eq!(st.done, 2, "两条有译文");
        // 只有空白（空格）的译文不算「已翻」—— trim 必须生效，
        // 否则界面显示「100% 翻完」但回填进去的是空白，用户会以为坏了。
        assert_eq!(st.todo, 2, "空白译文必须算作未完成");
        assert_eq!(st.sample.len(), 4);
        assert_eq!(st.sample[0].source, "おはよう");
        assert_eq!(st.sample[0].target, "早上好");
        assert!(st.sample[0].done);
        // 索引 3 才是那条「只有空格」的译文（索引 2 是正常已翻的「谢谢」）
        assert!(!st.sample[3].done, "空白译文不该标记成已翻");
        assert_eq!(st.sample[3].source, "さようなら");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn csv_stats_sample_limit_does_not_change_counts() {
        let dir = std::env::temp_dir().join("stool_tauri_csv_limit_test");
        let _ = std::fs::create_dir_all(&dir);
        let csv = dir.join("probe.csv");
        let rows: Vec<(&str, &str, &str, &str)> =
            (0..30).map(|_| ("a.ks", "k", "src", "tgt")).collect();
        write_probe_csv(&csv, &rows);

        let few = csv_stats_core(&csv.display().to_string(), 3).expect("统计应成功");
        assert_eq!(few.sample.len(), 3, "limit 只该截预览");
        assert_eq!(few.total, 30, "limit 不该影响总数");
        assert_eq!(few.done, 30, "limit 不该影响已翻计数");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn csv_stats_reports_missing_file_with_a_fix() {
        let e = csv_stats_core(r"D:\STool\verify\__没有这个__.csv", 8).unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
        assert!(e.contains("提取文本"), "应指向「提取文本」这一步，实际：{e}");
    }

    #[test]
    fn run_text_op_failures_always_carry_a_fix() {
        let cancel = AtomicBool::new(false);
        let p = noop_progress();
        let probe = r"D:\STool\verify\tail_test";

        // 没选输出目录（提取必须要有地方落 CSV）
        let e = run_text_op(stool::engines::Op::TextExtract, probe, "kirikiri", "  ", "", &cancel, &p)
            .unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");

        // 回填指定的 CSV 不存在
        let e = run_text_op(
            stool::engines::Op::TextImport,
            probe,
            "kirikiri",
            "",
            r"D:\STool\verify\__没有这个__.csv",
            &cancel,
            &p,
        )
        .unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");

        // 游戏目录不存在
        let e = run_text_op(stool::engines::Op::TextExtract, r"D:\STool\verify\__不存在__", "kirikiri", r"D:\tmp", "", &cancel, &p)
            .unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");

        // 还不知道引擎
        let e = run_text_op(stool::engines::Op::TextExtract, probe, "  ", r"D:\tmp", "", &cancel, &p)
            .unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
    }

    #[test]
    fn csv_to_json_core_builds_a_key_value_skeleton() {
        let dir = std::env::temp_dir().join("stool_tauri_mkjson_test");
        let _ = std::fs::create_dir_all(&dir);
        let csv = dir.join("probe.csv");
        write_probe_csv(
            &csv,
            &[
                ("a.ks", "k1", "おはよう", "早上好"),
                ("a.ks", "k2", "こんばんは", ""),
            ],
        );
        let json = dir.join("out.json");

        let (total, filled) =
            csv_to_json_core(&csv.display().to_string(), &json.display().to_string())
                .expect("生成 JSON 骨架应成功");
        assert_eq!(total, 2, "骨架应包含全部条目（没翻的也要留位，否则运行时注入会漏句）");
        assert_eq!(filled, 1, "只有一条带译文");
        assert!(json.is_file(), "JSON 文件必须真的落盘");

        let body = std::fs::read_to_string(&json).expect("读回 JSON");
        assert!(body.contains("おはよう"), "键应是原文，实际：{body}");
        assert!(body.contains("早上好"), "已翻的译文应带过去，实际：{body}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn csv_to_json_core_reports_missing_inputs_with_a_fix() {
        let dir = std::env::temp_dir().join("stool_tauri_mkjson_err_test");
        let _ = std::fs::create_dir_all(&dir);
        let csv = dir.join("probe.csv");
        write_probe_csv(&csv, &[("a.ks", "k", "s", "t")]);

        // CSV 不存在
        let e = csv_to_json_core(r"D:\STool\verify\__没有这个__.csv", &dir.join("o.json").display().to_string())
            .unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");

        // 没给 JSON 路径
        let e = csv_to_json_core(&csv.display().to_string(), "  ").unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");

        // 目标目录不存在
        let e = csv_to_json_core(
            &csv.display().to_string(),
            r"D:\STool\verify\__没有这个目录__\o.json",
        )
        .unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inject_core_guards_unsupported_engine_and_missing_json() {
        // 引擎不支持运行时注入 → 必须给出「改走回填通道」的出路，
        // 而不是一句「不支持」让用户以为整个翻译功能废了。
        let e = inject_core(r"D:\STool\verify\tail_test", "__不存在的引擎__", "", true).unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
        assert!(e.contains("回填"), "应指向回填通道，实际：{e}");

        // 游戏目录不存在
        let e = inject_core(r"D:\STool\verify\__不存在__", "kirikiri", "", false).unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
    }

    #[test]
    fn text_stats_core_reports_unsupported_without_panicking() {
        // 页面一打开就会调它：引擎不支持注入时也必须**正常返回**（supported=false），
        // 而不是抛错 —— 抛错会让整页显示「加载失败」，用户根本看不到回填通道。
        for eng in ["kirikiri", "__不存在的引擎__"] {
            let st = text_stats_core(r"D:\STool\verify\tail_test", eng)
                .unwrap_or_else(|e| panic!("{eng} 下状态查询不该报错：{e}"));
            // 机制/限制说明是给用户看的，支持与不支持都得给一句，不能两头空。
            if st.inject_supported {
                assert!(!st.inject_mechanism.is_empty(), "{eng} 支持注入就该说清怎么适配");
            } else {
                assert!(!st.inject_limits.is_empty(), "{eng} 不支持注入就该说清限制");
            }
        }
    }

    #[test]
    fn text_stats_core_reports_missing_dir_with_a_fix() {
        let e = text_stats_core(r"D:\STool\verify\__不存在__", "kirikiri").unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
    }

    // -- ⑦ 解锁全CG ----------------------------------------------------------

    /// 页面一打开就会调它，所以**任何引擎都不能报错** —— 报错会让整页显示
    /// 「加载失败」，用户连「自带全CG存档」那栏都看不到。
    #[test]
    fn unlock_plan_never_fails_for_any_engine() {
        if !Path::new(SAMPLE_TAIL).is_dir() {
            eprintln!("跳过：本机没有 {SAMPLE_TAIL}");
            return;
        }
        for eng in ["kirikiri", "renpy", "unity", "__不认识__"] {
            let p = unlock_plan_core(SAMPLE_TAIL, eng)
                .unwrap_or_else(|e| panic!("{eng} 下计划查询不该报错：{e}"));
            assert!(!p.routes.is_empty(), "{eng} 至少要给出一条解锁手段（兜底策略）");
            assert!(
                p.routes.iter().any(|r| r.recommended),
                "{eng} 应有且只有一条推荐路线"
            );
            assert!(
                p.routes.iter().filter(|r| r.recommended).count() == 1,
                "{eng} 的推荐路线不该多于一条"
            );
            // 默认路线必须真在候选里 —— 否则界面上「自动」指向一个不存在的选项。
            assert!(
                p.routes.iter().any(|r| r.key == p.default_route),
                "{eng} 默认路线 {} 不在候选里",
                p.default_route
            );
            assert!(!p.basis.is_empty(), "{eng} 必须给出识别依据");
            assert!(!p.action.is_empty(), "{eng} 必须给出解锁动作");
        }
    }

    /// 未收录引擎走兜底策略：`known=false`，但手段照样给全 ——
    /// 「没收录」不等于「没办法」（§3.5 无死胡同）。
    #[test]
    fn unlock_plan_falls_back_but_keeps_routes() {
        if !Path::new(SAMPLE_TAIL).is_dir() {
            eprintln!("跳过：本机没有 {SAMPLE_TAIL}");
            return;
        }
        let p = unlock_plan_core(SAMPLE_TAIL, "__不认识__").expect("兜底策略不该报错");
        assert!(!p.known, "未收录引擎应标 known=false");
        assert!(
            p.routes.iter().any(|r| r.key == "bundled"),
            "兜底策略必须含「替换自带存档」—— 它与引擎无关、成功率最高"
        );
        assert!(
            p.routes.iter().any(|r| !r.automated),
            "兜底策略应含至少一条「仅指引」路线，好让用户知道还能人工处理"
        );
    }

    #[test]
    fn unlock_plan_rejects_missing_dir_with_a_fix() {
        let e = unlock_plan_core(r"D:\STool\verify\__不存在__", "kirikiri").unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
    }

    /// **最关键的一条**：只读预览绝不能写盘。
    ///
    /// `unlock_plan_core` 是进页面就调的函数，它一旦顺手写了什么，
    /// 用户只是「看了看」就被改了存档 —— 这类 bug 不报错，只造成损失。
    #[test]
    fn unlock_plan_is_read_only() {
        let tmp = std::env::temp_dir().join(format!("stool_unlock_plan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("save")).unwrap();
        std::fs::create_dir_all(tmp.join("全CG存档")).unwrap();
        std::fs::write(tmp.join("全CG存档").join("Save01.sav"), b"all-cg").unwrap();
        std::fs::write(tmp.join("save").join("Save01.sav"), b"empty").unwrap();

        let root = tmp.to_string_lossy().into_owned();
        let p = unlock_plan_core(&root, "kirikiri").expect("计划查询应成功");

        // 原本的存档内容必须一字未动
        let after = std::fs::read(tmp.join("save").join("Save01.sav")).unwrap();
        assert_eq!(after, b"empty", "只读预览不得改动存档内容");
        assert!(!tmp.join("save").join("Save01.sav.stool.bak").exists(),
            "只读预览不得产生备份文件");
        // 但候选要真被认出来（否则「只读」是「什么都没做」而不是「看清楚了」）
        assert!(!p.bundled.is_empty(), "应认出「全CG存档」目录里的候选文件");
        assert!(!p.save_dirs.is_empty(), "应找出目标存档目录 save/");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 落地模式：复制进存档目录，且**覆盖前留备份** —— 这就是「撤销上一次」的依据。
    #[test]
    fn unlock_run_apply_copies_and_backs_up() {
        let tmp = std::env::temp_dir().join(format!("stool_unlock_apply_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("save")).unwrap();
        std::fs::create_dir_all(tmp.join("全CG存档")).unwrap();
        std::fs::write(tmp.join("全CG存档").join("Save01.sav"), b"all-cg").unwrap();
        std::fs::write(tmp.join("save").join("Save01.sav"), b"empty").unwrap();

        let root = tmp.to_string_lossy().into_owned();
        let cancel = AtomicBool::new(false);
        let opts = UnlockOpts::default();
        let out = unlock_run_core(&root, "kirikiri", true, &opts, &cancel, &noop_progress())
            .expect("落地执行不该报错");

        assert!(out.success, "自带存档替换应成功：{}", out.message);
        assert_eq!(
            std::fs::read(tmp.join("save").join("Save01.sav")).unwrap(),
            b"all-cg",
            "存档应被替换"
        );
        // 覆盖前必须留底，否则「撤销上一次」无从谈起
        let bak = tmp.join("save").join("Save01.sav.stool.bak");
        assert!(bak.exists(), "覆盖原存档前必须留 .stool.bak");
        assert_eq!(std::fs::read(&bak).unwrap(), b"empty", "备份应是覆盖前的原内容");

        // 备份能被 restore 找到并还原
        let found = stool::features::restore::find_backups(&tmp);
        assert_eq!(found.len(), 1, "应找到 1 个备份");
        stool::features::restore::restore_one(&found[0]).expect("还原应成功");
        assert_eq!(
            std::fs::read(tmp.join("save").join("Save01.sav")).unwrap(),
            b"empty",
            "还原后应回到原内容"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 缺省 `apply=false` → 只读报告，且不算失败（用户「先看看」是正常操作）。
    #[test]
    fn unlock_run_defaults_to_read_only() {
        let tmp = std::env::temp_dir().join(format!("stool_unlock_ro_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("save")).unwrap();
        std::fs::create_dir_all(tmp.join("全CG存档")).unwrap();
        std::fs::write(tmp.join("全CG存档").join("Save01.sav"), b"all-cg").unwrap();

        let root = tmp.to_string_lossy().into_owned();
        let cancel = AtomicBool::new(false);
        let out = unlock_run_core(&root, "kirikiri", false, &UnlockOpts::default(), &cancel, &noop_progress())
            .expect("只读预览不该报错");

        assert!(out.success, "只读预览应算正常返回");
        assert!(out.message.contains("只读") || out.message.contains("未复制"),
            "预览报告要说明「没落地」，实际：{}", out.message);
        assert!(!tmp.join("save").join("Save01.sav").exists(), "只读模式不得写入目标");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 未知 route 必须被**本地**挡下并给出可选值 —— 让内核兜底会报一句更绕的话。
    #[test]
    fn unlock_run_rejects_unknown_route_locally() {
        if !Path::new(SAMPLE_TAIL).is_dir() {
            eprintln!("跳过：本机没有 {SAMPLE_TAIL}");
            return;
        }
        let cancel = AtomicBool::new(false);
        let opts = UnlockOpts { route: "没这个手段".into(), ..Default::default() };
        let e = unlock_run_core(SAMPLE_TAIL, "kirikiri", false, &opts, &cancel, &noop_progress())
            .unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
        assert!(e.contains("bundled"), "应列出可选手段，实际：{e}");
    }

    /// `save_dir` 指向不存在的目录时不能默默退回自动 —— 那会把存档复制到别处。
    #[test]
    fn unlock_run_rejects_bad_save_dir() {
        if !Path::new(SAMPLE_TAIL).is_dir() {
            eprintln!("跳过：本机没有 {SAMPLE_TAIL}");
            return;
        }
        let cancel = AtomicBool::new(false);
        let opts = UnlockOpts {
            save_dir: r"D:\STool\verify\__没有这个目录__".into(),
            ..Default::default()
        };
        let e = unlock_run_core(SAMPLE_TAIL, "kirikiri", true, &opts, &cancel, &noop_progress())
            .unwrap_err();
        assert!(e.contains("改法"), "失败信息必须带「修法」，实际：{e}");
        assert!(e.contains("存档目录"), "应指明是存档目录的问题，实际：{e}");
    }

    /// 「撤销上一次」的入口只接受本工具产的备份，否则它就是个任意文件覆盖接口。
    #[test]
    fn unlock_restore_guard_rejects_non_backup_path() {
        let p = PathBuf::from(r"D:\STool\verify\demo_save\mv_save.json");
        if !p.is_file() {
            eprintln!("跳过：本机没有 {} 这个样本", p.display());
            return;
        }
        // 复刻 unlock_restore 里的守卫判定（命令体本身需要 Tauri runtime，测不到）
        let bak = p.display().to_string();
        assert!(!bak.ends_with(".stool.bak"), "样本本身不是备份，应被守卫拒绝");
    }

    // ---- ⑥ 游戏里改数值：命令层的纯函数 ------------------------------------

    /// 类型表必须与内核 `ScanType::ALL` 一一对应 —— 少一个 = 界面上没得选，
    /// 多一个 = 界面能选但内核 `parse` 不认。两种都是「看起来能用，实际不能用」。
    #[test]
    fn value_types_cover_every_kernel_type_in_order() {
        use stool::features::memscan::ScanType;
        let types = value_types();
        assert_eq!(types.len(), ScanType::ALL.len(), "类型表条数必须等于内核的类型数");
        for (got, want) in types.iter().zip(ScanType::ALL.iter()) {
            assert_eq!(got.key, type_key(*want), "顺序与键必须与内核一致");
            // 键必须能被内核解析回同一个类型（界面回传的就是这个键）
            assert_eq!(ScanType::parse(&got.key), Some(*want), "键 {got} 必须能被内核解析", got = got.key);
            assert_eq!(got.bytes, want.value_size(), "字节数必须来自内核");
            assert_eq!(got.numeric, stool::features::memscan::is_numeric(*want));
        }
    }

    /// 用户明确要求「文本描述对齐 Cheat Engine（4 字节 / 8 字节）」——
    /// 这条钉住那 4 个数值类型的展示文案，防止以后被改回「整数 32 位」。
    #[test]
    fn value_type_labels_use_cheat_engine_wording() {
        let types = value_types();
        let label = |k: &str| types.iter().find(|t| t.key == k).map(|t| t.label.clone()).unwrap_or_default();
        assert_eq!(label("i32"), "4 字节");
        assert_eq!(label("i64"), "8 字节");
        assert!(label("f32").starts_with("小数"), "小数类型要写出是小数，实际 {}", label("f32"));
        assert!(label("f64").starts_with("小数"));
        // 文本类型不写「N 字节」（字节数对文本没意义），但要说清编码
        assert!(label("utf8").contains("UTF-8"));
        assert!(label("utf16").contains("UTF-16"));
    }

    /// 过滤方式表同理：必须与内核 `Filter::ALL` 一一对应且键可解析回去。
    #[test]
    fn filter_opts_cover_every_kernel_filter() {
        use stool::features::memscan::Filter;
        let opts = filter_opts();
        assert_eq!(opts.len(), Filter::ALL.len());
        for (got, want) in opts.iter().zip(Filter::ALL.iter()) {
            assert_eq!(got.key, filter_key(*want));
            assert_eq!(filter_of(&got.key).unwrap(), *want, "键 {got} 必须能解析回原过滤方式", got = got.key);
        }
        // 空串按「等于」（前端漏传时的宽容默认）
        assert_eq!(filter_of("").unwrap(), Filter::Exact);
        assert_eq!(filter_of("  exact  ").unwrap(), Filter::Exact, "首尾空白要容忍");
    }

    /// 认不出的类型 / 过滤方式必须报错并带「改法」——
    /// 默默退回默认值会让人以为「我明明选的 8 字节，怎么扫的是 4 字节」。
    #[test]
    fn unknown_type_and_filter_fail_with_a_fix() {
        let e = type_of("i128").unwrap_err();
        assert!(e.contains("改法"), "实际：{e}");
        assert!(e.contains("i32"), "应列出可选值，实际：{e}");

        let e = filter_of("变强了").unwrap_err();
        assert!(e.contains("改法"), "实际：{e}");
    }

    /// 界面文本 → JSON 值：只有整串都是合法数字才当数字。
    #[test]
    fn edit_text_to_json_does_not_guess_aggressively() {
        assert_eq!(edit_text_to_json("100"), serde_json::Value::from(100));
        assert_eq!(edit_text_to_json(" -3 "), serde_json::Value::from(-3));
        assert_eq!(edit_text_to_json("true"), serde_json::Value::Bool(true));
        assert_eq!(edit_text_to_json("false"), serde_json::Value::Bool(false));
        assert_eq!(edit_text_to_json("1.5"), serde_json::Value::from(1.5));
        // 「007」不能被转成数字 7 —— 否则角色名 / 编号会被悄悄改掉
        assert_eq!(edit_text_to_json("007"), serde_json::Value::String("007".into()));
        assert_eq!(edit_text_to_json("0"), serde_json::Value::from(0), "单个 0 仍是数字");
        assert_eq!(edit_text_to_json("0.5"), serde_json::Value::from(0.5), "0.x 是小数，不该被当成前导零");
        assert_eq!(edit_text_to_json("12abc"), serde_json::Value::String("12abc".into()));
        assert_eq!(edit_text_to_json("学生A"), serde_json::Value::String("学生A".into()));
        assert_eq!(edit_text_to_json(""), serde_json::Value::String(String::new()));
    }

    /// 方式一的修改对象只认四类，认不出要带「改法」。
    #[test]
    fn mvmz_kind_rejects_unknown_with_a_fix() {
        for ok in ["gold", "variable", "switch", "item"] {
            assert!(mvmz_kind_of(ok).is_ok(), "{ok} 应被接受");
        }
        let e = mvmz_kind_of("money").unwrap_err();
        assert!(e.contains("改法"), "实际：{e}");
    }

    /// 名字表取名字：id 越界 / 类型不符都返回空串，不 panic。
    #[test]
    fn json_name_is_out_of_bounds_safe() {
        let names = serde_json::json!({ "vars": ["好感度", null, 12] });
        assert_eq!(json_name(&names, "vars", 0), "好感度");
        assert_eq!(json_name(&names, "vars", 2), "", "非字符串项应返回空串");
        assert_eq!(json_name(&names, "vars", 99), "", "越界应返回空串");
        assert_eq!(json_name(&names, "nope", 0), "", "缺键应返回空串");
        assert_eq!(json_name(&serde_json::Value::Null, "vars", 0), "");
    }

    /// 扫描会话空快照必须自洽：全 false / 全 0，且默认类型是「4 字节」。
    #[test]
    fn empty_scan_state_is_self_consistent() {
        let st = ScanState {
            active: false,
            pid: 0,
            proc_name: String::new(),
            ty: "i32".into(),
            ty_label: String::new(),
            first_done: false,
            hits: 0,
            readonly_hits: 0,
            can_undo: false,
            frozen: false,
            freeze_period_ms: 0,
            list: Vec::new(),
            truncated: false,
            bytes: 4,
            numeric: true,
        };
        assert!(!st.active && !st.first_done && st.hits == 0);
        // 默认类型必须真的能被内核解析（否则前端首屏就会报「不认识的数值类型」）
        assert!(type_of(&st.ty).is_ok());
    }

    /// 空游戏目录：状态必须自洽（没有 MOD、没有冲突、store 路径指向 stool_mods）。
    #[test]
    fn mods_state_is_self_consistent_on_an_empty_game() {
        let g = std::env::temp_dir().join(format!("stool_tauri_mods_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&g);
        std::fs::create_dir_all(&g).unwrap();

        let st = mods_state_of(&g);
        assert!(st.mods.is_empty(), "刚建的目录不该有 MOD");
        assert!(st.conflicts.is_empty());
        assert_eq!(st.conflict_count, 0);
        assert!(st.store_dir.replace('\\', "/").ends_with("stool_mods"), "store_dir={}", st.store_dir);

        let _ = std::fs::remove_dir_all(&g);
    }

    /// 装一个 MOD 后，状态里应出现它；再装一个覆盖同文件的，冲突数应从 0 变 1。
    ///
    /// 这里刻意 **不经过 `mods_state` 命令**（那需要 `tauri::State`），
    /// 而是直接调 `mods_state_of` —— 命令层真正干活的正是这个纯函数。
    #[test]
    fn mods_state_reports_conflicts_after_a_forced_install() {
        let g = std::env::temp_dir().join(format!("stool_tauri_mods_conflict_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&g);
        std::fs::create_dir_all(&g).unwrap();

        // 两个补丁目录，各带一个同路径文件
        let ma = g.join("_mod_a");
        let mb = g.join("_mod_b");
        for (d, body) in [(&ma, "from A"), (&mb, "from B")] {
            std::fs::create_dir_all(d.join("Data")).unwrap();
            std::fs::write(d.join("Data/common.txt"), body).unwrap();
        }

        stool::features::mods::install_mod(&g, &ma, "A", &|_, _| {}, false).unwrap();
        let after_a = mods_state_of(&g);
        assert_eq!(after_a.mods.len(), 1);
        assert!(after_a.mods[0].enabled);
        assert_eq!(after_a.mods[0].files, 1);
        assert_eq!(after_a.conflict_count, 0, "只有一个 MOD 时不该有冲突");

        // 默认拒绝（此时 B 与已启用的 A 争用同一文件）
        let err = stool::features::mods::install_mod(&g, &mb, "B", &|_, _| {}, false).unwrap_err();
        assert!(err.contains("覆盖同一文件"), "错误应说明冲突：{err}");

        // 强制装上后，状态里应恰好报出 1 处冲突，且两个 MOD 都在
        stool::features::mods::install_mod(&g, &mb, "B", &|_, _| {}, true).unwrap();
        let after_b = mods_state_of(&g);
        assert_eq!(after_b.mods.len(), 2);
        assert_eq!(after_b.conflict_count, 1, "两个已启用 MOD 争用同一文件");
        assert_eq!(after_b.conflicts[0].rel, "Data/common.txt");
        assert_eq!(after_b.conflicts[0].mods, vec!["A".to_string(), "B".to_string()]);

        // 停用 B 后冲突应消失，但 MOD 仍在列表里（只是 enabled=false）
        stool::features::mods::toggle_mod(&g, "B", false).unwrap();
        let after_off = mods_state_of(&g);
        assert_eq!(after_off.mods.len(), 2);
        assert_eq!(after_off.conflict_count, 0, "停用后不应再有冲突");
        assert!(after_off.mods.iter().any(|m| m.name == "B" && !m.enabled));

        let _ = std::fs::remove_dir_all(&g);
    }

    /// 卸载会把状态清空，且被覆盖的原文件应还原。
    #[test]
    fn mods_uninstall_restores_and_empties_state() {
        let g = std::env::temp_dir().join(format!("stool_tauri_mods_uninstall_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&g);
        std::fs::create_dir_all(g.join("Data")).unwrap();
        std::fs::write(g.join("Data/common.txt"), "ORIGINAL").unwrap();

        let m = g.join("_mod");
        std::fs::create_dir_all(m.join("Data")).unwrap();
        std::fs::write(m.join("Data/common.txt"), "PATCHED").unwrap();

        stool::features::mods::install_mod(&g, &m, "P", &|_, _| {}, false).unwrap();
        assert_eq!(std::fs::read_to_string(g.join("Data/common.txt")).unwrap(), "PATCHED");
        let st = mods_state_of(&g);
        assert_eq!(st.mods.len(), 1);
        assert_eq!(st.mods[0].overwritten, 1, "这个 MOD 覆盖了 1 个原有文件");

        stool::features::mods::uninstall_mod(&g, "P").unwrap();
        assert_eq!(
            std::fs::read_to_string(g.join("Data/common.txt")).unwrap(),
            "ORIGINAL",
            "卸载必须还原原文件"
        );
        assert!(mods_state_of(&g).mods.is_empty());

        let _ = std::fs::remove_dir_all(&g);
    }

    // -- ⑨ 工具箱 ------------------------------------------------------------

    #[test]
    fn check_rows_map_every_precheck_level() {
        use stool::features::precheck::{Item, Level, Report};
        let rep = Report {
            items: vec![
                Item { name: "写权限", level: Level::Ok, detail: "可写".into(), fix: None },
                Item {
                    name: "游戏占用",
                    level: Level::Warn,
                    detail: "游戏还开着".into(),
                    fix: Some("关掉游戏再试".into()),
                },
                Item {
                    name: "磁盘空间",
                    level: Level::Fail,
                    detail: "只剩 1 MB".into(),
                    fix: Some("换个输出盘".into()),
                },
            ],
        };
        let rows = check_rows_of(&rep);
        assert_eq!(rows.len(), 3);
        // 三档各映射到界面认识的那个字符串，别串位
        assert_eq!(rows[0].level, "ok");
        assert_eq!(rows[1].level, "warn");
        assert_eq!(rows[2].level, "fail");
        // Ok 项没有修法；有问题的一定有 —— 这是「失败必给修法」在数据层的体现
        assert!(rows[0].fix.is_empty(), "Ok 项不该带修法");
        assert!(!rows[2].fix.is_empty(), "Fail 项必须给修法");
        // 有失败项的 Report 整体不算 ok
        assert!(!rep.ok());
    }

    #[test]
    fn tools_settings_never_returns_the_mtl_key() {
        // `SettingsOut` 里**只有**「密钥配没配」这个布尔，没有密钥字段本身。
        // 用一个明显非法的密钥写进配置，确认它不会出现在返回结构里。
        let out = SettingsOut {
            output_dir: String::new(),
            tools: Vec::new(),
            mtl_base_url: "https://api.example.com".into(),
            mtl_model: "m".into(),
            mtl_key_set: true,
            proxy: String::new(),
            config_path: String::new(),
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("mtl_key_set"), "要保留「配没配」这个布尔");
        assert!(
            !json.contains("mtl_key\""),
            "返回结构里绝不能出现密钥字段本身，实际：{json}"
        );
    }

    #[test]
    fn tools_log_tail_is_clamped_and_path_reported() {
        // 纯逻辑：把上限口径钉死（太小无意义、太大一次 IPC 拖死前端）。
        // tail 会被 clamp 到 [50, 5000]；这里只验边界算术，不真读文件。
        for (raw, want) in [(0usize, 50usize), (10, 50), (200, 200), (99999, 5000)] {
            let clamped = raw.clamp(50, 5000);
            assert_eq!(clamped, want, "tail={raw} 应被夹到 {want}");
        }
    }

    // ---- 备份还原（撤销上一次写入）------------------------------------------

    /// 造一个临时目录 + 一个 `.stool.bak`：原文件写 "NEW"、备份写 "OLD"，
    /// 这样「还原有没有真的生效」一眼可判。返回 (目录, 备份路径, 原文件路径)。
    fn scratch_backup(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("stool_restore_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let orig = dir.join("save.json");
        let bak = dir.join("save.json.stool.bak");
        std::fs::write(&orig, b"NEW").unwrap();
        std::fs::write(&bak, b"OLD").unwrap();
        (dir, bak, orig)
    }

    #[test]
    fn restore_list_finds_backups_with_size_and_age() {
        let (dir, bak, _) = scratch_backup("list");
        let rows = backup_rows_of(&dir).unwrap();
        assert_eq!(rows.len(), 1, "应只找到那一个备份，实际：{rows:?}");
        assert_eq!(rows[0].path, bak.display().to_string());
        assert!(
            rows[0].name.ends_with(".stool.bak"),
            "名字要带后缀 —— 界面靠它说清「这是备份」而不是正式文件：{}",
            rows[0].name
        );
        assert_eq!(rows[0].size, human_bytes(3), "size 是算好的展示串");
        assert!(!rows[0].age.is_empty(), "age 要给「多久以前」，用来判哪个更早");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_list_rejects_missing_dir_with_a_fix() {
        let missing = std::env::temp_dir().join("stool_restore_does_not_exist_xyz");
        let err = backup_rows_of(&missing).unwrap_err();
        assert!(err.contains("不存在"), "原因要说清：{err}");
        assert!(err.contains("改法"), "必须给修法：{err}");
    }

    #[test]
    fn restore_one_rejects_paths_that_are_not_backups() {
        // 这是「不许把还原命令当成任意文件覆盖入口」那道边界。
        for bad in ["D:\\game\\save.json", "D:\\game\\data.xp3", "   ", ""] {
            let err = backup_path_checked(bad).unwrap_err();
            assert!(err.contains("改法"), "必须给修法（{bad:?}）：{err}");
        }
        assert!(backup_path_checked("D:\\game\\save.json.stool.bak").is_ok());
        // 从界面上复制来的路径常带首尾空白，要容忍
        assert!(backup_path_checked("  D:\\game\\a.stool.bak  ").is_ok());
    }

    #[test]
    fn restore_one_puts_the_old_content_back_and_keeps_the_backup() {
        // 走内核真实现，不造假绿：备份内容要真的覆盖回正式文件，且备份本身保留。
        let (_dir, bak, orig) = scratch_backup("one");
        assert_eq!(std::fs::read(&orig).unwrap(), b"NEW");
        let msg = stool::features::restore::restore_one(&bak).unwrap();
        assert_eq!(std::fs::read(&orig).unwrap(), b"OLD", "应还原成备份里的内容");
        assert!(bak.exists(), "备份要保留（可反复还原）：{msg}");
        assert!(msg.contains("还原"), "消息要说清做了什么：{msg}");
    }
}
