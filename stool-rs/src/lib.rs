//! STool 库根。

pub mod cli;
pub mod diag;
pub mod engines;
pub mod features;
pub mod formats;
/// 图形界面（eframe/egui）。Tauri 壳以 `default-features = false` 依赖本 crate，
/// 只取内核、不编译这一层；见 docs/TAURI重构方案.md §6.1。
#[cfg(feature = "gui")]
pub mod gui;
pub mod hash;
pub mod memapi;
pub mod settings;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
