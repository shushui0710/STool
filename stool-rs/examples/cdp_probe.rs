//! 通道1（RPG Maker MV/MZ 调试端口）自检：不经过图形界面，直接验证
//! 「启动 → 连接 → 读状态 → 改数值」这条链。
//!
//! 用法（推荐先 `cd` 到游戏目录，用 `.` 当参数，避免中文路径的编码问题）：
//! ```text
//! cd /d "<游戏目录>"
//! cargo run --manifest-path D:\STool\stool-rs\Cargo.toml --example cdp_probe -- . 7654 --set-gold 12345
//! ```
//!
//! 会带 `--remote-debugging-port` 把游戏拉起来（游戏窗口正常弹出，属正常），
//! 连上后打印名称表与状态；带 `--set-gold N` 时顺手改一次金币。
//!
//! 排查「连接调试端口 7654 失败」这类问题时，比反复开界面点按钮快得多。

use std::path::PathBuf;
use std::time::Duration;

use stool::features::runtime::{find_game_exe, probe_port, DebugGame};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("用法: cdp_probe <游戏目录> [端口] [--set-gold N]");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);
    let mut port: u16 = 7654;
    let mut set_gold: Option<i64> = None;
    let mut wait = 8u64;
    let mut evals: Vec<String> = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--set-gold" => set_gold = args.next().and_then(|v| v.parse().ok()),
            "--wait" => wait = args.next().and_then(|v| v.parse().ok()).unwrap_or(8),
            "--eval" => {
                if let Some(js) = args.next() {
                    evals.push(js);
                }
            }
            other => {
                if let Ok(p) = other.parse::<u16>() {
                    port = p;
                }
            }
        }
    }

    let exe = find_game_exe(&root).unwrap_or_else(|| {
        eprintln!("游戏目录里没找到启动程序（Game.exe / nw.exe）: {}", root.display());
        std::process::exit(1);
    });
    println!("[1] 启动程序: {}", exe.display());

    let already = probe_port(port);
    println!("[2] 端口 {port} 现状: {}", if already { "已开（游戏已在调试模式运行）" } else { "未开" });
    if !already {
        let pid = DebugGame::launch(&exe, port).unwrap_or_else(|e| {
            eprintln!("启动游戏失败: {e}");
            std::process::exit(1);
        });
        println!("[3] 已带调试参数启动 pid={pid}，等 {wait}s 让它开好端口…");
        std::thread::sleep(Duration::from_secs(wait));
    }

    let mut g = match DebugGame::connect(port) {
        Ok(g) => {
            println!("[4] 连接成功：http /json 与 WebSocket 都通了");
            g
        }
        Err(e) => {
            eprintln!("[4] 连接失败: {e}");
            std::process::exit(1);
        }
    };

    match g.is_rpgm_page() {
        Ok(v) => println!("[5] is_rpgm_page = {v}"),
        Err(e) => println!("[5] is_rpgm_page 出错: {e}"),
    }

    // 回归用：把核心全局量打回 null，精确复现「停在标题画面 / 还没进存档」那一态。
    // 用环境变量开关，免得为了复现去改命令行（避免中文/引号在 cmd 里出问题）。
    if std::env::var("STOOL_PROBE_SIM_NOT_READY").is_ok() {
        let js = "$dataSystem=null,$gameParty=null,$gameVariables=null,$gameSwitches=null";
        println!("[sim] 把核心全局量置回 null（模拟没进存档）");
        let _ = g.eval(js);
    }

    match g.is_rpgm_ready() {
        Ok(v) => println!("[6] is_rpgm_ready = {v}（false = 还在标题画面，没进存档）"),
        Err(e) => println!("[6] is_rpgm_ready 出错: {e}"),
    }

    for js in &evals {
        match g.eval(js) {
            Ok(v) => println!("[eval] {js}\n        => {v}"),
            Err(e) => println!("[eval] {js}\n        => 失败: {e}"),
        }
    }
    if !evals.is_empty() {
        std::thread::sleep(Duration::from_secs(1)); // 让场景切完再读
    }

    match g.read_names() {
        Ok(n) => {
            let len = |k: &str| n.get(k).and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            println!("[7] 名称表: 变量 {} / 开关 {} / 物品 {}", len("vars"), len("sw"), len("items"));
            if let Some(n0) = n.get("vars").and_then(|v| v.as_array()).and_then(|a| a.get(1)).and_then(|v| v.as_str()) {
                println!("        变量 #1 名字 = {n0:?}");
            }
        }
        Err(e) => println!("[7] 读名称表失败: {e}"),
    }

    match g.read_state() {
        Ok(st) => {
            println!(
                "[8] 状态: mv={} gold={} vars={} switches={} items={}",
                st.get("mv").and_then(|v| v.as_bool()).unwrap_or(false),
                st.get("gold").map(|v| v.to_string()).unwrap_or_else(|| "null".into()),
                st.get("variables").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
                st.get("switches").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
                st.get("items").and_then(|v| v.as_object()).map(|a| a.len()).unwrap_or(0),
            );
        }
        Err(e) => println!("[8] 读状态失败: {e}"),
    }

    if let Some(n) = set_gold {
        match g.set_gold(n) {
            Ok(v) => println!("[9] 金币已改成 {n}，游戏返回 {v}（回游戏菜单里确认）"),
            Err(e) => println!("[9] 改金币失败: {e}"),
        }
    }
    println!("完成。");
}
