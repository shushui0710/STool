//! STool 命令行入口（控制台程序）。
//!
//! 终端 / 脚本场景使用本程序；**图形界面不在这里** —— 它已统一到 Tauri 版
//! （`stool-tauri.exe`），本 crate 不再自带 GUI。子命令与用法见 `--help`：
//!   stool-cli detect <游戏目录>
//!   stool-cli extract <游戏目录> -o 输出目录
//!   stool-cli --help                （列出全部命令）

fn main() {
    stool::diag::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // 无参数时的指引（含 GUI 去向）只在 cli::main_args 里写一份，这里不另抄 ——
    // 两处各写一份，改了其中一处就会出现「同一程序两种说法」。
    let code = stool::cli::main_args(args);
    std::process::exit(code);
}
