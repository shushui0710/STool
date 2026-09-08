//! STool 命令行入口（控制台程序）。
//!
//! 图形界面请双击 stool.exe；终端 / 脚本场景使用本程序：
//!   stool-cli detect <游戏目录>
//!   stool-cli extract <游戏目录> -o 输出目录
//!   ...（子命令与 stool 相同）

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        println!("STool 命令行版 —— 图形界面请双击 stool.exe");
        println!();
        println!("用法示例:");
        println!("  stool-cli detect <游戏目录>            # 识别游戏引擎");
        println!("  stool-cli extract <游戏目录> -o 输出目录 # 解包资源");
        println!("  stool-cli save-edit <存档文件> --search 关键词");
        std::process::exit(2);
    }
    let code = stool::cli::main_args(args);
    std::process::exit(code);
}
