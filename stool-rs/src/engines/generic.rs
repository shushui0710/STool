//! 未知引擎兜底插件：特征提示 + GARbro 委派。

use std::path::Path;
use std::process::Command;

use super::{Ctx, Detection, Engine, Op, OpOutcome};
use crate::settings;

const KNOWN_EXT: &[(&str, &str)] = &[
    ("xp3", "KiriKiri"), ("rpa", "Ren'Py"), ("rgssad", "RPG Maker XP"),
    ("rgss2a", "RPG Maker VX"), ("rgss3a", "RPG Maker VX Ace"), ("wolf", "Wolf RPG"),
    ("nsa", "NScripter"), ("pck", "Godot"), ("asar", "Electron"), ("aqp", "AdvSys/ARCG"),
    ("pac", "通用封包"), ("arc", "通用封包"), ("dat", "通用数据"), ("pak", "通用封包"),
    ("pfs", "Ethornell/BGI"), ("mjo", "Mink"), ("dpm", "Donut"),
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
    fn detect(&self, _root: &Path) -> Detection {
        Detection {
            plugin_id: self.id().into(),
            name: self.name().into(),
            score: 10,
            evidence: vec!["未匹配到已知引擎特征".into()],
            notes: String::new(),
        }
    }
    fn capabilities(&self) -> Vec<Op> {
        vec![Op::Extract]
    }
    fn describe(&self, root: &Path) -> String {
        let mut hits: Vec<(String, String)> = Vec::new();
        for e in walkdir::WalkDir::new(root).into_iter().flatten() {
            if e.file_type().is_file() {
                if let Some(ext) = e.path().extension().and_then(|x| x.to_str()) {
                    let ext = ext.to_lowercase();
                    if let Some((_, eng)) = KNOWN_EXT.iter().find(|(k, _)| *k == ext) {
                        if !hits.iter().any(|(e, _)| e == eng) {
                            hits.push((eng.to_string(), e.file_name().to_string_lossy().into_owned()));
                        }
                    }
                }
            }
        }
        if hits.is_empty() {
            "未发现特征封包，建议用 GARbro 逐个尝试".into()
        } else {
            format!("疑似: {}", hits.iter().take(5).map(|(e, f)| format!("{e}({f})")).collect::<Vec<_>>().join("; "))
        }
    }
    fn extract(&self, ctx: &Ctx) -> OpOutcome {
        let cfg = settings::load();
        match settings::external_tool(&cfg, "garbro") {
            Some(garbro) => match Command::new(garbro)
                .args([ctx.root.to_string_lossy().as_ref(), "-o", ctx.out_dir.to_string_lossy().as_ref()])
                .output()
            {
                Ok(o) if o.status.success() => OpOutcome::ok(format!("GARbro 批量提取完成 → {}", ctx.out_dir.display())),
                Ok(o) => OpOutcome::fail(format!("GARbro 失败: {}", String::from_utf8_lossy(&o.stderr))),
                Err(e) => OpOutcome::fail(format!("GARbro 调用失败: {e}")),
            },
            None => OpOutcome::fail("未知格式：请在设置页配置 GARbro CLI 路径，或参照“未知格式排查顺序”人工处理"),
        }
    }
}
