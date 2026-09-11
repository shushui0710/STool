//! STool 图形界面入口。
//!
//! release 版声明为 Windows 图形子系统：双击 stool.exe 不再弹出控制台黑窗口。
//! debug 版保留控制台，方便看日志。命令行功能请使用 stool-cli.exe。

#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    stool::diag::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = if args.is_empty() {
        stool::gui::run()
    } else {
        // 带参数时仍路由到 CLI（提示：图形版在终端里可能看不到输出，
        // 终端场景请改用 stool-cli.exe）
        stool::cli::main_args(args)
    };
    std::process::exit(code);
}
