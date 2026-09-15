//! STool 的**唯一**图形界面（Rust 内核 + WebView2）。
//!
//! 它就是面向用户的 GUI：旧的 eframe/egui 版已删除，`stool-rs` 只留内核 + CLI。
//! 本 crate 依赖 `stool` 内核，取 `engines` / `features` 等模块
//! （见 `docs/TAURI重构方案.md`）。
//!
//! 构建提醒：不要用裸 `cargo build` 之外的方式绕开 `custom-protocol` feature，
//! 否则会得到一个**静默的白窗口**（同文档 §6.2）。

#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

mod cmd;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// 全局状态：当前游戏目录 / 检测出的引擎 / 当前打开的存档 / 是否有未写回的改动 / 取消标志。
#[derive(Default)]
pub struct AppState {
    pub game_root: Mutex<Option<String>>,
    /// 最近一次检测出的引擎 id —— 提取、封包、解锁都要用它选插件，
    /// 不能靠界面每次重传（否则前后端两处状态会漂移）。
    pub engine_id: Mutex<Option<String>>,
    pub save: Mutex<Option<stool::features::saves::SaveDoc>>,
    pub dirty: Mutex<bool>,
    /// 长任务（解包/封包）的取消标志。沿用内核 `Ctx::cancel` 的口径，
    /// 所以「界面点取消」与「内核在条目边界退出」是同一条链路。
    pub cancel: Arc<AtomicBool>,
    /// ⑥ 游戏里改数值：「方式二」通用内存扫描的会话（含目标进程句柄）。
    ///
    /// 会话要跨多次 IPC 存活（扫描 → 回游戏 → 再次扫描 → 写入），
    /// 所以不能放在某个命令的局部变量里；同时它持有进程句柄（RAII），
    /// 换进程 / 退页面时会被替换或丢弃，句柄随之关闭。
    ///
    /// 外面套一层 `Arc` 是必需的：扫描 / 写入要在 `spawn_blocking` 里跑
    /// （否则阻塞 IO 会堵住主线程，见 `cmd::offload`），而 `tauri::State`
    /// **不能跨线程传递** —— 只能先 clone 出这个 `Arc` 再 move 进闭包。
    pub scanner: Arc<Mutex<Option<crate::cmd::Session>>>,
    /// ⑥ 游戏里改数值：「方式一」MV/MZ 调试协议的会话（CDP 连接 + 名称表）。
    pub mvmz: Arc<Mutex<Option<crate::cmd::MvmzSession>>>,
}

fn main() {
    // 复用内核的日志初始化（写到内核既定的日志位置，而不是另起一套）。
    stool::diag::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            cmd::app_info,
            cmd::log_line,
            cmd::pick_folder,
            cmd::pick_file,
            cmd::env_game_root,
            cmd::detect_game,
            cmd::save_locations,
            cmd::save_files,
            cmd::load_save,
            cmd::pick_and_load_save,
            cmd::search_save,
            cmd::set_save_value,
            cmd::save_dirty,
            cmd::commit_save,
            cmd::restore_list,
            cmd::restore_one,
            cmd::extract_assets,
            cmd::repack_assets,
            cmd::cancel_task,
            cmd::open_folder,
            cmd::scan_media,
            cmd::read_media,
            cmd::open_file,
            cmd::text_extract,
            cmd::text_import,
            cmd::text_stats,
            cmd::csv_stats,
            cmd::text_make_json,
            cmd::text_inject,
            cmd::text_uninject,
            cmd::text_mtl,
            cmd::mtl_save,
            cmd::unlock_plan,
            cmd::unlock_run,
            cmd::unlock_backups,
            cmd::unlock_restore,
            cmd::runtime_processes,
            cmd::runtime_options,
            cmd::runtime_open,
            cmd::runtime_state,
            cmd::runtime_scan,
            cmd::runtime_hits,
            cmd::runtime_write,
            cmd::runtime_undo,
            cmd::runtime_freeze,
            cmd::runtime_freeze_tick,
            cmd::runtime_diag,
            cmd::runtime_regions,
            cmd::runtime_patch_list,
            cmd::runtime_patch_build,
            cmd::runtime_patch_remove,
            cmd::runtime_mvmz_status,
            cmd::runtime_mvmz_connect,
            cmd::runtime_mvmz_disconnect,
            cmd::runtime_mvmz_refresh,
            cmd::runtime_mvmz_set,
            cmd::mods_state,
            cmd::mods_preview,
            cmd::mods_install,
            cmd::mods_toggle,
            cmd::mods_uninstall,
            cmd::tools_settings,
            cmd::tools_save,
            cmd::tool_set,
            cmd::tool_download,
            cmd::tools_health,
            cmd::tools_selfcheck,
            cmd::tools_log,
            cmd::tools_export_diag,
            cmd::env_query,
            cmd::env_save_path,
            cmd::env_out_dir,
            cmd::env_page,
        ])
        .run(tauri::generate_context!())
        .expect("STool 启动失败");
}
