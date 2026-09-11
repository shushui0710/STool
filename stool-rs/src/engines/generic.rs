//! 未知引擎兜底插件：特征提示 + GARbro 委派。

use std::path::Path;
use std::process::Command;

use super::scan::ScanCtx;
use super::{Ctx, Detection, Engine, Op, OpOutcome};
use crate::settings;

/// 扩展名 → 疑似引擎。用于"未识别"时给出人话提示（不再只是"未知"）。
const KNOWN_EXT: &[(&str, &str)] = &[
    ("xp3", "KiriKiri"), ("rpa", "Ren'Py"), ("rgssad", "RPG Maker XP"),
    ("rgss2a", "RPG Maker VX"), ("rgss3a", "RPG Maker VX Ace"), ("wolf", "Wolf RPG"),
    ("nsa", "NScripter"), ("pck", "Godot"), ("asar", "Electron"), ("aqp", "AdvSys/ARCG"),
    ("arc", "Ethornell/Majiro（同名 .arc，需看 exe 判断）"),
    ("pfs", "Ethornell/BGI"), ("pfs2", "Artemis"),
    ("mjo", "Majiro"), ("dpm", "Donut"),
    ("ypf", "YU-RIS"), ("int", "CatSystem2"), ("ldb", "RPG Maker 2000/2003"),
    ("pak", "Siglus/Unreal/通用"), ("utoc", "Unreal Engine 5"), ("ucas", "Unreal Engine 5"),
    ("ald", "AliceSoft"), ("npk", "Nitroplus"), ("npa", "Nitroplus"),
    ("prt", "LiveMaker"), ("grp", "LiveMaker"), ("love", "LÖVE/Love2D"),
    ("lpk", "Purple"), ("ypf2", "YU-RIS"),
];

pub struct GenericPlugin;

impl Engine for GenericPlugin {
    fn id(&self) -> &'static str {
        "generic"
    }
    fn name(&self) -> &'static str {
        "未知引擎（兜底）"
    }
    fn priority(&self) -> i32 {
        -100
    }
    fn detect_scan(&self, _scan: &ScanCtx) -> Detection {
        let mut d = Detection::new(self.id(), self.name());
        d.hit(0, "未匹配到任何已收录的引擎特征");
        d.note("请在设置页配置 GARbro 后点“解包”，或参照“未知格式排查顺序”人工处理");
        d
    }
    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract]
    }
    fn describe(&self, root: &Path) -> String {
        let scan = ScanCtx::build(root);
        let mut hits: Vec<String> = Vec::new();
        for (ext, count) in scan.top_exts(40) {
            if let Some((_, eng)) = KNOWN_EXT.iter().find(|(k, _)| *k == ext) {
                if !hits.iter().any(|h| h.starts_with(eng.split('（').next().unwrap_or(eng))) {
                    hits.push(format!("{eng}(.{ext} × {count})"));
                }
            }
            if hits.len() >= 6 {
                break;
            }
        }
        let head = scan.summary();
        if hits.is_empty() {
            format!("{head}；未发现特征封包，建议用 GARbro 逐个尝试")
        } else {
            format!("{head}；疑似: {}", hits.join("; "))
        }
    }
    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        garbro_extract(ctx)
    }
}

/// GARbro 委派解包（兜底插件与盲区识别器共用）。
pub fn garbro_extract(ctx: &Ctx) -> OpOutcome {
    let cfg = settings::load();
    match settings::external_tool(&cfg, "garbro") {
        Some(garbro) => match Command::new(garbro)
            .args([ctx.root.to_string_lossy().as_ref(), "-o", ctx.out_dir.to_string_lossy().as_ref()])
            .output()
        {
            Ok(o) if o.status.success() => OpOutcome::ok(format!("GARbro 批量提取完成 → {}", ctx.out_dir.display())),
            Ok(o) => {
                let msg = if o.stderr.is_empty() { o.stdout } else { o.stderr };
                OpOutcome::fail(format!("GARbro 失败: {}", String::from_utf8_lossy(&msg)))
            }
            Err(e) => OpOutcome::fail(format!("GARbro 调用失败: {e}")),
        },
        None => OpOutcome::fail(
            "该格式没有内置解析器：请在设置页配置 GARbro CLI 路径后重试，或参照“未知格式排查顺序”人工处理",
        ),
    }
}
