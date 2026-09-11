//! STool 库根。

pub mod cli;
pub mod diag;
pub mod engines;
pub mod features;
pub mod formats;
pub mod gui;
pub mod settings;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
