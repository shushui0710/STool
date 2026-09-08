//! eframe/egui 图形界面：首页检测、解包、文本、存档编辑器、运行时修改、MOD、设置、日志。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use eframe::egui;
use egui::{Color32, RichText};

use crate::engines::{self, Detection, Op, OpOutcome, Registry};
use crate::features::{memscan, preview, runtime, saves, tools_dl};

#[derive(PartialEq, Clone, Copy)]
enum Page {
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
struct AudioOut {
    _stream: rodio::OutputStream,
    sink: rodio::Sink,
}

struct Shared {
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
struct MsStatus {
    busy: bool,
    msg: String,
}

struct StoolApp {
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
    save_locations: Vec<(String, PathBuf)>,
    mod_name: String,
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
    save_edit_bufs: HashMap<String, String>,
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
    dl_result: Arc<Mutex<Option<(String, Result<PathBuf, String>)>>>,
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
            save_locations: Vec::new(),
            mod_name: String::new(),
            mods: Vec::new(),
            cfg,
            settings_saved_at: None,
            toast: None,
            start: std::time::Instant::now(),

            save_doc: None,
            save_path_str: String::new(),
            save_query: String::new(),
            save_hits: Vec::new(),
            save_edit_bufs: HashMap::new(),
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
        }
    }
}

impl StoolApp {
    fn refresh_detections(&mut self) {
        self.game_root = PathBuf::from(self.game_root_str.trim());
        self.csv_path = PathBuf::from(self.csv_path_str.trim());
        self.detections = self.registry.detect_all(&self.game_root);
        self.selected = self.detections.iter().position(|d| d.ok()).or(Some(0));
        self.save_locations = crate::features::saves::find_save_locations(&self.game_root);
        self.mods = crate::features::mods::list_mods(&self.game_root);
        self.rt_auto_tried = false;
        self.log(format!("已检测 {}", self.game_root.display()));
    }

    fn log(&self, msg: String) {
        if let Ok(mut logs) = self.shared.logs.lock() {
            let el = self.start.elapsed().as_secs();
            logs.push(format!("[{:02}:{:02}] {}", el / 60, el % 60, msg));
        }
    }

    fn selected_det(&self) -> Option<&Detection> {
        self.selected.and_then(|i| self.detections.get(i))
    }

    fn spawn(&mut self, name: &str, f: impl FnOnce(Arc<Shared>, PathBuf, PathBuf) -> OpOutcome + Send + 'static) {
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
            let res = f(shared.clone(), root, out);
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
        let Some(d) = self.selected_det().map(|d| d.clone()) else { return };
        let plugin_id = d.plugin_id.clone();
        let name = format!("{plugin_id}:{op:?}");
        self.spawn(&name, move |shared, root, out| {
            exec_op(op, &plugin_id, shared, &root, &out, &extra_opts, None)
        });
    }

    fn poll_worker(&mut self) {
        let finished = self.worker.as_ref().map(|h| h.is_finished()).unwrap_or(false);
        if finished {
            if let Some(handle) = self.worker.take() {
                if handle.join().is_err() {
                    self.log("任务线程异常退出".into());
                }
            }
            if let Ok(r) = self.shared.result.lock() {
                if let Some(res) = r.as_ref() {
                    self.log(format!("{} {}", if res.success { "✔" } else { "✘" }, res.message));
                    self.toast = Some((
                        format!("{} {}", if res.success { "完成" } else { "失败" }, res.message),
                        std::time::Instant::now(),
                    ));
                }
            }
            self.refresh_detections();
        }
    }
}

/// 在任务线程内执行插件操作（Ctx 借用的局部变量存活到返回为止）。
fn exec_op(
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
    match op {
        Op::Extract => engine.extract(&ctx),
        Op::Repack => engine.repack(&ctx, &root.join("stool_repack_src")),
        Op::Decompile => engine.decompile(&ctx),
        Op::Save => engine.save(&ctx),
        Op::Unlock => engine.unlock(&ctx),
        Op::TextExtract => engine.text_extract(&ctx, csv.unwrap_or(&out.join("text.csv"))),
        Op::TextImport => engine.text_import(&ctx, csv.unwrap_or(&out.join("text.csv"))),
    }
}

impl eframe::App for StoolApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();

        // 拖放：文件夹 → 首页检测；文件 → 存档编辑器加载
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped.iter() {
            if let Some(p) = &f.path {
                if self.page == Page::Preview {
                    if p.is_dir() {
                        self.pv_dir_str = p.display().to_string();
                        self.pv_files = preview::list_media(p);
                        self.pv_sel = None;
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
                    self.refresh_detections();
                    self.page = Page::Home;
                    self.toast = Some((format!("已检测拖入的游戏目录"), std::time::Instant::now()));
                } else if p.is_file() {
                    self.save_path_str = p.display().to_string();
                    match saves::SaveDoc::load(p) {
                        Ok(d) => {
                            self.save_doc = Some(d);
                            self.save_hits.clear();
                            self.save_sel = None;
                            self.save_dirty = false;
                            self.save_edit_bufs.clear();
                            self.page = Page::Save;
                            self.toast = Some(("存档已加载（拖放）".into(), std::time::Instant::now()));
                        }
                        Err(e) => self.toast = Some((format!("加载失败: {e}"), std::time::Instant::now())),
                    }
                }
            }
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

impl StoolApp {
    fn page_home(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "游戏目录检测", "识别这个游戏是用什么引擎做的，并列出判定证据");
        ui.horizontal(|ui| {
            ui.label("目录:");
            let w = ui.available_width() - 170.0;
            ui.add_sized([w.max(80.0), 22.0], egui::TextEdit::singleline(&mut self.game_root_str));
            if ui.button("浏览...").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.game_root_str = p.display().to_string();
                    self.refresh_detections();
                }
            }
            if ui.button("检测").clicked() {
                self.refresh_detections();
            }
        });
        ui.add_space(10.0);
        if self.detections.is_empty() {
            ui.add_space(ui.available_height() * 0.16);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("🎮").heading());
                ui.add_space(8.0);
                ui.label(RichText::new("把游戏文件夹直接拖进本窗口，或按下面三步开始").heading());
                ui.add_space(12.0);
                ui.label(RichText::new("①  在上方输入（或点“浏览...”选择）游戏根目录").small());
                ui.label(RichText::new("②  点“检测”，STool 会识别引擎并列出判定依据").small());
                ui.label(RichText::new("③  到左侧对应页面：解包资源 · 预览 · 汉化文本 · 改存档 · 运行时修改").small());
            });
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let visuals = ui.visuals().clone();
            for (i, d) in self.detections.iter().enumerate() {
                let sel = self.selected == Some(i);
                let fill = if sel {
                    visuals.selection.bg_fill
                } else if d.ok() {
                    visuals.faint_bg_color
                } else {
                    visuals.extreme_bg_color
                };
                let stroke = if sel { visuals.selection.stroke } else { visuals.widgets.noninteractive.bg_stroke };
                let resp = egui::Frame::group(ui.style())
                    .fill(fill)
                    .stroke(stroke)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{} ({})", d.name, d.plugin_id)).strong());
                            ui.label(
                                RichText::new(if d.ok() { "● 可用" } else { "○ 证据不足" })
                                    .small()
                                    .color(if d.ok() { Color32::from_rgb(120, 200, 120) } else { visuals.weak_text_color() }),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(format!("置信度 {} 分", d.score)).weak().small());
                            });
                        });
                        ui.label(RichText::new(d.evidence.join("；")).small());
                        if !d.notes.is_empty() {
                            ui.label(RichText::new(&d.notes).small().weak());
                        }
                    })
                    .response;
                if resp.clicked() {
                    self.selected = Some(i);
                }
                ui.add_space(4.0);
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.small_button("📂 打开游戏目录").clicked() {
                open_in_explorer(&self.game_root);
            }
            if let Some((_, dir)) = self.save_locations.first() {
                if ui.small_button("📂 打开存档目录").clicked() {
                    open_in_explorer(dir);
                }
            }
        });
    }

    fn page_extract(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "资源解包 / 封包", "");
        let Some(d) = self.selected_det().cloned() else {
            ui.label("请先在首页完成检测。");
            return;
        };
        ui.label(format!("当前引擎: {} ({})", d.name, d.plugin_id));
        ui.label(format!("输出目录: {}", self.out_dir.display()));
        ui.add_space(8.0);
        let caps: Vec<Op> = self
            .registry
            .get(&d.plugin_id)
            .map(|e| e.capabilities())
            .unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            for cap in caps {
                let enabled = matches!(cap, Op::Extract | Op::Decompile | Op::Save | Op::Unlock);
                let tip = if enabled { "" } else { "（请用 CLI 执行）" };
                let btn = egui::Button::new(format!("{}{}", cap.label(), tip));
                if ui.add_enabled(enabled || matches!(cap, Op::Repack), btn).clicked() {
                    self.run_plugin_op(cap, HashMap::new());
                }
            }
        });
        ui.add_space(12.0);
        ui.label(RichText::new("提示：解包结果输出到输出目录，不会改动游戏文件；封包/回写前自动备份原文件（.stool.bak）。").weak());
    }

    /// 后台线程下载外部工具并自动写入配置。
    fn start_tool_download(&mut self, key: &str) {
        if self.dl_busy.load(Ordering::Relaxed) {
            self.toast = Some(("已有下载在进行中…".into(), std::time::Instant::now()));
            return;
        }
        self.dl_busy.store(true, Ordering::Relaxed);
        let proxy = self.cfg.proxy.clone();
        let key = key.to_string();
        let result = self.dl_result.clone();
        self.log(format!("▶ 开始下载外部工具: {key}"));
        std::thread::spawn(move || {
            let res = tools_dl::download(&key, &proxy, &|frac, msg| {
                let _ = frac; // 大里程碑由日志体现，避免刷屏
                if frac >= 0.99 {
                    let _ = msg;
                }
            });
            if let Ok(mut r) = result.lock() {
                *r = Some((key, res));
            }
        });
    }

    // -----------------------------------------------------------------
    // 资源预览
    // -----------------------------------------------------------------

    fn refresh_pv_files(&mut self) {
        let dir = PathBuf::from(self.pv_dir_str.trim());
        self.pv_files = preview::list_media(&dir);
        self.pv_msg = format!("共 {} 个媒体文件", self.pv_files.len());
    }

    fn load_pv_texture(&mut self, path: &PathBuf, ctx: &egui::Context) {
        let key = path.display().to_string();
        if self.pv_tex_key == key && self.pv_tex.is_some() {
            return;
        }
        match std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|d| preview::decode_image_rgba(&d))
        {
            Ok((rgba, w, h)) => {
                let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                self.pv_tex = Some(ctx.load_texture("pv", img, egui::TextureOptions::LINEAR));
                self.pv_tex_key = key;
                self.pv_msg = format!("{} × {}", w, h);
            }
            Err(e) => {
                self.pv_tex = None;
                self.pv_msg = e;
            }
        }
    }

    fn pv_play(&mut self, path: &PathBuf) {
        // 停掉旧的
        self.pv_stop();
        let file = std::fs::File::open(path).map_err(|e| format!("打开音频失败: {e}"));
        let Ok(file) = file else { return };
        match (rodio::OutputStream::try_default(), rodio::Decoder::new(std::io::BufReader::new(file))) {
            (Ok((stream, handle)), Ok(src)) => match rodio::Sink::try_new(&handle) {
                Ok(sink) => {
                    sink.set_volume(self.pv_volume);
                    sink.append(src);
                    self.pv_audio = Some(AudioOut { _stream: stream, sink });
                    self.pv_playing = Some(path.clone());
                }
                Err(e) => self.pv_msg = format!("创建播放队列失败: {e}"),
            },
            (Err(e), _) => self.pv_msg = format!("无法访问音频设备: {e}"),
            (_, Err(e)) => self.pv_msg = format!("音频解码失败（可能是不支持的格式，如 m4a/aac）: {e}"),
        }
    }

    fn pv_stop(&mut self) {
        if let Some(a) = self.pv_audio.take() {
            a.sink.stop();
        }
        self.pv_playing = None;
    }

    fn page_preview(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "资源预览", "解包出来的图片可以直接看，音频可以直接试听；把文件或文件夹拖进窗口也行");

        // 目录选择
        if self.pv_dir_str.is_empty() {
            self.pv_dir_str = self.out_dir.display().to_string();
        }
        ui.horizontal(|ui| {
            ui.label("资源目录:");
            let w = (ui.available_width() - 200.0).max(80.0);
            ui.add_sized([w, 22.0], egui::TextEdit::singleline(&mut self.pv_dir_str));
            if ui.button("浏览...").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.pv_dir_str = p.display().to_string();
                    self.refresh_pv_files();
                    self.pv_sel = None;
                }
            }
            if ui.button("🔄 刷新列表").clicked() {
                self.refresh_pv_files();
            }
        });

        // 左列表 + 右预览
        egui::SidePanel::left("pv_list_panel")
            .resizable(true)
            .default_width(300.0)
            .min_width(180.0)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("筛选:");
                    ui.add_sized([ui.available_width() - 8.0, 20.0], egui::TextEdit::singleline(&mut self.pv_filter));
                });
                ui.label(RichText::new(&self.pv_msg).weak().small());
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let f = self.pv_filter.trim().to_lowercase();
                    for p in self.pv_files.clone() {
                        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if !f.is_empty() && !name.to_lowercase().contains(&f) {
                            continue;
                        }
                        let kind = preview::kind_of(&p);
                        let selected = self.pv_sel.as_deref() == Some(p.as_path());
                        if ui
                            .add(egui::SelectableLabel::new(
                                selected,
                                RichText::new(format!("{} {}", kind.icon(), name)).small(),
                            ))
                            .clicked()
                        {
                            self.pv_sel = Some(p.clone());
                            if kind == preview::MediaKind::Image {
                                self.pv_tex = None; // 触发重载
                                self.pv_tex_key.clear();
                            } else if kind == preview::MediaKind::Audio {
                                self.pv_msg.clear();
                            }
                        }
                    }
                    if self.pv_files.is_empty() {
                        ui.label(RichText::new("（这个目录下没有媒体文件）").weak());
                    }
                });
            });

        // 右侧预览区
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let Some(path) = self.pv_sel.clone() else {
                ui.add_space(ui.available_height() * 0.4);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("👈 从左侧选择一个文件预览").weak());
                });
                ui.add_space(ui.available_height() * 0.4);
                return;
            };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            ui.label(RichText::new(&name).strong());
            match preview::kind_of(&path) {
                preview::MediaKind::Image => {
                    self.load_pv_texture(&path, ui.ctx());
                    if let Some(tex) = &self.pv_tex {
                        let size = tex.size_vec2();
                        let avail = ui.available_size() - egui::vec2(16.0, 90.0);
                        let scale = (avail.x / size.x).min(avail.y / size.y).min(1.0).max(0.02);
                        let show = size * scale;
                        ui.add_space(6.0);
                        ui.vertical_centered(|ui| {
                            ui.add(egui::Image::new(tex).max_size(show));
                        });
                    }
                    ui.label(RichText::new(&self.pv_msg).weak().small());
                    if name.to_lowercase().ends_with(".gif") {
                        ui.label(RichText::new("GIF 只显示第一帧").weak().small());
                    }
                }
                preview::MediaKind::Audio => {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("🎵").heading());
                        ui.add_space(8.0);
                        if let Some(playing) = &self.pv_playing {
                            if playing == &path {
                                let done = self.pv_audio.as_ref().map(|a| a.sink.empty()).unwrap_or(true);
                                if done {
                                    ui.label(RichText::new("▶ 播放结束").weak());
                                } else {
                                    ui.label(RichText::new("🔊 正在播放…").color(Color32::from_rgb(120, 200, 120)));
                                }
                            }
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let is_playing = self.pv_playing.as_deref() == Some(path.as_path())
                                && self.pv_audio.as_ref().map(|a| !a.sink.empty()).unwrap_or(false);
                            if ui.add_enabled(!is_playing, egui::Button::new("▶ 播放")).clicked() {
                                self.pv_play(&path);
                            }
                            if ui.button("⏹ 停止").clicked() {
                                self.pv_stop();
                            }
                            ui.label("音量:");
                            ui.add(egui::Slider::new(&mut self.pv_volume, 0.0..=1.0).show_value(false));
                        });
                    });
                    if let Some(err) = preview::audio_supported(&path) {
                        ui.label(RichText::new(err).weak().small());
                    }
                    ui.label(RichText::new(&self.pv_msg).weak().small());
                }
                preview::MediaKind::Other => {
                    ui.label(RichText::new("不支持的预览类型").weak());
                }
            }
        });
    }

    fn page_text(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "对话文本提取 / 翻译回填", "提取台词到 CSV → 翻译 → 回填，实现汉化");
        let Some(d) = self.selected_det().cloned() else {
            ui.label("请先在首页完成检测。");
            return;
        };
        ui.label(format!("当前引擎: {} ({})", d.name, d.plugin_id));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("CSV:");
            ui.add_sized([ui.available_width() - 110.0, 22.0], egui::TextEdit::singleline(&mut self.csv_path_str));
            if ui.button("...").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).save_file() {
                    self.csv_path_str = p.display().to_string();
                    self.csv_path = p;
                }
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("① 提取文本 → CSV").clicked() {
                let plugin_id = d.plugin_id.clone();
                let csv = self.csv_path.clone();
                self.spawn("text_extract", move |shared, root, out| {
                    let opts: HashMap<String, String> = HashMap::new();
                    exec_op(Op::TextExtract, &plugin_id, shared, &root, &out, &opts, Some(&csv))
                });
            }
            if ui.button("② 回填翻译（CSV translation 列）").clicked() {
                let plugin_id = d.plugin_id.clone();
                let csv = self.csv_path.clone();
                self.spawn("text_import", move |shared, root, out| {
                    let opts: HashMap<String, String> = HashMap::new();
                    exec_op(Op::TextImport, &plugin_id, shared, &root, &out, &opts, Some(&csv))
                });
            }
        });
        ui.add_space(8.0);
        ui.label(RichText::new("流程：提取 → 在 Excel/WPS 中翻译 translation 列 → 回填 → 按提示将输出目录放回游戏。").weak());
    }

    // -----------------------------------------------------------------
    // 存档编辑器
    // -----------------------------------------------------------------

    fn page_save(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "存档编辑器", "像 saveeditonline 一样：打开存档 → 自动识别格式 → 搜索/浏览 → 修改 → 保存回原文件（自动备份）");

        let mut doc_opt = self.save_doc.take();
        let mut bufs = std::mem::take(&mut self.save_edit_bufs);

        // --- 工具栏 ---
        ui.horizontal(|ui| {
            if ui.button("📂 打开存档文件...").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("存档", &["rpgsave", "rmmzsave", "rmzsave", "json", "rxdata", "rvdata", "rvdata2", "dat", "sav", "save"])
                    .add_filter("全部文件", &["*"])
                    .pick_file()
                {
                    self.save_path_str = p.display().to_string();
                    match saves::SaveDoc::load(&p) {
                        Ok(d) => {
                            self.save_hits.clear();
                            self.save_sel = None;
                            self.save_dirty = false;
                            bufs.clear();
                            doc_opt = Some(d);
                            self.toast = Some(("存档已加载".into(), std::time::Instant::now()));
                        }
                        Err(e) => self.toast = Some((format!("加载失败: {e}"), std::time::Instant::now())),
                    }
                }
            }
            if ui.button("按路径加载").clicked() {
                let p = PathBuf::from(self.save_path_str.trim());
                match saves::SaveDoc::load(&p) {
                    Ok(d) => {
                        self.save_hits.clear();
                        self.save_sel = None;
                        self.save_dirty = false;
                        bufs.clear();
                        doc_opt = Some(d);
                        self.toast = Some(("存档已加载".into(), std::time::Instant::now()));
                    }
                    Err(e) => self.toast = Some((format!("加载失败: {e}"), std::time::Instant::now())),
                }
            }
            if ui.button("🔄 重新加载").clicked() && doc_opt.is_some() {
                let p = doc_opt.as_ref().unwrap().path.clone();
                match saves::SaveDoc::load(&p) {
                    Ok(d) => {
                        self.save_hits.clear();
                        self.save_sel = None;
                        self.save_dirty = false;
                        bufs.clear();
                        doc_opt = Some(d);
                    }
                    Err(e) => self.toast = Some((format!("重载失败: {e}"), std::time::Instant::now())),
                }
            }
            let can_save = doc_opt.as_ref().map(|d| d.format.writable()).unwrap_or(false);
            if ui.add_enabled(can_save, egui::Button::new("💾 保存回写（自动备份）")).clicked() {
                if let Some(doc) = doc_opt.as_ref() {
                    match doc.save() {
                        Ok(m) => {
                            self.save_dirty = false;
                            self.toast = Some((m, std::time::Instant::now()));
                        }
                        Err(e) => self.toast = Some((format!("保存失败: {e}"), std::time::Instant::now())),
                    }
                }
            }
            if ui.add_enabled(doc_opt.is_some(), egui::Button::new("⬇ 导出 JSON")).clicked() {
                if let Some(doc) = doc_opt.as_ref() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).save_file() {
                        let text = serde_json::to_string_pretty(&doc.root).unwrap_or_default();
                        match std::fs::write(&p, text) {
                            Ok(()) => self.toast = Some((format!("已导出 {}", p.display()), std::time::Instant::now())),
                            Err(e) => self.toast = Some((format!("导出失败: {e}"), std::time::Instant::now())),
                        }
                    }
                }
            }
            if ui.add_enabled(can_save, egui::Button::new("⬆ 导入 JSON")).clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
                    match std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
                        Some(v) => {
                            if let Some(doc) = doc_opt.as_mut() {
                                doc.root = v;
                                self.save_dirty = true;
                                bufs.clear();
                                self.toast = Some(("JSON 已导入（记得保存回写）".into(), std::time::Instant::now()));
                            }
                        }
                        None => self.toast = Some(("导入失败: 不是有效的 JSON 文件".into(), std::time::Instant::now())),
                    }
                }
            }
            if self.save_dirty {
                ui.label(RichText::new("● 有未保存的修改").color(Color32::from_rgb(230, 170, 60)).small());
            }
        });

        ui.horizontal(|ui| {
            ui.label("文件:");
            let w = (ui.available_width() - 80.0).max(80.0);
            ui.add_sized([w, 20.0], egui::TextEdit::singleline(&mut self.save_path_str));
        });

        if let Some(doc) = doc_opt.as_ref() {
            let writable = if doc.format.writable() { "" } else { "（只读视图，不能修改）" };
            ui.label(RichText::new(format!("识别格式: {} {writable}", doc.format.label())).strong());
        }
        ui.add_space(4.0);

        let Some(doc) = doc_opt.as_mut() else {
            self.save_edit_bufs = bufs;
            self.save_doc = doc_opt;
            ui.separator();
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("💾").heading());
                ui.label(RichText::new("把存档文件直接拖进本窗口，或点上方“打开存档文件”").heading());
                ui.add_space(4.0);
                ui.label(RichText::new("支持 RPG Maker MV / MZ 存档与 JSON 存档；改前自动备份，改坏可还原").weak().small());
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for (label, dir) in &self.save_locations {
                    ui.add_space(2.0);
                    ui.label(RichText::new(label).strong());
                    let files = saves::list_files(dir, &["rpgsave", "rmmzsave", "rmzsave", "json", "rxdata", "rvdata", "rvdata2"]);
                    for f in files.iter().take(40) {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).small());
                            if ui.small_button("打开").clicked() {
                                self.save_path_str = f.display().to_string();
                            }
                        });
                    }
                    if files.len() > 40 {
                        ui.label(RichText::new(format!("…共 {} 个文件，可在加载框输入完整路径打开", files.len())).weak().small());
                    }
                }
                if self.save_locations.is_empty() {
                    ui.label("（未发现存档目录，可直接打开任意存档文件）");
                }
            });
            return;
        };

        // --- 搜索 ---
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("🔍 搜索:");
            let w = (ui.available_width() - 260.0).max(80.0);
            ui.add_sized([w, 22.0], egui::TextEdit::singleline(&mut self.save_query));
            egui::ComboBox::from_id_salt("save_scope")
                .selected_text(self.save_scope.label())
                .width(90.0)
                .show_ui(ui, |ui| {
                    for sc in saves::SearchScope::ALL {
                        ui.selectable_value(&mut self.save_scope, sc, sc.label());
                    }
                });
            if ui.button("搜索").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.save_hits = doc.search(&self.save_query, self.save_scope);
                if self.save_hits.is_empty() {
                    self.toast = Some(("没有找到匹配项".into(), std::time::Instant::now()));
                }
            }
            if !self.save_hits.is_empty() && ui.button("清空").clicked() {
                self.save_hits.clear();
            }
        });
        if !self.save_hits.is_empty() {
            ui.label(RichText::new(format!("找到 {} 处，点击行即可修改：", self.save_hits.len())).small());
            egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                for p in self.save_hits.clone() {
                    let val_text = doc.get(&p).map(value_edit_text).unwrap_or_default();
                    let row = format!("{}  =  {}", p, short_text(&val_text, 40));
                    let selected = self.save_sel.as_deref() == Some(p.as_str());
                    if ui.add(egui::SelectableLabel::new(selected, RichText::new(row).monospace().small())).clicked() {
                        self.save_sel = Some(p.clone());
                        self.save_sel_buf = val_text;
                    }
                }
            });
        }

        // --- 选中路径编辑器 ---
        if let Some(sel) = self.save_sel.clone() {
            ui.add_space(4.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("✏ 修改: {sel}")).strong());
                    if ui.small_button("✕ 关闭").clicked() {
                        self.save_sel = None;
                    }
                });
                ui.label(format!("当前值: {}", doc.get(&sel).map(value_edit_text).unwrap_or_else(|| "(不存在)".into())));
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 100.0).max(80.0);
                    ui.add_sized([w, 22.0], egui::TextEdit::singleline(&mut self.save_sel_buf));
                    if ui.button("应用修改").clicked() {
                        match doc.set(&sel, parse_edit_text(&self.save_sel_buf)) {
                            Ok(()) => {
                                self.save_dirty = true;
                                bufs.clear();
                                self.toast = Some(("已修改（记得保存回写）".into(), std::time::Instant::now()));
                            }
                            Err(e) => self.toast = Some((format!("修改失败: {e}"), std::time::Instant::now())),
                        }
                    }
                });
            });
        }

        // --- 树形浏览编辑 ---
        ui.separator();
        ui.label(RichText::new("存档内容（点开折叠项浏览；直接改文本框，离开输入框即生效）:").weak());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let changed = render_json_node(ui, &mut bufs, "", &mut doc.root, 0);
            if changed {
                self.save_dirty = true;
            }
        });

        self.save_edit_bufs = bufs;
        self.save_doc = doc_opt;
    }

    // -----------------------------------------------------------------
    // 运行时修改
    // -----------------------------------------------------------------

    /// MV/MZ 游戏自动接入：调试端口已开就直接连，否则自动找 Game.exe 带参启动再连。
    fn rt_auto_connect(&mut self) {
        if self.rt_game.is_some() || self.rt_connecting.load(Ordering::Relaxed) {
            return;
        }
        let is_mv = self
            .selected_det()
            .map(|d| d.ok() && d.plugin_id == "rpgmaker_mv")
            .unwrap_or(false);
        if !is_mv {
            return;
        }
        let Ok(port) = self.rt_port_str.trim().parse::<u16>() else { return };
        if runtime::probe_port(port) {
            self.rt_msg = "检测到游戏调试端口已开启，正在自动连接…".into();
        } else {
            let Some(exe) = runtime::find_game_exe(&self.game_root) else {
                self.rt_msg = "检测到 MV/MZ 游戏，但没找到启动程序（Game.exe），请手动选择游戏程序后点启动。".into();
                return;
            };
            match runtime::DebugGame::launch(&exe, port) {
                Ok(pid) => self.rt_msg = format!("已自动以调试模式启动游戏（进程 {pid}），正在连接…（若游戏原本已手动开着，请先关闭它避免多开）"),
                Err(e) => {
                    self.rt_msg = format!("自动启动失败：{e}。可手动选择游戏程序后点启动。");
                    return;
                }
            }
        }
        self.rt_start_connect(port);
    }

    /// 后台线程连接调试端口（完成后结果由 update() 收取）。
    fn rt_start_connect(&mut self, port: u16) {
        if self.rt_connecting.load(Ordering::Relaxed) {
            return;
        }
        self.rt_connecting.store(true, Ordering::Relaxed);
        self.rt_msg = "正在连接（游戏启动慢的话最多等 15 秒）…".into();
        let flag = self.rt_connecting.clone();
        let result = self.rt_result.clone();
        std::thread::spawn(move || {
            let g = runtime::DebugGame::connect(port);
            if let Ok(mut r) = result.lock() {
                *r = Some(g);
            }
            flag.store(false, Ordering::Relaxed);
        });
    }

    fn page_runtime(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "运行时修改", "游戏开着就能改，改了立即生效");

        // ============ 方式一：MV/MZ 调试协议 ============
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("方式一：RPG Maker MV / MZ 游戏（按名字精确改金币/变量/开关/物品）").strong());
            ui.horizontal(|ui| {
                ui.label("游戏程序:");
                let w = (ui.available_width() - 320.0).max(80.0);
                ui.add_sized([w, 22.0], egui::TextEdit::singleline(&mut self.rt_exe_str));
                if ui.button("浏览...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("游戏程序", &["exe"]).pick_file() {
                        self.rt_exe_str = p.display().to_string();
                    }
                }
                ui.label("端口:");
                ui.add_sized([52.0, 22.0], egui::TextEdit::singleline(&mut self.rt_port_str));
            });
            ui.horizontal(|ui| {
                if ui.button("▶ 以调试模式启动游戏").clicked() {
                    let exe = PathBuf::from(self.rt_exe_str.trim());
                    match self.rt_port_str.trim().parse::<u16>() {
                        Ok(port) => match runtime::DebugGame::launch(&exe, port) {
                            Ok(pid) => self.rt_msg = format!("游戏已启动（进程 {pid}），等游戏窗口出现后点“连接游戏”。"),
                            Err(e) => self.toast = Some((e, std::time::Instant::now())),
                        },
                        Err(_) => self.toast = Some(("端口要是 0-65535 的数字".into(), std::time::Instant::now())),
                    }
                }
                let connecting = self.rt_connecting.load(Ordering::Relaxed);
                if ui.add_enabled(!connecting && self.rt_game.is_none(), egui::Button::new(if connecting { "连接中…" } else { "🔌 连接游戏" })).clicked() {
                    match self.rt_port_str.trim().parse::<u16>() {
                        Ok(port) => self.rt_start_connect(port),
                        Err(_) => self.toast = Some(("端口要是 0-65535 的数字".into(), std::time::Instant::now())),
                    }
                }
                if self.rt_game.is_some() && ui.button("🔄 刷新数据").clicked() {
                    if let Some(g) = self.rt_game.as_mut() {
                        match g.read_state() {
                            Ok(s) => {
                                self.rt_state = s;
                                self.rt_names = g.read_names().unwrap_or(self.rt_names.take());
                                self.rt_msg = "已刷新。".into();
                            }
                            Err(e) => {
                                self.rt_msg = format!("连接已断开: {e}");
                                self.rt_game = None;
                                self.rt_state = serde_json::Value::Null;
                            }
                        }
                    }
                }
                if self.rt_game.is_some() && ui.button("断开").clicked() {
                    self.rt_game = None;
                    self.rt_state = serde_json::Value::Null;
                    self.rt_msg = "已断开。".into();
                }
            });
            if !self.rt_msg.is_empty() {
                ui.label(RichText::new(&self.rt_msg).small());
            }

            if let Some(g) = self.rt_game.as_mut() {
                let ready = self.rt_state.get("mv").and_then(|v| v.as_bool()).unwrap_or(false);
                if ready {
                    let gold = self.rt_state.get("gold").and_then(|v| v.as_i64()).unwrap_or(0);
                    ui.label(format!("当前金币: {gold}"));
                    ui.add_space(4.0);

                    // 快捷编辑器
                    ui.horizontal(|ui| {
                        ui.label("修改:");
                        egui::ComboBox::from_id_salt("rt_mode")
                            .selected_text(["金币", "变量", "开关", "物品数量"][self.rt_mode])
                            .width(90.0)
                            .show_ui(ui, |ui| {
                                for (i, m) in ["金币", "变量", "开关", "物品数量"].iter().enumerate() {
                                    ui.selectable_value(&mut self.rt_mode, i, *m);
                                }
                            });
                        if self.rt_mode != 0 {
                            ui.label("编号 ID:");
                            ui.add_sized([60.0, 22.0], egui::TextEdit::singleline(&mut self.rt_id_str));
                        }
                        ui.label("新值:");
                        if self.rt_mode == 2 {
                            ui.label(RichText::new("（true=开 / false=关）").weak().small());
                        }
                        ui.add_sized([120.0, 22.0], egui::TextEdit::singleline(&mut self.rt_val_str));
                        if ui.button("✅ 写入游戏").clicked() {
                            let res = match self.rt_mode {
                                0 => self.rt_val_str.trim().parse::<i64>().map_err(|e| format!("金币要是数字: {e}")).and_then(|n| g.set_gold(n).map(|v| format!("金币已改为 {v}"))),
                                1 => match self.rt_id_str.trim().parse::<i64>() {
                                    Ok(id) => g.set_variable(id, &parse_edit_text(&self.rt_val_str)).map(|v| format!("变量 {id} 已改为 {v}")),
                                    Err(_) => Err("变量 ID 要是数字".into()),
                                },
                                2 => match self.rt_id_str.trim().parse::<i64>() {
                                    Ok(id) => {
                                        let on = matches!(self.rt_val_str.trim(), "true" | "开" | "1" | "on" | "TRUE" | "True");
                                        g.set_switch(id, on).map(|v| format!("开关 {id} 已改为 {v}"))
                                    }
                                    Err(_) => Err("开关 ID 要是数字".into()),
                                },
                                _ => match self.rt_id_str.trim().parse::<i64>() {
                                    Ok(id) => match self.rt_val_str.trim().parse::<i64>() {
                                        Ok(n) => g.set_item(id, n).map(|v| format!("物品 {id} 已改为 {v} 个")),
                                        Err(_) => Err("物品数量要是数字".into()),
                                    },
                                    Err(_) => Err("物品 ID 要是数字".into()),
                                },
                            };
                            match res {
                                Ok(m) => {
                                    if let Some(s) = g.read_state().ok() {
                                        self.rt_state = s;
                                    }
                                    self.toast = Some((m, std::time::Instant::now()));
                                }
                                Err(e) => self.toast = Some((format!("写入失败: {e}"), std::time::Instant::now())),
                            }
                        }
                    });

                    // 变量 / 物品一览
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, false]).show(ui, |ui| {
                        if let Some(vars) = self.rt_state.get("variables").and_then(|v| v.as_object()) {
                            ui.label(RichText::new(format!("游戏变量（共 {} 个，点“选”填入上方编辑器）:", vars.len())).strong());
                            let mut ids: Vec<(i64, &serde_json::Value)> = vars
                                .iter()
                                .filter_map(|(k, v)| k.parse::<i64>().ok().map(|id| (id, v)))
                                .filter(|(_, v)| !v.is_null())
                                .collect();
                            ids.sort_by_key(|(id, _)| *id);
                            for (id, v) in ids.iter().take(400) {
                                let nm = json_name(&self.rt_names, "vars", *id);
                                let title = if nm.is_empty() { format!("#{id}") } else { format!("#{id} {nm}") };
                                ui.horizontal(|ui| {
                                    ui.add_sized([170.0, 18.0], egui::Label::new(RichText::new(short_text(&title, 26)).monospace()).truncate());
                                    ui.add(egui::Label::new(RichText::new(short_text(&value_edit_text(v), 60)).monospace()).truncate());
                                    if ui.small_button("选").clicked() {
                                        self.rt_mode = 1;
                                        self.rt_id_str = id.to_string();
                                        self.rt_val_str = value_edit_text(v);
                                    }
                                });
                            }
                            if ids.len() > 400 {
                                ui.label(RichText::new(format!("…其余 {} 个省略，直接输 ID 改", ids.len() - 400)).weak().small());
                            }
                        }
                        if let Some(items) = self.rt_state.get("items").and_then(|v| v.as_object()) {
                            ui.label(RichText::new(format!("持有物品（{} 种）:", items.len())).strong());
                            let mut its: Vec<(i64, i64)> = items
                                .iter()
                                .filter_map(|(k, v)| Some((k.parse::<i64>().ok()?, v.as_i64()?)))
                                .collect();
                            its.sort();
                            for (id, n) in its {
                                let nm = json_name(&self.rt_names, "items", id);
                                let title = if nm.is_empty() { format!("物品 #{id} × {n}") } else { format!("{nm} × {n}") };
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(title).monospace());
                                    if ui.small_button("选").clicked() {
                                        self.rt_mode = 3;
                                        self.rt_id_str = id.to_string();
                                        self.rt_val_str = n.to_string();
                                    }
                                });
                            }
                        }
                    });
                } else {
                    ui.label(RichText::new("游戏核心还没加载：请先进入游戏标题或读一个存档，然后点“刷新数据”。").weak());
                }
            } else {
                ui.label(RichText::new("MV/MZ 游戏在首页检测通过后，进入本页会自动启动并连接（游戏会自动弹出，属正常现象）。如果自动连接失败（比如游戏已经手动开着），请先关掉游戏，再点上面的按钮重试。其他引擎的游戏请用下面的方式二。").weak());
            }
        });

        ui.add_space(8.0);

        // ============ 方式二：通用内存扫描 ============
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("方式二：通用内存扫描（任何游戏都能用，替代 Cheat Engine）").strong());
            ui.label(RichText::new("用法和 Cheat Engine 一样：先记住当前数值（比如金币 100）→ 首次扫描 → 回游戏让数值变化 → 再次扫描过滤 → 剩下的地址里写入新值。").weak());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("🔄 刷新进程列表").clicked() {
                    self.ms_procs = memscan::list_processes();
                    self.toast = Some((format!("共 {} 个进程", self.ms_procs.len()), std::time::Instant::now()));
                }
                ui.label("筛选:");
                ui.add_sized([140.0, 22.0], egui::TextEdit::singleline(&mut self.ms_proc_filter));
            });
            ui.horizontal(|ui| {
                ui.label("目标进程:");
                let filtered: Vec<&(u32, String)> = self
                    .ms_procs
                    .iter()
                    .filter(|(_, n)| self.ms_proc_filter.trim().is_empty() || n.to_lowercase().contains(&self.ms_proc_filter.trim().to_lowercase()))
                    .take(400)
                    .collect();
                egui::ComboBox::from_id_salt("ms_proc")
                    .selected_text(&self.ms_pid_label)
                    .width(320.0)
                    .show_ui(ui, |ui| {
                        for (pid, name) in filtered {
                            let label = format!("{pid} — {name}");
                            if ui.selectable_label(self.ms_pid == *pid, &label).clicked() {
                                self.ms_pid = *pid;
                                self.ms_pid_label = label.clone();
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("数值类型:");
                egui::ComboBox::from_id_salt("ms_ty")
                    .selected_text(self.ms_ty.label())
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for t in memscan::ScanType::ALL {
                            if ui.selectable_label(self.ms_ty == t, t.label()).clicked() && self.ms_ty != t {
                                self.ms_ty = t;
                                // 类型变了，旧会话作废；锁定也要停掉
                                if self.ms_frozen {
                                    self.ms_freeze_stop.store(true, Ordering::Relaxed);
                                    self.ms_frozen = false;
                                }
                                *self.ms_scanner.lock().unwrap() = None;
                                if let Ok(mut st) = self.ms_sh.lock() {
                                    st.msg.clear();
                                }
                            }
                        }
                    });
                ui.label("数值:");
                ui.add(egui::TextEdit::singleline(&mut self.ms_value).desired_width(140.0));
                let busy = self.ms_sh.lock().map(|s| s.busy).unwrap_or(false);
                if ui.add_enabled(!busy && self.ms_pid != 0, egui::Button::new("🔎 首次扫描")).clicked() {
                    self.ms_start_scan(true);
                }
                ui.label("过滤:");
                egui::ComboBox::from_id_salt("ms_filter")
                    .selected_text(self.ms_filter.label())
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for f in memscan::Filter::ALL {
                            ui.selectable_value(&mut self.ms_filter, f, f.label());
                        }
                    });
                if ui.add_enabled(!busy && self.ms_pid != 0, egui::Button::new("再次扫描")).clicked() {
                    self.ms_start_scan(false);
                }
            });
            ui.horizontal(|ui| {
                let busy = self.ms_sh.lock().map(|s| s.busy).unwrap_or(false);
                ui.label(RichText::new("扫描完成后:").weak());
                if ui.add_enabled(!busy, egui::Button::new("✅ 写入所有命中")).clicked() {
                    self.ms_write_all();
                }
                if ui.add_enabled(!busy && self.ms_pid != 0, egui::Button::new(if self.ms_frozen { "🔓 解锁数值" } else { "🔒 锁定数值" })).clicked() {
                    self.ms_toggle_freeze();
                }
                if ui.add_enabled(!busy, egui::Button::new("↩ 撤销写入")).clicked() {
                    self.ms_undo();
                }
                if self.ms_frozen {
                    ui.label(RichText::new("● 已锁定").color(Color32::from_rgb(220, 120, 120)).small());
                }
            });
            if let Ok(st) = self.ms_sh.lock() {
                if !st.msg.is_empty() {
                    ui.label(RichText::new(&st.msg).small());
                }
            }
            // 命中列表
            if let Ok(opt) = self.ms_scanner.lock() {
                if let Some(s) = opt.as_ref() {
                    if s.first_done {
                        ui.label(RichText::new(format!("当前命中 {} 处（显示前 200）：", s.hit_count())).strong());
                        egui::ScrollArea::vertical().max_height(240.0).auto_shrink([false, false]).show(ui, |ui| {
                            for h in s.hits.iter().take(200) {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(format!("0x{:016X}", h.addr)).weak().monospace().small());
                                    ui.label(RichText::new(h.value.display()).monospace());
                                });
                            }
                        });
                    }
                }
            }
        });
    }

    /// 后台线程执行首次/再次扫描。
    fn ms_start_scan(&mut self, first: bool) {
        let value = self.ms_value.trim().to_string();
        let filter = self.ms_filter;
        let scanner = self.ms_scanner.clone();
        let sh = self.ms_sh.clone();
        {
            let mut st = sh.lock().unwrap();
            if st.busy {
                self.toast = Some(("扫描还在进行中，稍等…".into(), std::time::Instant::now()));
                return;
            }
            st.busy = true;
            st.msg = if first { "正在扫描全部内存，可能要几秒到几十秒…".into() } else { "正在过滤…".into() };
        }
        std::thread::spawn(move || {
            let msg = match scanner.lock() {
                Ok(mut opt) => match opt.as_mut() {
                    Some(s) => {
                        let r = if first {
                            s.first_scan(&value).map(|n| format!("首次扫描完成：命中 {n} 处。回游戏改变数值后用“再次扫描”过滤。"))
                        } else {
                            s.next_scan(&value, filter).map(|n| format!("过滤完成：剩 {n} 处。"))
                        };
                        match r {
                            Ok(m) => m,
                            Err(e) => format!("✘ {e}"),
                        }
                    }
                    None => "✘ 请先选择目标进程".into(),
                },
                Err(_) => "✘ 扫描器状态异常".into(),
            };
            if let Ok(mut st) = sh.lock() {
                st.busy = false;
                st.msg = msg;
            }
        });
    }

    /// 撤销写入：恢复所有被写过的地址的原始值。
    fn ms_undo(&mut self) {
        let result = match self.ms_scanner.lock() {
            Ok(mut opt) => match opt.as_mut() {
                Some(s) if s.has_saved() => {
                    let n = s.undo();
                    s.refresh();
                    Ok::<usize, String>(n)
                }
                Some(_) => Err("没有可撤销的写入".into()),
                None => Err("请先扫描".into()),
            },
            Err(_) => Err("扫描器状态异常".into()),
        };
        match result {
            Ok(n) => self.toast = Some((format!("已恢复 {n} 处原值"), std::time::Instant::now())),
            Err(e) => self.toast = Some((format!("✘ {e}"), std::time::Instant::now())),
        }
    }

    /// 数值锁定：后台线程每 150ms 反复写入当前输入框的值（Cheat Engine 的 Freeze）。
    fn ms_toggle_freeze(&mut self) {
        if self.ms_frozen {
            self.ms_freeze_stop.store(true, Ordering::Relaxed);
            self.ms_frozen = false;
            self.toast = Some(("已解除锁定".into(), std::time::Instant::now()));
            return;
        }
        let value = self.ms_value.trim().to_string();
        if value.is_empty() {
            self.toast = Some(("请先在数值框填入要锁定的值".into(), std::time::Instant::now()));
            return;
        }
        let scanner = self.ms_scanner.clone();
        let stop = Arc::new(AtomicBool::new(false));
        self.ms_freeze_stop = stop.clone();
        self.ms_frozen = true;
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Ok(opt) = scanner.lock() {
                    if let Some(s) = opt.as_ref() {
                        let _ = s.write_raw(&value, None);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        });
        self.toast = Some(("已锁定该数值（游戏里改不掉；要改锁定值请先解锁再锁定）".into(), std::time::Instant::now()));
    }

    /// 把新值写入所有命中地址（用当前输入框的值）。
    fn ms_write_all(&mut self) {
        let value = self.ms_value.trim().to_string();
        let result = {
            let mut opt = match self.ms_scanner.lock() {
                Ok(o) => o,
                Err(_) => return,
            };
            match opt.as_mut() {
                Some(s) => s.write(&value, None).map(|n| {
                    s.refresh();
                    format!("已把 {n} 处地址改为 {value}，回游戏看看效果！")
                }),
                None => Err("请先扫描".into()),
            }
        };
        match result {
            Ok(m) => self.toast = Some((m, std::time::Instant::now())),
            Err(e) => self.toast = Some((format!("✘ {e}"), std::time::Instant::now())),
        }
    }

    // -----------------------------------------------------------------

    fn page_mods(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "补丁 / MOD 管理", "安装补丁目录覆盖到游戏，可停用/卸载并自动还原");
        ui.label(RichText::new("安装 = 把补丁目录覆盖到游戏目录（原文件自动备份到 stool_mods/_backups/）。").weak());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("名称:");
            ui.add_sized([180.0, 22.0], egui::TextEdit::singleline(&mut self.mod_name));
            if ui.button("选择补丁目录...").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    match crate::features::mods::install_mod(&self.game_root, &p, &self.mod_name, &|_, _| {}) {
                        Ok(e) => {
                            self.toast = Some((format!("已安装 {}", e.name), std::time::Instant::now()));
                            self.mod_name.clear();
                            self.mods = crate::features::mods::list_mods(&self.game_root);
                        }
                        Err(e) => self.toast = Some((e, std::time::Instant::now())),
                    }
                }
            }
        });
        ui.add_space(10.0);
        ui.heading("已安装");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for m in self.mods.clone() {
                ui.horizontal(|ui| {
                    let state = if m.enabled { "[启用]" } else { "[停用]" };
                    ui.label(format!("{state} {} — {} 文件（覆盖 {}）", m.name, m.files.len(), m.overwritten.len()));
                    if ui.button(if m.enabled { "停用" } else { "启用" }).clicked() {
                        if let Err(e) = crate::features::mods::toggle_mod(&self.game_root, &m.name, !m.enabled) {
                            self.toast = Some((e, std::time::Instant::now()));
                        }
                        self.mods = crate::features::mods::list_mods(&self.game_root);
                    }
                    if ui.button("卸载").clicked() {
                        match crate::features::mods::uninstall_mod(&self.game_root, &m.name) {
                            Ok(()) => {
                                self.toast = Some((format!("已卸载 {}", m.name), std::time::Instant::now()));
                                self.mods = crate::features::mods::list_mods(&self.game_root);
                            }
                            Err(e) => self.toast = Some((e, std::time::Instant::now())),
                        }
                    }
                });
            }
            if self.mods.is_empty() {
                ui.label("（无）");
            }
        });
    }

    fn page_settings(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "设置", "外部工具路径与代理；现在可以一键自动下载并配置");
        let dl_busy = self.dl_busy.load(Ordering::Relaxed);
        ui.label(RichText::new("不用自己找工具：点“⬇ 下载”会自动从官方 GitHub 下载到 ~/.stool/tools/ 并填好路径（走下方代理）。").weak());
        ui.add_space(4.0);
        let mut dl_key: Option<String> = None;
        {
            let cfg = &mut self.cfg;
            for (label, key) in [
                ("WolfDec.exe（Wolf 解包）", "wolfdec"),
                ("AssetRipper CLI（Unity 解包）", "asset_ripper"),
                ("GARbro CLI（未知格式兜底）", "garbro"),
                ("GDRE Tools（Godot 反编译）", "gdre_tools"),
                ("unrpyc.py（Ren'Py 反编译）", "unrpyc"),
                ("Python 解释器（外挂用）", "python"),
                ("代理", "proxy"),
            ] {
                let has_dl = tools_dl::spec_by_key(key).is_some();
                ui.label(RichText::new(label).weak().small());
                ui.horizontal(|ui| {
                    let v = match key {
                        "wolfdec" => &mut cfg.wolfdec,
                        "asset_ripper" => &mut cfg.asset_ripper,
                        "garbro" => &mut cfg.garbro,
                        "gdre_tools" => &mut cfg.gdre_tools,
                        "unrpyc" => &mut cfg.unrpyc,
                        "python" => &mut cfg.python,
                        _ => &mut cfg.proxy,
                    };
                    let w = ui.available_width() - if has_dl { 96.0 } else { 12.0 };
                    ui.add_sized([w.max(200.0), 22.0], egui::TextEdit::singleline(v));
                    if has_dl && ui.add_enabled(!dl_busy, egui::Button::new("⬇ 下载")).clicked() {
                        dl_key = Some(key.to_string());
                    }
                });
                ui.add_space(4.0);
            }
        }
        if dl_busy {
            ui.label(RichText::new("⏳ 正在后台下载（大工具可能要一两分钟，完成后会提示）…").small());
        }
        if let Some(key) = dl_key {
            self.start_tool_download(&key);
        }
        ui.add_space(8.0);
        if ui.button("保存设置").clicked() {
            match crate::settings::save(&self.cfg) {
                Ok(()) => {
                    self.settings_saved_at = Some(std::time::Instant::now());
                    self.toast = Some(("设置已保存".into(), std::time::Instant::now()));
                }
                Err(e) => self.toast = Some((format!("保存失败: {e}"), std::time::Instant::now())),
            }
        }
        if let Some(t) = self.settings_saved_at {
            ui.label(RichText::new(format!("已保存（{:?} 前）", t.elapsed())).weak().small());
        }
        ui.add_space(12.0);
        ui.separator();
        ui.label(RichText::new("外部工具说明").strong());
        ui.label("推荐直接点上方“⬇ 下载”按钮：工具会从官方 GitHub Release 自动下载、解压到 ~/.stool/tools/ 并自动填好路径。下载不动时检查上方代理设置。手动安装的话：把可执行文件完整路径填到上方保存即可。");
    }

    fn page_help(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "使用指南", "大部分操作都能靠“把文件/文件夹拖进窗口”完成");
        let sections: Vec<(&str, Vec<&str>)> = vec![
            ("🚀 快速上手", vec![
                "把游戏文件夹直接拖进本窗口 → 自动识别引擎；把存档文件拖进窗口 → 直接进入存档编辑器",
                "左侧页面按用途排列：解包资源、翻译文本、改存档、改运行中的游戏、装补丁",
            ]),
            ("📦 解包与汉化", vec![
                "资源解包页：点按钮即可把封包里的图片/音频/脚本提取到输出目录，不会动游戏原文件",
                "文本/汉化页：① 提取文本到 CSV → ② 在 Excel/WPS 里翻译 translation 列 → 回填",
            ]),
            ("💾 存档编辑", vec![
                "打开存档文件（MV 的在游戏目录 save/ 下，后缀 .rpgsave）→ 自动识别格式",
                "搜索数值或键名（如金币数、角色名）→ 点击结果定位 → 改值 → 保存回写（自动备份 .stool.bak）",
                "支持导出/导入 JSON，方便备份或分享修改",
            ]),
            ("🎯 运行时修改", vec![
                "MV/MZ 游戏：首页检测后进入本页自动启动并连接，改金币/变量/物品立即生效",
                "其他游戏：用通用内存扫描，用法与 Cheat Engine 相同：首次扫描 → 变数值 → 再次扫描过滤 → 写入",
                "扫描完成后可以“锁定数值”（游戏里改不掉）或“撤销写入”（恢复原值）",
            ]),
            ("🧩 补丁 MOD", vec![
                "选择补丁文件夹一键覆盖安装，可随时停用/卸载，卸载时自动还原原文件",
            ]),
            ("⌨ 命令行", vec![
                "stool detect/extract/decompile/text-extract/text-import/save/unlock/mod-*",
                "stool save-edit 存档文件 --search 关键词 / --set /路径=新值（脚本批量改存档）",
            ]),
            ("❓ 常见问题", vec![
                "扫不到内存或打不开进程 → 以管理员身份运行 STool",
                "MV/MZ 已手动开着连不上 → 先关闭游戏再重试（避免多开）",
                "解包后文件在哪 → 底部状态栏点“📂 输出目录”直接打开",
            ]),
        ];
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (title, lines) in sections {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(title).strong());
                    for l in lines {
                        ui.label(RichText::new(l).small());
                    }
                });
                ui.add_space(2.0);
            }
        });
    }

    fn page_log(&mut self, ui: &mut egui::Ui) {
        ui.heading("运行日志");
        ui.add_space(4.0);
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            if let Ok(logs) = self.shared.logs.lock() {
                let text_color = ui.visuals().text_color();
                let weak = ui.visuals().weak_text_color();
                for line in logs.iter() {
                    let color = if line.contains("✔") {
                        Color32::from_rgb(120, 200, 120)
                    } else if line.contains("✘") {
                        Color32::from_rgb(235, 120, 120)
                    } else if line.contains("▶") {
                        Color32::from_rgb(120, 170, 235)
                    } else {
                        text_color
                    };
                    // 时间戳淡显，正文按状态着色
                    if let Some((ts, body)) = line.split_once(']') {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(format!("{ts}]")).small().color(weak).monospace());
                            ui.label(RichText::new(body.trim_start()).small().color(color));
                        });
                    } else {
                        ui.label(RichText::new(line).small().color(color));
                    }
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// 存档树渲染辅助
// ---------------------------------------------------------------------------

/// 统一页头：大标题 + 弱副标题 + 分隔间距。
fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(title).heading().strong());
    if !subtitle.is_empty() {
        ui.add_space(2.0);
        ui.label(RichText::new(subtitle).weak().small());
    }
    ui.add_space(8.0);
}

/// 转义 JSON Pointer 里的 ~ 和 /。
fn escape_ptr(k: &str) -> String {
    k.replace('~', "~0").replace('/', "~1")
}

fn container_len(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::Object(m) => m.len(),
        serde_json::Value::Array(a) => a.len(),
        _ => 0,
    }
}

fn type_hint(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::String(_) => "文本",
        serde_json::Value::Number(_) => "数值",
        serde_json::Value::Bool(_) => "布尔",
        serde_json::Value::Null => "空",
        _ => "",
    }
}

/// 值的编辑文本表示（字符串去引号，其他用 JSON 形式）。
fn value_edit_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 编辑文本解析回 JSON 值：true/false/null → 对应类型，数字 → 数值，其余 → 字符串。
fn parse_edit_text(s: &str) -> serde_json::Value {
    let t = s.trim();
    match t {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        "null" => serde_json::Value::Null,
        _ => {
            if let Ok(i) = t.parse::<i64>() {
                serde_json::json!(i)
            } else if let Ok(f) = t.parse::<f64>() {
                serde_json::json!(f)
            } else {
                serde_json::Value::String(s.to_string())
            }
        }
    }
}

fn short_text(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

/// 从运行时名称表里取第 id 个名字（没有就返回空串）。
fn json_name(names: &serde_json::Value, key: &str, id: i64) -> String {
    if id < 0 {
        return String::new();
    }
    names
        .get(key)
        .and_then(|v| v.get(id as usize))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// 用系统文件管理器打开目录。
#[cfg(windows)]
fn open_in_explorer(p: &std::path::Path) {
    let _ = std::process::Command::new("explorer").arg(p).spawn();
}
#[cfg(not(windows))]
fn open_in_explorer(_p: &std::path::Path) {}

/// 递归渲染 JSON 节点；返回是否有值被修改。
fn render_json_node(
    ui: &mut egui::Ui,
    bufs: &mut HashMap<String, String>,
    ptr: &str,
    node: &mut serde_json::Value,
    depth: usize,
) -> bool {
    let mut changed = false;
    match node {
        serde_json::Value::Object(map) => {
            if map.len() > 3000 {
                ui.label(RichText::new(format!("（{} 项太多，请用搜索定位修改）", map.len())).weak());
                return false;
            }
            let keys: Vec<String> = map.keys().cloned().collect();
            for k in keys {
                let child_ptr = format!("{ptr}/{}", escape_ptr(&k));
                let Some(v) = map.get_mut(&k) else { continue };
                if v.is_object() || v.is_array() {
                    let title = format!("📁 {k}（{}）", container_len(v));
                    let ok = render_container(ui, bufs, &child_ptr, v, &title, depth);
                    changed |= ok;
                } else {
                    changed |= render_leaf(ui, bufs, &child_ptr, &k, v);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            if arr.len() > 3000 {
                ui.label(RichText::new(format!("（{} 项太多，请用搜索定位修改）", arr.len())).weak());
                return false;
            }
            for (i, v) in arr.iter_mut().enumerate() {
                let child_ptr = format!("{ptr}/{i}");
                if v.is_object() || v.is_array() {
                    let title = format!("📁 [{i}]（{}）", container_len(v));
                    changed |= render_container(ui, bufs, &child_ptr, v, &title, depth);
                } else {
                    changed |= render_leaf(ui, bufs, &child_ptr, &format!("[{i}]"), v);
                }
            }
        }
        other => {
            // 根节点直接是标量（罕见）
            changed |= render_leaf(ui, bufs, ptr, "(根)", other);
        }
    }
    changed
}

fn render_container(
    ui: &mut egui::Ui,
    bufs: &mut HashMap<String, String>,
    ptr: &str,
    node: &mut serde_json::Value,
    title: &str,
    depth: usize,
) -> bool {
    let mut changed = false;
    egui::CollapsingHeader::new(RichText::new(title).strong())
        .id_salt(ptr)
        .default_open(depth < 1)
        .show(ui, |ui| {
            changed = render_json_node(ui, bufs, ptr, node, depth + 1);
        });
    changed
}

fn render_leaf(
    ui: &mut egui::Ui,
    bufs: &mut HashMap<String, String>,
    ptr: &str,
    key: &str,
    node: &mut serde_json::Value,
) -> bool {
    let text = value_edit_text(node);
    let buf = bufs.entry(ptr.to_string()).or_insert_with(|| text.clone());
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized([150.0, 18.0], egui::Label::new(RichText::new(short_text(key, 40)).monospace()).truncate());
        let w = (ui.available_width() - 56.0).max(60.0);
        let mut r = ui.add_sized([w, 20.0], egui::TextEdit::singleline(buf));
        if !type_hint(node).is_empty() {
            r = r.on_hover_text(type_hint(node));
        }
        if r.lost_focus() {
            if *buf != text {
                *node = parse_edit_text(buf);
                *buf = value_edit_text(node);
                changed = true;
            }
        } else if !r.has_focus() && *buf != text {
            // 外部（搜索编辑器）改了值，同步显示
            *buf = text;
        }
    });
    changed
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
            fonts.font_data.insert("cjk".into(), egui::FontData::from_owned(data).into());
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
        fonts.font_data.insert("emoji".into(), egui::FontData::from_owned(data).into());
        fonts.families.entry(egui::FontFamily::Proportional).or_default().push("emoji".into());
        fonts.families.entry(egui::FontFamily::Monospace).or_default().push("emoji".into());
    }
    cc.egui_ctx.set_fonts(fonts);
}
