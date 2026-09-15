//! STool 图形界面入口。
//!
//! release 版声明为 Windows 图形子系统：双击 stool.exe 不再弹出控制台黑窗口。
//! debug 版保留控制台，方便看日志。命令行功能请使用 stool-cli.exe。

#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    stool::diag::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = if args.is_empty() {
        #[cfg(feature = "gui")]
        {
            stool::gui::run()
        }
        // 关掉 gui feature 时的构建（如给 Tauri 壳复用内核）：无参数启动要给出明确指引，
        // 不能静默退 0 让人以为"打开了但没反应"。
        #[cfg(not(feature = "gui"))]
        {
            eprintln!("本构建未启用图形界面（gui feature 已关闭）。");
            eprintln!("改法：用 stool-cli.exe 走命令行，或按默认 feature 重新构建（cargo build --release）。");
            2
        }
    } else {
        // 带参数时仍路由到 CLI（提示：图形版在终端里可能看不到输出，
        // 终端场景请改用 stool-cli.exe）
        stool::cli::main_args(args)
    };
    std::process::exit(code);
}
