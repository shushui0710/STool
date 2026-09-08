//! 引擎插件层：Engine trait + 注册表 + 检测评分。

pub mod external;
pub mod generic;
pub mod others;
pub mod renpy;

pub use others::{GodotPlugin, HtmlGamePlugin, KirikiriPlugin, NscripterPlugin, RpgMakerMvPlugin, RpgMakerRgssPlugin};

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Detection {
    pub plugin_id: String,
    pub name: String,
    pub score: i32,
    pub evidence: Vec<String>,
    pub notes: String,
}

impl Detection {
    pub fn ok(&self) -> bool {
        self.score >= 60
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OpOutcome {
    pub success: bool,
    pub message: String,
    pub files_done: usize,
    pub logs: Vec<String>,
}

impl OpOutcome {
    pub fn ok(msg: impl Into<String>) -> Self {
        Self { success: true, message: msg.into(), files_done: 0, logs: vec![] }
    }
    pub fn okn(msg: impl Into<String>, n: usize) -> Self {
        Self { success: true, message: msg.into(), files_done: n, logs: vec![] }
    }
    pub fn fail(msg: impl Into<String>) -> Self {
        Self { success: false, message: msg.into(), files_done: 0, logs: vec![] }
    }
}

/// 能力枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Extract,
    Repack,
    Decompile,
    TextExtract,
    TextImport,
    TextInject,
    Save,
    Unlock,
}

impl Op {
    pub fn label(&self) -> &'static str {
        match self {
            Op::Extract => "资源解包",
            Op::Repack => "封包/回写",
            Op::Decompile => "脚本反编译",
            Op::TextExtract => "文本提取",
            Op::TextImport => "翻译回填",
            Op::TextInject => "JSON 注入汉化",
            Op::Save => "存档编辑",
            Op::Unlock => "解锁辅助",
        }
    }
}

/// 执行上下文：进度回调 + 取消标志 + 选项。
pub struct Ctx<'a> {
    pub root: &'a Path,
    pub out_dir: &'a Path,
    pub options: &'a HashMap<String, String>,
    pub progress: &'a dyn Fn(f32, &str),
    pub cancel: &'a AtomicBool,
}

impl<'a> Ctx<'a> {
    pub fn report(&self, frac: f32, msg: &str) {
        (self.progress)(frac.clamp(0.0, 1.0), msg);
    }
    pub fn opt(&self, key: &str) -> Option<&str> {
        self.options.get(key).map(|s| s.as_str())
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }
}

pub trait Engine: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn priority(&self) -> i32 {
        0
    }
    fn detect(&self, root: &Path) -> Detection;
    fn capabilities(&self) -> Vec<Op> {
        vec![]
    }
    fn describe(&self, _root: &Path) -> String {
        String::new()
    }

    fn extract(&self, _ctx: &Ctx) -> OpOutcome {
        OpOutcome::fail("未实现")
    }
    fn repack(&self, _ctx: &Ctx, _src_dir: &Path) -> OpOutcome {
        OpOutcome::fail("未实现")
    }
    fn decompile(&self, _ctx: &Ctx) -> OpOutcome {
        OpOutcome::fail("未实现")
    }
    fn text_extract(&self, _ctx: &Ctx, _out_csv: &Path) -> OpOutcome {
        OpOutcome::fail("未实现")
    }
    fn text_import(&self, _ctx: &Ctx, _csv_path: &Path) -> OpOutcome {
        OpOutcome::fail("未实现")
    }
    /// JSON 注入汉化（运行时替换，不改游戏文件）。json_path 为翻译 JSON；
    /// 传不存在/空路径时回退到 <游戏目录>/translation.json。
    fn text_inject(&self, _ctx: &Ctx, _json_path: &Path) -> OpOutcome {
        OpOutcome::fail("该引擎不支持运行时 JSON 注入（当前支持：RPG Maker MV/MZ、Ren'Py、HTML/Electron）")
    }
    fn save(&self, _ctx: &Ctx) -> OpOutcome {
        OpOutcome::fail("未实现")
    }
    fn unlock(&self, _ctx: &Ctx) -> OpOutcome {
        OpOutcome::fail("未实现")
    }
}

pub struct Registry {
    pub engines: Vec<Box<dyn Engine>>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        let engines: Vec<Box<dyn Engine>> = vec![
            Box::new(renpy::RenpyPlugin),
            Box::new(others::RpgMakerMvPlugin),
            Box::new(others::RpgMakerRgssPlugin),
            Box::new(others::KirikiriPlugin),
            Box::new(others::GodotPlugin),
            Box::new(others::NscripterPlugin),
            Box::new(others::HtmlGamePlugin),
            Box::new(external::WolfPlugin),
            Box::new(external::UnityPlugin),
            Box::new(generic::GenericPlugin),
        ];
        Registry { engines }
    }

    pub fn detect_all(&self, root: &Path) -> Vec<Detection> {
        let mut results: Vec<Detection> = self
            .engines
            .iter()
            .map(|e| {
                let mut d = e.detect(root);
                d.plugin_id = e.id().to_string();
                d.name = e.name().to_string();
                d
            })
            .collect();
        results.sort_by_key(|d| {
            let prio = self
                .engines
                .iter()
                .find(|e| e.id() == d.plugin_id)
                .map(|e| e.priority())
                .unwrap_or(0);
            std::cmp::Reverse((d.score, prio))
        });
        results
    }

    pub fn best(&self, root: &Path) -> Option<Detection> {
        self.detect_all(root).into_iter().find(|d| d.ok())
    }

    pub fn get(&self, id: &str) -> Option<&dyn Engine> {
        self.engines.iter().find(|e| e.id() == id).map(|e| e.as_ref())
    }
}

/// 安全输出路径：防路径穿越 + 自动建父目录。
pub fn safe_out_path(out_dir: &Path, rel: &str) -> std::path::PathBuf {
    let norm = rel.replace('\\', "/");
    let parts: Vec<&str> = norm.split('/').filter(|p| !p.is_empty() && *p != "." && *p != "..").collect();
    let mut path = out_dir.to_path_buf();
    if parts.is_empty() {
        path.push("unnamed");
    } else {
        for p in parts {
            path.push(p);
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    path
}
