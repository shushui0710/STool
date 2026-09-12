//! eframe/egui 图形界面：首页检测、解包、文本、存档编辑器、运行时修改、MOD、设置、日志。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub(crate) use eframe::egui;
use egui::{Color32, RichText};

pub(crate) use crate::engines::{self, Confidence, Detection, Op, OpOutcome, Registry};
pub(crate) use crate::features::{memscan, preview, runtime, saves, tools_dl};

mod pages;
mod save_tree;
mod util;

pub(crate) use save_tree::*;
pub(crate) use util::*;

#[derive(PartialEq, Clone, Copy)]
pub(crate) enum Page {
    Home,
    Extract,
    Preview,
    Text,
    Save,
    Runtime,
    Mods,
    Settings,
    Log,
    Help,
}

/// 音频播放输出（持有输出流与播放队列，必须存活才有声音）。
pub(crate) struct AudioOut {
    _stream: rodio::OutputStream,
    sink: rodio::Sink,
}

pub(crate) struct Shared {
    progress: Mutex<(f32, String)>,
    logs: Mutex<Vec<String>>,
    result: Mutex<Option<OpOutcome>>,
    cancel: Arc<AtomicBool>,
}

impl Shared {
    fn new() -> Self {
        Shared {
            progress: Mutex::new((0.0, String::new())),
            logs: Mutex::new(Vec::new()),
            result: Mutex::new(None),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// 内存扫描后台任务状态。
#[derive(Default)]
pub(crate) struct MsStatus {
    busy: bool,
    msg: String,
}

/// 首次写内存前待确认的动作（写内存属修改器行为，必须先让用户明确知道风险）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MsPending {
    WriteAll,
    Freeze,
}

/// 外部工具下载结果：`(工具名, 结果)`。
type DlResult = Arc<Mutex<Option<(String, Result<PathBuf, String>)>>>;
/// 后台引擎检测结果：`(检测列表, 可能的存档目录, MOD 列表, 扫描的根目录, 引擎详情)`。
type DetResult = Arc<
    Mutex<
        Option<(
            Vec<Detection>,
            Vec<(String, PathBuf)>,
            Vec<crate::features::mods::ModEntry>,
            PathBuf,
            Vec<(String, String)>,
        )>,
    >,
>;
/// 后台预览列表结果：`(目录, 文件列表)`。
type PvListResult = Arc<Mutex<Option<(PathBuf, Vec<PathBuf>)>>>;
/// 诊断包导出结果：`Ok((zip 路径, 告警条数))` / `Err(原因)`。
type DiagResult = Arc<Mutex<Option<Result<(PathBuf, usize), String>>>>;

pub(crate) struct StoolApp {
    page: Page,
    registry: Registry,
    game_root: PathBuf,
    game_root_str: String,
    out_dir: PathBuf,
    detections: Vec<Detection>,
    selected: Option<usize>,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    csv_path: PathBuf,
    csv_path_str: String,
    inj_json_str: String,
    inj_status: Option<(bool, usize, String)>, // (已注入, 条目数, JSON 路径)
    save_locations: Vec<(String, PathBuf)>,
    mod_name: String,
    /// 允许与已启用 MOD 覆盖同一文件（默认否，避免互相覆盖）
    mod_force: bool,
    mods: Vec<crate::features::mods::ModEntry>,
    cfg: crate::settings::Config,
    settings_saved_at: Option<std::time::Instant>,
    toast: Option<(String, std::time::Instant)>,
    start: std::time::Instant,

    // ---- 存档编辑器 ----
    save_doc: Option<saves::SaveDoc>,
    save_path_str: String,
    save_query: String,
    save_hits: Vec<String>,
    /// 容器“显示更多”状态：路径 → 已额外展开的行数（把大容器分页，避免一次布局上千行）
    save_more: HashMap<String, usize>,
    /// 调试钩子（STOOL_SAVE_OPEN）：首帧自动展开顶层容器，便于截图/自动化
    save_auto_open: bool,
    save_sel: Option<String>,
    save_sel_buf: String,
    save_dirty: bool,

    // ---- 运行时修改（MV/MZ 调试协议） ----
    rt_exe_str: String,
    rt_port_str: String,
    rt_game: Option<runtime::DebugGame>,
    rt_state: serde_json::Value,
    rt_msg: String,
    rt_mode: usize,
    rt_id_str: String,
    rt_val_str: String,
    rt_connecting: Arc<AtomicBool>,
    rt_result: Arc<Mutex<Option<Result<runtime::DebugGame, String>>>>,
    rt_auto_tried: bool,
    rt_names: serde_json::Value,

    // ---- 封包类引擎运行时补丁包（KiriKiri patchN.xp3）----
    xp_src_str: String,
    xp_name_str: String,
    xp_msg: String,
    xp_items: Vec<(String, u64, Option<usize>, bool, bool)>, // 名/字节/条目数/是否本工具创建/是否加密

    // ---- 通用内存扫描 ----
    ms_procs: Vec<(u32, String)>,
    ms_proc_filter: String,
    ms_pid: u32,
    ms_pid_label: String,
    ms_ty: memscan::ScanType,
    ms_filter: memscan::Filter,
    ms_value: String,
    ms_sh: Arc<Mutex<MsStatus>>,
    ms_scanner: Arc<Mutex<Option<memscan::Scanner>>>,
    ms_frozen: bool,
    ms_freeze_stop: Arc<AtomicBool>,
    /// 用户是否已确认「写内存」风险（未确认时首次写入会先弹确认框）。
    ms_ack: bool,
    /// 待确认的写内存动作。
    ms_pending: Option<MsPending>,
    save_scope: saves::SearchScope,

    // ---- 资源预览 ----
    pv_dir_str: String,
    pv_files: Vec<PathBuf>,
    pv_filter: String,
    pv_sel: Option<PathBuf>,
    pv_tex: Option<egui::TextureHandle>,
    pv_tex_key: String,
    pv_msg: String,
    pv_audio: Option<AudioOut>,
    pv_playing: Option<PathBuf>,
    pv_volume: f32,

    // ---- 外部工具下载 ----
    dl_busy: Arc<AtomicBool>,
    dl_result: DlResult,
    /// 诊断包导出（P1-1）：后台导出，避免大目录检测卡住 UI
    diag_busy: Arc<AtomicBool>,
    diag_result: DiagResult,

    // ---- 机器翻译 ----
    mtl_base_url: String,
    mtl_key: String,
    mtl_model: String,
    mtl_batch: String,
    mtl_jobs: String,
    mtl_glossary: String,

    // ---- 备份还原 / 双档案切换 ----
    bak_list: Vec<PathBuf>,
    bak_scan_done: bool,

    // ---- 后台检测 / 后台加载（拖放与检测不阻塞界面） ----
    det_busy: Arc<AtomicBool>,
    det_result: DetResult,
    /// 本次操作是否可能改变游戏目录的内容（决定操作完成后要不要整树重扫引擎）。
    /// 解包/反编译/文本提取只写输出目录，不需要重扫 —— 大目录下这一步很贵。
    det_dirty: bool,
    save_load_result: Arc<Mutex<Option<Result<saves::SaveDoc, String>>>>,
    pv_list_result: PvListResult,
    /// 启动时是否已尝试自动检测（STOOL_GAME 环境变量指定目录）
    boot_detect_done: bool,

    // ---- 全 CG 解锁（统一抽象：自带存档 / 注册表 / 存档位 …） ----
    /// 是否真正落地（false = 只读预览）
    unlock_apply: bool,
    /// 候选键名过滤（子串），留空表示不过滤（Unity 等注册表路线适用）
    unlock_filter: String,
    /// 指定手段（`None` = 由策略表自动选：有自带存档优先用，否则引擎首选）
    unlock_route: Option<crate::features::unlock::UnlockRoute>,
    /// 选定引擎的详情（`Engine::describe`，在后台检测线程里算好，避免 UI 线程重复扫目录）
    engine_detail: Vec<(String, String)>,
}

impl Default for StoolApp {
    fn default() -> Self {
        let cfg = crate::settings::load();
        let out_dir = if cfg.output_dir.is_empty() {
            std::env::current_dir().unwrap_or_default().join("stool_output")
        } else {
            PathBuf::from(&cfg.output_dir)
        };
        // 调试/截图辅助：STOOL_PAGE=home|extract|text|save|runtime|mods|settings|log|help|preview
        let page = std::env::var("STOOL_PAGE").ok().and_then(|v| match v.as_str() {
            "extract" => Some(Page::Extract),
            "text" => Some(Page::Text),
            "save" => Some(Page::Save),
            "runtime" => Some(Page::Runtime),
            "mods" => Some(Page::Mods),
            "settings" => Some(Page::Settings),
            "log" => Some(Page::Log),
            "help" => Some(Page::Help),
            "preview" => Some(Page::Preview),
            _ => None,
        }).unwrap_or(Page::Home);
        StoolApp {
            page,
            registry: Registry::new(),
            game_root: PathBuf::from("."),
            game_root_str: String::new(),
            out_dir,
            detections: Vec::new(),
            selected: None,
            shared: Arc::new(Shared::new()),
            worker: None,
            csv_path: PathBuf::from("stool_text.csv"),
            csv_path_str: "stool_text.csv".into(),
            inj_json_str: String::new(),
            inj_status: None,
            save_locations: Vec::new(),
            mod_name: String::new(),
            mod_force: false,
            mods: Vec::new(),
            cfg: cfg.clone(),
            settings_saved_at: None,
            toast: None,
            start: std::time::Instant::now(),

            save_doc: None,
            save_path_str: String::new(),
            save_query: String::new(),
            save_hits: Vec::new(),
            save_more: HashMap::new(),
            save_auto_open: std::env::var("STOOL_SAVE_OPEN").is_ok(),
            save_sel: None,
            save_sel_buf: String::new(),
            save_dirty: false,

            rt_exe_str: String::new(),
            rt_port_str: "9222".into(),
            rt_game: None,
            rt_state: serde_json::Value::Null,
            rt_msg: String::new(),
            rt_mode: 0,
            rt_id_str: String::new(),
            rt_val_str: String::new(),
            rt_connecting: Arc::new(AtomicBool::new(false)),
            rt_result: Arc::new(Mutex::new(None)),
            rt_auto_tried: false,
            rt_names: serde_json::Value::Null,
            xp_src_str: String::new(),
            xp_name_str: String::new(),
            xp_msg: String::new(),
            xp_items: Vec::new(),

            ms_procs: Vec::new(),
            ms_proc_filter: String::new(),
            ms_pid: 0,
            ms_pid_label: "（先刷新进程列表）".into(),
            ms_ty: memscan::ScanType::I32,
            ms_filter: memscan::Filter::Exact,
            ms_value: String::new(),
            ms_sh: Arc::new(Mutex::new(MsStatus::default())),
            ms_scanner: Arc::new(Mutex::new(None)),
            ms_frozen: false,
            ms_freeze_stop: Arc::new(AtomicBool::new(false)),
            ms_ack: false,
            ms_pending: None,
            save_scope: saves::SearchScope::All,

            pv_dir_str: String::new(),
            pv_files: Vec::new(),
            pv_filter: String::new(),
            pv_sel: None,
            pv_tex: None,
            pv_tex_key: String::new(),
            pv_msg: String::new(),
            pv_audio: None,
            pv_playing: None,
            pv_volume: 0.8,

            dl_busy: Arc::new(AtomicBool::new(false)),
            dl_result: Arc::new(Mutex::new(None)),
            diag_busy: Arc::new(AtomicBool::new(false)),
            diag_result: Arc::new(Mutex::new(None)),

            mtl_base_url: cfg.mtl_base_url.clone(),
            mtl_key: cfg.mtl_key.clone(),
            mtl_model: cfg.mtl_model.clone(),
            mtl_batch: cfg.mtl_batch.to_string(),
            mtl_jobs: cfg.mtl_jobs.to_string(),
            mtl_glossary: cfg.mtl_glossary.clone(),

            bak_list: Vec::new(),
            bak_scan_done: false,

            det_busy: Arc::new(AtomicBool::new(false)),
            det_result: Arc::new(Mutex::new(None)),
            det_dirty: true,
            save_load_result: Arc::new(Mutex::new(None)),
            pv_list_result: Arc::new(Mutex::new(None)),
            boot_detect_done: false,
            unlock_apply: false,
            unlock_filter: String::new(),
            unlock_route: None,
            engine_detail: Vec::new(),
        }
    }
}

impl StoolApp {
    /// 把输入框里的路径同步到内部字段（不触发检测）。
    fn sync_paths(&mut self) {
        self.game_root = PathBuf::from(self.game_root_str.trim());
        self.csv_path = PathBuf::from(self.csv_path_str.trim());
    }

    /// 引擎检测改为后台执行：立即返回并提示，避免大目录拖放/检测时界面假死。
    fn refresh_detections(&mut self) {
        self.sync_paths();
        if self.game_root.as_os_str().is_empty() {
            return;
        }
        if self.det_busy.load(Ordering::Relaxed) {
            self.toast = Some(("检测正在进行中，请稍候…".into(), std::time::Instant::now()));
            return;
        }
        self.det_busy.store(true, Ordering::Relaxed);
        self.toast = Some(("正在后台检测引擎…".into(), std::time::Instant::now()));
        self.log(format!("▶ 后台检测 {}", self.game_root.display()));
        let root = self.game_root.clone();
        let result = self.det_result.clone();
        std::thread::spawn(move || {
            // 检测器无状态，在线程内新建，避免跨线程共享
            let registry = Registry::new();
            let dets = registry.detect_all(&root);
            let saves = crate::features::saves::find_save_locations(&root);
            let mods = crate::features::mods::list_mods(&root);
            // 选定引擎的详情（Engine::describe）在后台算一次；
            // 只对"选定"这一个引擎调用，避免每个引擎各扫一遍目录。
            let detail = Registry::pick_from(&dets)
                .and_then(|(d, _)| {
                    registry
                        .get(&d.plugin_id)
                        .map(|e| (d.plugin_id.clone(), e.describe(&root)))
                })
                .filter(|(_, t)| !t.is_empty());
            let engine_detail: Vec<(String, String)> = detail.into_iter().collect();
            if let Ok(mut r) = result.lock() {
                *r = Some((dets, saves, mods, root, engine_detail));
            }
        });
    }

    /// 轮询后台检测结果（每帧调用）。
    fn poll_det_result(&mut self) {
        let ready = self.det_result.lock().ok().and_then(|mut r| r.take());
        if let Some((dets, saves, mods, root, engine_detail)) = ready {
            self.det_busy.store(false, Ordering::Relaxed);
            if root == self.game_root {
                // 期间目录没被再次更换才应用结果
                self.detections = dets;
                self.selected = self
                    .detections
                    .iter()
                    .position(|d| d.ok())
                    .or(Some(0));
                self.save_locations = saves;
                self.mods = mods;
                self.engine_detail = engine_detail;
                self.rt_auto_tried = false;
                self.log(format!("✔ 检测完成 {}", root.display()));
                self.toast = Some(("检测完成".into(), std::time::Instant::now()));
            }
        }
    }

    fn log(&self, msg: String) {
        if let Ok(mut logs) = self.shared.logs.lock() {
            let el = self.start.elapsed().as_secs();
            logs.push(format!("[{:02}:{:02}] {}", el / 60, el % 60, msg));
            // 日志封顶：只保留最近 500 条，避免日志页越来越卡
            if logs.len() > 500 {
                let over = logs.len() - 500;
                logs.drain(0..over);
            }
        }
    }

    fn selected_det(&self) -> Option<&Detection> {
        self.selected.and_then(|i| self.detections.get(i))
    }

    fn spawn(
        &mut self,
        name: &str,
        scope: crate::features::precheck::Scope,
        f: impl FnOnce(Arc<Shared>, PathBuf, PathBuf) -> OpOutcome + Send + 'static,
    ) {
        // 默认按「可能改动游戏目录」处理，操作完成后会重扫引擎；
        // 只写输出目录的操作（解包/反编译/文本提取）会在 run_plugin_op 里改成 false，省掉一次整树扫描。
        self.det_dirty = true;
        // 执行前环境预检（P1-2）：目录有效 / 可写 + 磁盘空间 + 文件占用，
        // 失败直接给出「原因 + 修法」，避免任务跑到一半才失败（几 GB 解包尤其致命）。
        let rep = crate::features::precheck::run(&self.game_root, Some(&self.out_dir), scope);
        if rep.worst() != crate::features::precheck::Level::Ok {
            for line in rep.lines() {
                self.log(format!("[预检] {line}"));
            }
        }
        if !rep.ok() {
            self.toast = Some((rep.fail_summary(), std::time::Instant::now()));
            return;
        }
        if let Err(e) = std::fs::create_dir_all(self.out_dir.clone()) {
            self.toast = Some((format!("输出目录不可用: {e}"), std::time::Instant::now()));
            return;
        }
        if self.worker.as_ref().map(|h| !h.is_finished()).unwrap_or(false) {
            self.toast = Some(("已有任务在执行，请等待或取消".into(), std::time::Instant::now()));
            return;
        }
        let root = self.game_root.clone();
        let out = self.out_dir.clone();
        self.shared.cancel.store(false, Ordering::Relaxed);
        if let Ok(mut r) = self.shared.result.lock() {
            *r = None;
        }
        if let Ok(mut p) = self.shared.progress.lock() {
            *p = (0.0, String::new());
        }
        self.log(format!("▶ 开始: {name}"));
        let shared = self.shared.clone();
        let title = name.to_string();
        self.worker = Some(std::thread::spawn(move || {
            // 用 diag::guard 捕获任务内 panic：转为可展示的错误而不是静默崩溃
            let shared_task = shared.clone();
            let res = crate::diag::guard(&title, move || f(shared_task, root, out))
                .unwrap_or_else(OpOutcome::fail);
            if let Ok(mut p) = shared.progress.lock() {
                *p = (1.0, title);
            }
            if let Ok(mut r) = shared.result.lock() {
                *r = Some(res);
            }
        }));
        self.page = Page::Log;
    }

    fn run_plugin_op(&mut self, op: Op, extra_opts: HashMap<String, String>) {
        let Some(d) = self.selected_det().cloned() else { return };
        let plugin_id = d.plugin_id.clone();
        let name = format!("{plugin_id}:{op:?}");
        let scope = crate::features::precheck::scope_for_op(op, &extra_opts);
        // 解包 / 反编译 / 文本提取只往输出目录写，游戏目录一个字节都不会变
        // → 完成后没必要再整树重扫引擎（大目录下这一步很贵）。
        let writes_root = !matches!(op, Op::Extract | Op::Decompile | Op::TextExtract);
        self.spawn(&name, scope, move |shared, root, out| {
            exec_op(op, &plugin_id, shared, &root, &out, &extra_opts, None)
        });
        self.det_dirty = writes_root;
    }

    /// D1 游戏体检（P2-8）：区域设置 / 日文字体 / 运行库 DLL / 路径 / 写权限。
    /// 检查很轻（几次 read_dir + 注册表），同步执行即可。
    fn run_health(&mut self) {
        if !self.game_root.is_dir() {
            self.toast = Some(("请先在首页选择有效的游戏目录".into(), std::time::Instant::now()));
            return;
        }
        let engine = self.selected_det().map(|d| d.plugin_id.clone()).unwrap_or_default();
        let rep = crate::features::health::check(&self.game_root, &engine);
        self.log(format!("🩺 游戏体检：{}", self.game_root.display()));
        for line in rep.lines() {
            self.log(line);
        }
        let hints = rep
            .items
            .iter()
            .filter(|i| i.level != crate::features::precheck::Level::Ok)
            .count();
        self.toast = Some((format!("体检完成：{hints} 项提示（详见日志页）"), std::time::Instant::now()));
        self.page = Page::Log;
    }

    /// P2-9 批量队列：扫描所选目录下的多个游戏并逐个识别（在后台线程跑，避免卡界面）。
    fn run_batch_detect(&mut self) {
        let base = self.game_root.clone();
        if !base.is_dir() {
            self.toast = Some(("请先选择有效的目录".into(), std::time::Instant::now()));
            return;
        }
        let roots = crate::features::batch::discover(&base, 3);
        if roots.is_empty() {
            self.toast = Some((format!("{} 下未发现候选游戏目录", base.display()), std::time::Instant::now()));
            return;
        }
        let n = roots.len();
        self.log(format!("📦 批量检测：{}（发现 {n} 个候选目录）", base.display()));
        self.spawn("批量检测", crate::features::precheck::Scope::read_only(), move |shared, _root, _out| {
            let items = crate::features::batch::detect_batch(&roots, &|i, total, p| {
                if let Ok(mut pr) = shared.progress.lock() {
                    *pr = ((i + 1) as f32 / total.max(1) as f32, p.display().to_string());
                }
            });
            let mut msg = format!("批量检测完成：{}\n", crate::features::batch::summary(&items));
            for it in &items {
                let mark = if it.ok { "✔" } else { "✘" };
                let eng = if it.engine_id.is_empty() { "未识别" } else { it.engine_id.as_str() };
                msg.push_str(&format!("{mark} {} [{eng}] {}\n", it.root.display(), it.confidence));
            }
            let ok = items.iter().filter(|i| i.ok).count();
            crate::engines::OpOutcome::okn(msg, ok)
        });
    }

    /// P2-6 封包自检：解包 → 重打包 → 逐条目比对，确认对该封包的读写无损。
    /// 需要整包解包再回读，放后台线程跑。
    fn run_selfcheck(&mut self) {
        if !self.game_root.is_dir() {
            self.toast = Some(("请先在首页选择有效的游戏目录".into(), std::time::Instant::now()));
            return;
        }
        let root = self.game_root.clone();
        let work = self.out_dir.join("stool_selfcheck");
        self.log(format!("🧩 封包自检：{}", root.display()));
        self.spawn("封包自检", crate::features::precheck::Scope::read_only(), move |_shared, _root, _out| {
            let rep = crate::features::selfcheck::check_dir(&root, &work);
            let mut msg = String::new();
            for line in rep.lines() {
                msg.push_str(&line);
                msg.push('\n');
            }
            if rep.ok() {
                crate::engines::OpOutcome::okn(msg, rep.items.len())
            } else {
                crate::engines::OpOutcome::fail(msg)
            }
        });
    }

    fn poll_worker(&mut self) {
        let finished = self.worker.as_ref().map(|h| h.is_finished()).unwrap_or(false);
        if finished {
            if let Some(handle) = self.worker.take() {
                if handle.join().is_err() {
                    let hint = format!(
                        "任务线程异常退出（详见日志 {}）",
                        crate::diag::log_path().display()
                    );
                    crate::diag::log("ERROR", &hint);
                    self.log(hint);
                }
            }
            if let Ok(r) = self.shared.result.lock() {
                if let Some(res) = r.as_ref() {
                    self.log(format!("{} {}", if res.success { "✔" } else { "✘" }, res.message));
                    if !res.success {
                        // 失败信息同步落盘，便于事后排查（release 版无控制台）
                        crate::diag::log("ERROR", &format!("任务失败: {}", res.message));
                    }
                    self.toast = Some((
                        format!("{} {}", if res.success { "完成" } else { "失败" }, res.message),
                        std::time::Instant::now(),
                    ));
                }
            }
            // 只有可能改动游戏目录的操作才重扫引擎；解包/反编译/文本提取跳过（省一次整树扫描）
            if self.det_dirty {
                self.refresh_detections();
            } else {
                self.sync_paths();
            }
            self.det_dirty = true;
        }
    }
}

/// 在任务线程内执行插件操作（Ctx 借用的局部变量存活到返回为止）。
pub(crate) fn exec_op(
    op: Op,
    plugin_id: &str,
    shared: Arc<Shared>,
    root: &PathBuf,
    out: &PathBuf,
    opts: &HashMap<String, String>,
    csv: Option<&PathBuf>,
) -> OpOutcome {
    let registry = Registry::new();
    let engine = match registry.get(plugin_id) {
        Some(e) => e,
        None => return OpOutcome::fail("插件不存在"),
    };
    let cancel = shared.cancel.clone();
    let progress = |frac: f32, msg: &str| {
        if let Ok(mut p) = shared.progress.lock() {
            *p = (frac.clamp(0.0, 1.0), msg.to_string());
        }
    };
    let ctx = engines::Ctx { root, out_dir: out, options: opts, progress: &progress, cancel: &cancel };
    // 解包并行化（P2-2）：每次操作前清空「已建目录」缓存。
    engines::clear_dir_cache();
    match op {
        Op::Extract => engine.extract(&ctx),
        Op::Repack => engine.repack(&ctx, &root.join("stool_repack_src")),
        Op::Decompile => engine.decompile(&ctx),
        Op::Save => engine.save(&ctx),
        Op::Unlock => engine.unlock(&ctx),
        Op::TextExtract => engine.text_extract(&ctx, csv.unwrap_or(&out.join("text.csv"))),
        Op::TextImport => engine.text_import(&ctx, csv.unwrap_or(&out.join("text.csv"))),
        Op::TextInject => engine.text_inject(&ctx, csv.unwrap_or(&root.join("translation.json"))),
    }
}

impl eframe::App for StoolApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // STOOL_GAME=<目录>：启动即自动检测（调试/截图/自动化用）
        if !self.boot_detect_done {
            self.boot_detect_done = true;
            if let Ok(g) = std::env::var("STOOL_GAME") {
                if !g.trim().is_empty() && self.game_root_str.trim().is_empty() {
                    self.game_root_str = g.trim().to_string();
                    self.refresh_detections();
                }
            }
            // STOOL_SAVE=<文件>（+可选 STOOL_SAVE_SEL=<JSON Pointer>）：启动即加载存档并预选字段（调试/截图用）
            if let Ok(s) = std::env::var("STOOL_SAVE") {
                let s = s.trim().to_string();
                if !s.is_empty() {
                    self.save_path_str = s.clone();
                    self.page = Page::Save;
                    let result = self.save_load_result.clone();
                    std::thread::spawn(move || {
                        let res = saves::SaveDoc::load(std::path::Path::new(&s)).map_err(|e| e.to_string());
                        if let Ok(mut r) = result.lock() {
                            *r = Some(res);
                        }
                    });
                }
            }
        }

        self.poll_worker();

        // 拖放：立即给出加载反馈，重活全部放后台线程，避免界面假死
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped.iter() {
            if let Some(p) = &f.path {
                if self.page == Page::Preview {
                    if p.is_dir() {
                        self.pv_dir_str = p.display().to_string();
                        self.pv_sel = None;
                        self.toast = Some(("已收到目录，正在后台扫描媒体文件…".into(), std::time::Instant::now()));
                        let dir = p.clone();
                        let result = self.pv_list_result.clone();
                        std::thread::spawn(move || {
                            let files = preview::list_media(&dir);
                            if let Ok(mut r) = result.lock() {
                                *r = Some((dir, files));
                            }
                        });
                    } else if p.is_file() {
                        self.pv_sel = Some(p.clone());
                        if let Some(parent) = p.parent() {
                            self.pv_dir_str = parent.display().to_string();
                        }
                        if preview::kind_of(p) == preview::MediaKind::Image {
                            self.load_pv_texture(p, ctx);
                        }
                    }
                    continue;
                }
                if p.is_dir() {
                    self.game_root_str = p.display().to_string();
                    self.page = Page::Home;
                    self.toast = Some(("已收到游戏目录，正在后台检测…".into(), std::time::Instant::now()));
                    self.refresh_detections();
                } else if p.is_file() {
                    self.save_path_str = p.display().to_string();
                    self.page = Page::Save;
                    self.toast = Some(("已收到文件，正在后台加载存档…".into(), std::time::Instant::now()));
                    let sp = p.clone();
                    let result = self.save_load_result.clone();
                    std::thread::spawn(move || {
                        let res = saves::SaveDoc::load(&sp).map_err(|e| e.to_string());
                        if let Ok(mut r) = result.lock() {
                            *r = Some(res);
                        }
                    });
                }
            }
        }

        // 后台检测结果 / 存档加载结果 / 预览目录扫描结果
        self.poll_det_result();
        if let Ok(mut r) = self.save_load_result.lock() {
            if let Some(res) = r.take() {
                match res {
                    Ok(d) => {
                        self.save_doc = Some(d);
                        self.save_hits.clear();
                        self.save_sel = None;
                        self.save_dirty = false;
                        self.save_more.clear();
                        // 调试钩子：预选一个字段，便于截图/自动化验证右侧编辑面板
                        if let Ok(sel) = std::env::var("STOOL_SAVE_SEL") {
                            let sel = sel.trim().to_string();
                            if !sel.is_empty() {
                                if let Some(doc) = self.save_doc.as_ref() {
                                    self.save_sel_buf = doc.get(&sel).map(value_edit_text).unwrap_or_default();
                                }
                                self.save_sel = Some(sel);
                            }
                        }
                        self.toast = Some(("存档加载完成".into(), std::time::Instant::now()));
                    }
                    Err(e) => self.toast = Some((format!("存档加载失败: {e}"), std::time::Instant::now())),
                }
            }
        }
        if let Ok(mut r) = self.pv_list_result.lock() {
            if let Some((dir, files)) = r.take() {
                if *self.pv_dir_str.trim() == dir {
                    let n = files.len();
                    self.pv_files = files;
                    if n >= preview::MAX_MEDIA {
                        self.toast = Some((
                            format!("媒体文件很多，仅列出前 {n} 个（可缩小目录或用筛选）"),
                            std::time::Instant::now(),
                        ));
                    }
                }
            }
        }
        if self.det_busy.load(Ordering::Relaxed) {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }

        // 内存扫描后台任务完成提醒 + 保持重绘
        if let Ok(st) = self.ms_sh.lock() {
            if st.busy {
                ctx.request_repaint_after(std::time::Duration::from_millis(120));
            }
        }
        // 外部工具下载结果
        if self.dl_busy.load(Ordering::Relaxed) {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        // 诊断包导出结果（P1-1）
        if self.diag_busy.load(Ordering::Relaxed) {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        if let Ok(mut r) = self.diag_result.lock() {
            if let Some(res) = r.take() {
                self.diag_busy.store(false, Ordering::Relaxed);
                match res {
                    Ok((path, warns)) => {
                        self.log(format!("✔ 诊断包已导出 → {}（{warns} 条告警）", path.display()));
                        self.toast = Some((
                            format!("诊断包已导出：{}", path.display()),
                            std::time::Instant::now(),
                        ));
                    }
                    Err(e) => {
                        self.log(format!("✘ 诊断包导出失败: {e}"));
                        self.toast = Some((format!("诊断包导出失败: {e}"), std::time::Instant::now()));
                    }
                }
            }
        }
        if let Ok(mut r) = self.dl_result.lock() {
            if let Some((key, res)) = r.take() {
                self.dl_busy.store(false, Ordering::Relaxed);
                match res {
                    Ok(path) => {
                        let mut cfg = crate::settings::load();
                        match key.as_str() {
                            "wolfdec" => cfg.wolfdec = path.display().to_string(),
                            "asset_ripper" => cfg.asset_ripper = path.display().to_string(),
                            "garbro" => cfg.garbro = path.display().to_string(),
                            "gdre_tools" => cfg.gdre_tools = path.display().to_string(),
                            "unrpyc" => cfg.unrpyc = path.display().to_string(),
                            _ => {}
                        }
                        let _ = crate::settings::save(&cfg);
                        self.cfg = cfg;
                        self.toast = Some((format!("✅ {} 已下载并配置完成", key), std::time::Instant::now()));
                    }
                    Err(e) => self.toast = Some((format!("✘ 下载失败: {e}"), std::time::Instant::now())),
                }
            }
        }
        // 调试连接后台任务结果接收
        if let Ok(mut r) = self.rt_result.lock() {
            if let Some(res) = r.take() {
                match res {
                    Ok(mut g) => {
                        let ready = g.is_rpgm_ready().unwrap_or(false);
                        if ready {
                            self.rt_state = g.read_state().unwrap_or(serde_json::Value::Null);
                            self.rt_names = g.read_names().unwrap_or(serde_json::Value::Null);
                            self.rt_msg = "已连接游戏，可以修改了。".into();
                        } else {
                            self.rt_msg = "已连接，但游戏还没加载 RPG Maker 核心（先进入游戏/读个档再刷新）。".into();
                            self.rt_state = serde_json::Value::Null;
                        }
                        self.rt_game = Some(g);
                    }
                    Err(e) => {
                        self.rt_msg = e.clone();
                        self.toast = Some((e, std::time::Instant::now()));
                    }
                }
            }
        }
        if self.rt_connecting.load(Ordering::Relaxed) {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        let (frac, msg) = self
            .shared
            .progress
            .lock()
            .map(|p| p.clone())
            .unwrap_or((0.0, String::new()));
        let running = self.worker.as_ref().map(|h| !h.is_finished()).unwrap_or(false);

        // 底部状态栏
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                let ok = self.selected_det().map(|d| d.ok()).unwrap_or(false);
                let engine = self
                    .selected_det()
                    .map(|d| format!("{} ({})", d.name, d.plugin_id))
                    .unwrap_or_else(|| "未检测游戏".into());
                ui.label(
                    RichText::new(if ok { "●" } else { "○" })
                        .small()
                        .color(if ok { Color32::from_rgb(120, 200, 120) } else { ui.visuals().weak_text_color() }),
                );
                ui.label(RichText::new(format!("引擎: {engine}")).small());
                ui.separator();
                ui.label(RichText::new(format!("输出: {}", short_text(&self.out_dir.display().to_string(), 46))).small());
                if self.save_dirty {
                    ui.label(RichText::new("存档未保存").small().color(Color32::from_rgb(230, 170, 60)));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("📂 输出目录").clicked() {
                        open_in_explorer(&self.out_dir);
                    }
                    if ui.small_button("📂 游戏目录").clicked() {
                        open_in_explorer(&self.game_root);
                    }
                });
            });
            ui.add_space(2.0);
        });

        egui::SidePanel::left("nav").exact_width(136.0).show(ctx, |ui| {
            ui.add_space(10.0);
            ui.heading(RichText::new("STool").strong());
            ui.label(RichText::new(format!("v{}", crate::VERSION)).small().weak());
            ui.add_space(10.0);
            for (p, label) in [
                (Page::Home, "🏠 引擎检测"),
                (Page::Extract, "📦 资源解包"),
                (Page::Preview, "🖼 资源预览"),
                (Page::Text, "💬 文本/汉化"),
                (Page::Save, "💾 存档编辑"),
                (Page::Runtime, "🎯 运行时修改"),
                (Page::Mods, "🧩 补丁/MOD"),
                (Page::Settings, "⚙ 设置"),
                (Page::Log, "📜 运行日志"),
                (Page::Help, "❓ 使用指南"),
            ] {
                if ui.selectable_label(self.page == p, label).clicked() {
                    self.page = p;
                }
            }
            ui.add_space(12.0);
            if running {
                ui.separator();
                ui.add(egui::ProgressBar::new(frac).show_percentage());
                ui.label(RichText::new(&msg).small().weak());
                if ui.small_button("取消").clicked() {
                    self.shared.cancel.store(true, Ordering::Relaxed);
                }
            }
        });

        // 进入"运行时修改"页时，MV/MZ 游戏自动启动+连接（用户无需手动操作）
        if self.page == Page::Runtime && !self.rt_auto_tried {
            self.rt_auto_tried = true;
            self.rt_auto_connect();
            self.xp_refresh();
        }

        egui::CentralPanel::default().show(ctx, |ui| match self.page {
            Page::Home => self.page_home(ui),
            Page::Extract => self.page_extract(ui),
            Page::Preview => self.page_preview(ui),
            Page::Text => self.page_text(ui),
            Page::Save => self.page_save(ui),
            Page::Runtime => self.page_runtime(ui),
            Page::Mods => self.page_mods(ui),
            Page::Settings => self.page_settings(ui),
            Page::Log => self.page_log(ui),
            Page::Help => self.page_help(ui),
        });

        if let Some((msg, t)) = &self.toast {
            if t.elapsed() < std::time::Duration::from_secs(4) {
                egui::Area::new(egui::Id::new("toast"))
                    .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -20.0])
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(RichText::new(msg).strong());
                        });
                    });
            } else {
                self.toast = None;
            }
        }
    }
}


// ---------------------------------------------------------------------------
// 启动
// ---------------------------------------------------------------------------

pub fn run() -> i32 {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 740.0])
            .with_title(format!("STool v{} — 多引擎游戏综合工具", crate::VERSION)),
        ..Default::default()
    };
    let result = eframe::run_native(
        "STool",
        options,
        Box::new(|cc| {
            install_cjk_font(cc);
            configure_style(&cc.egui_ctx);
            Ok(Box::new(StoolApp::default()))
        }),
    );
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("GUI 启动失败: {e:?}");
            1
        }
    }
}

/// 全局视觉风格：圆角、间距、选中色，让界面更现代、信息更聚焦。
fn configure_style(ctx: &egui::Context) {
    ctx.style_mut(|style| {
        let v = &mut style.visuals;
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.rounding = egui::Rounding::same(5.0);
        }
        v.window_rounding = egui::Rounding::same(8.0);
        v.menu_rounding = egui::Rounding::same(6.0);
        v.extreme_bg_color = if v.dark_mode { Color32::from_gray(28) } else { Color32::from_gray(243) };
        v.selection.bg_fill = Color32::from_rgb(58, 102, 178);
        v.selection.stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(125, 175, 245));
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
        style.spacing.menu_margin = egui::Margin::same(6.0);
    });
}

/// 加载 Windows 系统中文字体（微软雅黑），避免 egui 默认字体显示豆腐块。
fn install_cjk_font(cc: &eframe::CreationContext<'_>) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates = [
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyh.ttf",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simsun.ttc",
    ];
    for path in candidates {
        if let Ok(data) = std::fs::read(path) {
            fonts.font_data.insert("cjk".into(), egui::FontData::from_owned(data));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "cjk".into());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("cjk".into());
            break;
        }
    }
    // Segoe UI Emoji：界面图标的字形来源（作为回退，中文仍走雅黑）
    if let Ok(data) = std::fs::read("C:////Windows////Fonts////seguiemj.ttf") {
        fonts.font_data.insert("emoji".into(), egui::FontData::from_owned(data));
        fonts.families.entry(egui::FontFamily::Proportional).or_default().push("emoji".into());
        fonts.families.entry(egui::FontFamily::Monospace).or_default().push("emoji".into());
    }
    cc.egui_ctx.set_fonts(fonts);
}
