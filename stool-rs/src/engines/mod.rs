//! 引擎插件层：Engine trait + 注册表 + 检测评分。

pub mod external;
pub mod generic;
pub mod others;
pub mod recognize;
pub mod renpy;
pub mod scan;

pub use others::{GodotPlugin, HtmlGamePlugin, KirikiriPlugin, NscripterPlugin, RpgMakerMvPlugin, RpgMakerRgssPlugin, TyranoPlugin};
pub use scan::ScanCtx;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// 判定线：`score >= DETECT_LINE` 才算"确认识别"。低于此线只作疑似展示。
pub const DETECT_LINE: i32 = 60;

/// 识别置信度（由分数换算，供 GUI 直接展示，避免各界自己写阈值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Confidence {
    /// ≥ 85：特征齐全，基本不会错
    High,
    /// 60–84：确认识别
    Medium,
    /// 30–59：疑似，证据不足
    Low,
    /// < 30：几乎无证据
    None,
}

impl Confidence {
    pub fn label(&self) -> &'static str {
        match self {
            Confidence::High => "高置信",
            Confidence::Medium => "已确认",
            Confidence::Low => "疑似（证据不足）",
            Confidence::None => "未识别",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Detection {
    pub plugin_id: String,
    pub name: String,
    pub score: i32,
    pub evidence: Vec<String>,
    pub notes: String,
}

impl Detection {
    pub fn new(plugin_id: &str, name: &str) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            name: name.into(),
            score: 0,
            evidence: Vec::new(),
            notes: String::new(),
        }
    }

    /// 命中一条判据：加分 + 记录证据。
    pub fn hit(&mut self, pts: i32, why: impl Into<String>) {
        self.score += pts;
        self.evidence.push(why.into());
    }

    /// 追加备注（多条用"；"连接）。
    pub fn note(&mut self, s: impl Into<String>) {
        let s = s.into();
        if s.is_empty() {
            return;
        }
        if !self.notes.is_empty() {
            self.notes.push('；');
        }
        self.notes.push_str(&s);
    }

    pub fn ok(&self) -> bool {
        self.score >= DETECT_LINE
    }

    pub fn confidence(&self) -> Confidence {
        match self.score {
            s if s >= 85 => Confidence::High,
            s if s >= DETECT_LINE => Confidence::Medium,
            s if s >= 30 => Confidence::Low,
            _ => Confidence::None,
        }
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
    /// 检测入口：只从共享扫描结果里判断，**不得**再自己遍历目录
    /// （批量检测走 `Registry::detect_all`，整棵树只扫一次）。
    fn detect_scan(&self, scan: &ScanCtx) -> Detection;

    /// 便捷包装：单独查一个目录时用（会临时构建一次扫描）。
    /// 批量检测请走 `Registry::detect_all` 以复用同一次扫描。
    fn detect(&self, root: &Path) -> Detection {
        self.detect_scan(&ScanCtx::build(root))
    }

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
    ///
    /// 只对"文本在 JS/DOM/脚本层、引擎留了运行时替换入口"的引擎可用；
    /// 具体范围与适配方式见 `crate::features::inject::SUPPORT_TABLE`。
    fn text_inject(&self, _ctx: &Ctx, _json_path: &Path) -> OpOutcome {
        OpOutcome::fail(format!(
            "「{}」不支持运行时 JSON 注入：它的文本封在私有封包/编译脚本里，没有稳定的运行时替换入口。\
             当前注入范围：Ren'Py、RPG Maker MV/MZ、TyranoBuilder、HTML/Electron。\
             该引擎请走「解包 → 文本提取 → 翻译 → 翻译回填/封包回写」这套离线流程。",
            self.name()
        ))
    }
    fn save(&self, _ctx: &Ctx) -> OpOutcome {
        OpOutcome::fail("未实现")
    }

    /// 该引擎的解锁手段（声明式，默认查 [`crate::features::unlock::UNLOCK_CATALOG`]）。
    /// 未收录的引擎会落到兜底策略（自带存档 + 人工），所以这里几乎不会为空。
    fn unlock_routes(&self) -> &'static [crate::features::unlock::UnlockRoute] {
        crate::features::unlock::routes_for(self.id())
    }

    /// 引擎**私有**的解锁实现：统一调度器 [`crate::features::unlock::run`] 按路线分派。
    /// 返回 `None` 表示该手段无私有实现（调度器据此走通用路线或给出指引）。
    ///
    /// 与 `extract`/`save` 等一样，实现里**不得**自行遍历整棵目录树；
    /// 需要目录信息请用 `ScanCtx`。
    fn unlock_impl(
        &self,
        _route: crate::features::unlock::UnlockRoute,
        _ctx: &Ctx,
    ) -> Option<OpOutcome> {
        None
    }

    /// 全 CG / 画廊解锁：统一走 [`crate::features::unlock::run`]，
    /// 由策略表选路线、`unlock_impl` 提供引擎私有实现。
    /// 缺省**只读预览**（安全默认），`--opt:apply=1` 才落地。
    fn unlock(&self, ctx: &Ctx) -> OpOutcome {
        crate::features::unlock::run(self.id(), ctx, |r| self.unlock_impl(r, ctx))
    }
}

/// 引擎对外能力 = 插件声明 ∪ 策略表推导（凡有解锁路线即具备 `Op::Unlock`）。
///
/// 这样新增引擎只需在 [`crate::features::unlock::UNLOCK_CATALOG`] 里加一行，
/// 无需再逐个改 `capabilities()`；GUI/CLI 统一走这里取能力。
pub fn effective_capabilities(e: &dyn Engine) -> Vec<Op> {
    let mut v = e.capabilities();
    if !e.unlock_routes().is_empty() && !v.contains(&Op::Unlock) {
        v.push(Op::Unlock);
    }
    v
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
        let mut engines: Vec<Box<dyn Engine>> = vec![
            Box::new(renpy::RenpyPlugin),
            Box::new(others::RpgMakerMvPlugin),
            Box::new(others::RpgMakerRgssPlugin),
            Box::new(others::KirikiriPlugin),
            Box::new(others::GodotPlugin),
            Box::new(others::NscripterPlugin),
            Box::new(others::TyranoPlugin),
            Box::new(others::HtmlGamePlugin),
            Box::new(external::WolfPlugin),
            Box::new(external::UnityPlugin),
        ];
        // 盲区引擎识别器（表驱动，优先级统一低于正式引擎）
        for r in recognize::RECOGNIZERS {
            engines.push(Box::new(recognize::Recognizer(r)));
        }
        engines.push(Box::new(generic::GenericPlugin));
        Registry { engines }
    }

    /// 全量检测：整棵目录树**只扫一次**，结果按 (分数, 优先级) 降序。
    pub fn detect_all(&self, root: &Path) -> Vec<Detection> {
        self.detect_with_scan(&ScanCtx::build(root))
    }

    /// 用已有扫描结果检测（多个游戏目录复用同一扫描时更快）。
    pub fn detect_with_scan(&self, scan: &ScanCtx) -> Vec<Detection> {
        let mut results: Vec<Detection> = self
            .engines
            .iter()
            .map(|e| {
                let mut d = e.detect_scan(scan);
                // 统一以 trait 的 id/name 为准，避免插件自己写错
                d.plugin_id = e.id().to_string();
                d.name = e.name().to_string();
                d
            })
            .collect();

        // 有任何一个正经引擎拿到证据时，就不必再展示"未知引擎（兜底）"占位行
        let any_evidence = results.iter().any(|d| d.score > 0 && d.plugin_id != "generic");
        if any_evidence {
            results.retain(|d| d.plugin_id != "generic");
        }

        // 0 分的引擎对用户没有任何信息量（几十个引擎全列出来只会淹没有效结果）。
        // 只保留"有分"的 + 全都没命中时的兜底插件。
        results.retain(|d| d.score > 0 || d.plugin_id == "generic");

        // 近失提示：差一点过线的插件，明确告诉用户"差什么"，而不是只显示低分
        for d in results.iter_mut() {
            if !d.ok() && d.score >= 30 {
                d.note(format!(
                    "差 {} 分未达判定线（{DETECT_LINE} 分）：可能缺文件、已被解包或改过名",
                    DETECT_LINE - d.score
                ));
            }
        }

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

    /// 最高分的"已确认"引擎。
    pub fn best(&self, root: &Path) -> Option<Detection> {
        self.detect_all(root).into_iter().find(|d| d.ok())
    }

    /// 从**已算好**的检测结果里选一个给 UI/CLI 用：返回 `(结果, 是否已确认)`。
    /// 结果需已按 (分数, 优先级) 降序 —— `detect_all` 的输出即满足。
    ///
    /// 与 `pick` 的区别：这里**不重新扫描目录**，适合"已经拿到 `detect_all` 结果"的调用方。
    pub fn pick_from(results: &[Detection]) -> Option<(Detection, bool)> {
        results.first().map(|d| (d.clone(), d.ok()))
    }

    /// 便捷包装：单独给一个目录时先检测、再选一个（会构建一次扫描）。
    pub fn pick(&self, root: &Path) -> Option<(Detection, bool)> {
        Self::pick_from(&self.detect_all(root))
    }

    pub fn get(&self, id: &str) -> Option<&dyn Engine> {
        self.engines.iter().find(|e| e.id() == id).map(|e| e.as_ref())
    }

    /// 每个插件对外的能力矩阵（GUI/CLI 用来解释"这个引擎能做什么"）。
    pub fn capability_matrix(&self) -> Vec<(String, String, Vec<Op>)> {
        let mut v: Vec<(String, String, Vec<Op>)> = self
            .engines
            .iter()
            .map(|e| (e.id().to_string(), e.name().to_string(), effective_capabilities(e.as_ref())))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }
}

thread_local! {
    /// 已建好的父目录缓存（P2-2）：解包动辄几千上万条目，但目录只有几十个，
    /// `create_dir_all` 每次都要逐级 stat。缓存后同一目录只建一次。
    /// 每个线程各存一份（并行解包时各线程互不干扰）。
    static DIR_CACHE: RefCell<HashSet<PathBuf>> = RefCell::new(HashSet::new());
}

/// 清空「已建父目录」缓存。
///
/// 缓存只在**同一次操作**内有效：操作开始时调用一次，避免用户中途删掉输出目录后
/// 缓存仍以为目录存在而导致写盘失败。（cli/gui 的 op 分发处各调一次。）
pub fn clear_dir_cache() {
    DIR_CACHE.with(|c| {
        if let Ok(mut s) = c.try_borrow_mut() {
            s.clear();
        }
    });
}

/// 安全输出路径：防路径穿越 + 自动建父目录（带目录缓存）。
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
        // `insert` 返回 true 表示「此前不在缓存」= 首次遇到该目录，才真正建。
        let first = DIR_CACHE.with(|c| c.borrow_mut().insert(parent.to_path_buf()));
        if first {
            if let Err(e) = std::fs::create_dir_all(parent) {
                // 建失败就把缓存摘掉，下次重试，避免永久跳过导致全部写失败。
                DIR_CACHE.with(|c| {
                    let _ = c.borrow_mut().remove(parent);
                });
                crate::diag::log("WARN", &format!("建目录失败 {}: {e}", parent.display()));
            }
        }
    }
    path
}

/// 解包/解密循环中的落盘助手：写出文件，失败时记录日志并计入 `failed`。
///
/// 相比原来的 `let _ = fs::write(...)`，这里**不会静默吞掉**写失败——
/// 磁盘满、路径过长、占用等都会落到日志，并在任务结果里以「N 个失败」呈现，
/// 避免用户以为解包成功但文件实际缺失。
/// 返回 `true` 表示写入成功。
pub fn write_out(out_dir: &Path, rel: &str, bytes: &[u8], failed: &mut usize) -> bool {
    let path = safe_out_path(out_dir, rel);
    match std::fs::write(&path, bytes) {
        Ok(()) => true,
        Err(e) => {
            *failed += 1;
            crate::diag::log("WARN", &format!("写出失败 {}: {e}", path.display()));
            false
        }
    }
}

/// 在解包结果消息里追加「失败数」提示（0 时不追加）。
pub fn with_fail_note(msg: String, failed: usize) -> String {
    if failed == 0 {
        msg
    } else {
        format!("{msg}（另有 {failed} 个文件写入失败，详见日志）")
    }
}

// ---------------------------------------------------------------------------
// 解包断点续传（P2-4）
// ---------------------------------------------------------------------------

/// 台账落盘间隔（条）。逐条写 JSON 在 8000+ 条目时太慢，攒够再写。
const RESUME_FLUSH_EVERY: usize = 64;

/// 解包断点续传台账（P2-4）。
///
/// 大封包解包动辄几万条目、几个 GB，中途取消 / 崩溃 / 磁盘写满都很常见。
/// 台账记录「已成功写出的条目 → 大小」，重跑同一操作时：
/// - 已记录且产物仍在（大小一致）→ **跳过**（不再重复解压与写盘）；
/// - 跑完且无写失败 → 删除台账；否则保留，供下次续传。
///
/// 台账位于输出目录：`<out>/.stool_resume_<op>.json`；`root` 不一致即视为全新任务。
/// `--opt:resume=0` 可关闭（忽略旧台账、从零重写）。
pub struct Resume {
    ledger: PathBuf,
    op: String,
    root: String,
    done: BTreeMap<String, u64>,
    enabled: bool,
    dirty: bool,
    since_flush: usize,
    /// 本次因命中断点而跳过的条目数。
    pub skipped: usize,
}

impl Resume {
    /// 打开（或新建）台账。旧台账 op/root 不匹配或解析失败 → 从零开始。
    pub fn open(out_dir: &Path, op: &str, root: &Path) -> Resume {
        let root_s = root.to_string_lossy().into_owned();
        let ledger = out_dir.join(format!(".stool_resume_{op}.json"));
        let mut done: BTreeMap<String, u64> = BTreeMap::new();
        if let Ok(txt) = std::fs::read_to_string(&ledger) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                let same = v.get("op").and_then(|o| o.as_str()) == Some(op)
                    && v.get("root").and_then(|r| r.as_str()) == Some(root_s.as_str());
                if same {
                    if let Some(map) = v.get("done").and_then(|d| d.as_object()) {
                        for (k, val) in map {
                            if let Some(n) = val.as_u64() {
                                done.insert(k.clone(), n);
                            }
                        }
                    }
                }
            }
        }
        Resume {
            ledger,
            op: op.to_string(),
            root: root_s,
            done,
            enabled: true,
            dirty: false,
            since_flush: 0,
            skipped: 0,
        }
    }

    /// 从 `ctx.options` 读取开关：`resume=0` 关闭续传。
    pub fn configured(mut self, ctx: &Ctx) -> Self {
        if matches!(ctx.opt("resume"), Some("0") | Some("off") | Some("false")) {
            self.enabled = false;
        }
        self
    }

    /// 续传是否生效。
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 已记录且产物仍在（大小一致）→ true（并计入 `skipped`）。
    ///
    /// 大小校验很关键：用户中途删过产物 / 换过文件时，不能盲信旧台账。
    pub fn already_done(&mut self, out_dir: &Path, rel: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let Some(&sz) = self.done.get(rel) else { return false };
        let ok = std::fs::metadata(safe_out_path(out_dir, rel)).map(|m| m.len() == sz).unwrap_or(false);
        if ok {
            self.skipped += 1;
        }
        ok
    }

    /// 登记一条已写出的条目。
    fn mark(&mut self, rel: &str, size: u64) {
        if !self.enabled {
            return;
        }
        self.done.insert(rel.to_string(), size);
        self.dirty = true;
        self.since_flush += 1;
        if self.since_flush >= RESUME_FLUSH_EVERY {
            self.flush();
        }
    }

    /// 立即落盘台账（取消 / 每 64 条自动调用）。
    pub fn flush(&mut self) {
        if !self.enabled || !self.dirty {
            return;
        }
        let mut obj = serde_json::Map::new();
        for (k, v) in &self.done {
            obj.insert(k.clone(), serde_json::Value::from(*v));
        }
        let body = serde_json::json!({
            "version": 1,
            "op": self.op,
            "root": self.root,
            "done": serde_json::Value::Object(obj),
        });
        crate::settings::ensure_parent(&self.ledger);
        match serde_json::to_string(&body).map_err(|e| e.to_string()).and_then(|s| {
            std::fs::write(&self.ledger, s).map_err(|e| e.to_string())
        }) {
            Ok(()) => {
                self.dirty = false;
                self.since_flush = 0;
            }
            Err(e) => crate::diag::log("WARN", &format!("断点台账写入失败 {}: {e}", self.ledger.display())),
        }
    }

    /// 结束本次操作。`complete=true`（无写失败）→ 删除台账；否则保留供续传。
    /// 返回本次跳过的条目数。
    pub fn finish(mut self, complete: bool) -> usize {
        if !self.enabled {
            return self.skipped;
        }
        if complete {
            let _ = std::fs::remove_file(&self.ledger);
        } else {
            self.flush();
            crate::diag::log(
                "INFO",
                &format!("解包未完成，已记录断点 {} 条：{}", self.done.len(), self.ledger.display()),
            );
        }
        self.skipped
    }
}

/// 断点续传版落盘：已完成且产物仍在 → 跳过；否则写出并登记。
pub fn write_out_resume(
    out_dir: &Path,
    rel: &str,
    bytes: &[u8],
    failed: &mut usize,
    resume: &mut Resume,
) -> bool {
    if resume.already_done(out_dir, rel) {
        return true;
    }
    let ok = write_out(out_dir, rel, bytes, failed);
    if ok {
        resume.mark(rel, bytes.len() as u64);
    }
    ok
}

/// 结果消息里追加续传提示（跳过 0 时不追加）。
pub fn with_resume_note(msg: String, skipped: usize) -> String {
    if skipped == 0 {
        msg
    } else {
        format!("{msg}（其中 {skipped} 个沿用上次结果，断点续传）")
    }
}

// ---------------------------------------------------------------------------
// 解包并行化（P2-2）
// ---------------------------------------------------------------------------

/// 并行解包工作单元：一个「相对输出路径 + 该条目的读取信息」。
///
/// 泛型 `T` 承载各格式自己的条目类型（`Xp3Entry` / `PckEntry` / `V3Entry` / `AsarNode` …），
/// 于是五种格式共用同一份并行逻辑。
pub struct Job<T> {
    /// 相对输出路径（`safe_out_path` 的 key，也是断点台账的 key）。
    pub rel: String,
    /// 读取该条目所需的信息（偏移/大小/密钥，或条目对象本身）。
    pub item: T,
}

/// 并行度：`--opt:jobs=N` 指定（≥1），否则取 CPU 并行度。
pub fn worker_count(ctx: &Ctx) -> usize {
    if let Some(n) = ctx.opt("jobs").and_then(|s| s.trim().parse::<usize>().ok()) {
        if n >= 1 {
            return n;
        }
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// 并行解包：多线程「读条目 → 解码 → 写盘」。
///
/// - 每个工作线程用 `mk_reader()` 各自建一份读取句柄（如 `Source::open`），
///   按原子游标抢条目——天然 work-stealing，无需锁。
/// - 结果经 mpsc 回到**主线程**统一处理：报进度、登记断点台账、统计失败数
///   （`Resume` / `Ctx` 都不是 `Sync`，放主线程最省心）。
/// - 取消：主线程发现 `ctx.cancelled()` 就置 `abort`，各线程在条目边界退出。
/// - `read` 为纯解码：`Fn(&mut S, &T) -> Result<Vec<u8>, String>`，需 `Sync`。
///
/// 返回 `(写成功数, 失败数)`。跳过（断点续传）的条目由调用方在过滤阶段统计，
/// **不**进入本函数。
pub fn parallel_extract<T, S, M, R>(
    jobs: &[Job<T>],
    out_dir: &Path,
    workers: usize,
    ctx: &Ctx,
    mk_reader: M,
    read: R,
    resume: &mut Resume,
) -> (usize, usize)
where
    T: Sync,
    M: Fn() -> Result<S, String> + Sync,
    R: Fn(&mut S, &T) -> Result<Vec<u8>, String> + Sync,
{
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;

    let total = jobs.len();
    if total == 0 {
        return (0, 0);
    }
    let workers = workers.max(1).min(total);
    let cursor = AtomicUsize::new(0);
    let abort = AtomicBool::new(false);
    // (rel, 写出字节数, 错误信息)
    let (tx, rx) = mpsc::channel::<(String, u64, Option<String>)>();
    let mk_reader = &mk_reader;
    let read = &read;

    let mut written = 0usize;
    let mut failed = 0usize;

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            let cursor = &cursor;
            let abort = &abort;
            scope.spawn(move || {
                let mut rd = match mk_reader() {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send((String::new(), 0, Some(e)));
                        return;
                    }
                };
                loop {
                    if abort.load(Ordering::Relaxed) {
                        break;
                    }
                    let i = cursor.fetch_add(1, Ordering::Relaxed);
                    if i >= total {
                        break;
                    }
                    let job = &jobs[i];
                    match read(&mut rd, &job.item) {
                        Ok(blob) => {
                            let path = safe_out_path(out_dir, &job.rel);
                            match std::fs::write(&path, &blob) {
                                Ok(()) => {
                                    let _ = tx.send((job.rel.clone(), blob.len() as u64, None));
                                }
                                Err(e) => {
                                    crate::diag::log("WARN", &format!("写出失败 {}: {e}", path.display()));
                                    let _ = tx.send((job.rel.clone(), 0, Some(format!("写出失败 {e}"))));
                                }
                            }
                        }
                        Err(e) => {
                            crate::diag::log("WARN", &format!("读取失败 {}: {e}", job.rel));
                            let _ = tx.send((job.rel.clone(), 0, Some(e)));
                        }
                    }
                }
            });
        }
        // 主线程自己这份发送端必须丢掉，否则 recv 永远等不到 Disconnected。
        drop(tx);
        let mut seen = 0usize;
        loop {
            if ctx.cancelled() {
                abort.store(true, Ordering::Relaxed);
            }
            match rx.recv_timeout(std::time::Duration::from_millis(150)) {
                Ok((rel, size, err)) => {
                    seen += 1;
                    match err {
                        None => {
                            written += 1;
                            resume.mark(&rel, size);
                        }
                        Some(_) => failed += 1,
                    }
                    let label = if rel.is_empty() { "(读取句柄打开失败)" } else { rel.as_str() };
                    ctx.report(seen as f32 / total as f32, label);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    (written, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_from_reports_confidence_without_rescanning() {
        let mut hi = Detection::new("unity", "Unity");
        hi.hit(80, "UnityPlayer.dll");
        let mut lo = Detection::new("generic", "未知");
        lo.hit(0, "无特征");
        let results = vec![hi, lo];

        let (d, ok) = Registry::pick_from(&results).unwrap();
        assert_eq!(d.plugin_id, "unity", "应取第一个（已按分数降序）");
        assert!(ok, "80 分应判为已确认");

        // 未达判定线 → ok = false（让 UI 显示"疑似"而不是静默当成功）
        let mut sus = Detection::new("x", "X");
        sus.hit(40, "弱特征");
        let (d2, ok2) = Registry::pick_from(&[sus]).unwrap();
        assert_eq!(d2.score, 40);
        assert!(!ok2);

        assert!(Registry::pick_from(&[]).is_none());
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("stool_resume_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn resume_skips_completed_and_removes_ledger_on_success() {
        let d = tmp("basic");
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let game = d.join("game");
        std::fs::create_dir_all(&game).unwrap();

        {
            let mut r = Resume::open(&out, "extract", &game);
            let mut failed = 0usize;
            assert!(write_out_resume(&out, "a/b.txt", b"hello", &mut failed, &mut r));
            assert!(write_out_resume(&out, "c.bin", b"12345", &mut failed, &mut r));
            assert_eq!(failed, 0);
            assert_eq!(r.finish(true), 0, "首次无跳过");
        }
        assert!(!out.join(".stool_resume_extract.json").exists(), "完成后台账应删除");

        // 第二次：模拟中断前已写 1 个，重跑应跳过它
        {
            let mut r = Resume::open(&out, "extract", &game);
            let mut failed = 0usize;
            assert!(write_out_resume(&out, "a/b.txt", b"hello", &mut failed, &mut r));
            let _ = r.finish(false); // 未完成 → 保留台账
        }
        assert!(out.join(".stool_resume_extract.json").exists());

        {
            let mut r = Resume::open(&out, "extract", &game);
            assert!(r.already_done(&out, "a/b.txt"), "已写且大小一致 → 应跳过");
            assert!(!r.already_done(&out, "never.txt"), "没写过的不应跳过");
            assert_eq!(r.skipped, 1);
            let _ = r.finish(true);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resume_ignores_ledger_for_other_root() {
        let d = tmp("root");
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let g1 = d.join("g1");
        let g2 = d.join("g2");
        std::fs::create_dir_all(&g1).unwrap();
        std::fs::create_dir_all(&g2).unwrap();
        {
            let mut r = Resume::open(&out, "extract", &g1);
            let mut failed = 0usize;
            write_out_resume(&out, "x.txt", b"xx", &mut failed, &mut r);
            let _ = r.finish(false);
        }
        // 换了一个游戏目录 → 旧台账必须作废
        let mut r = Resume::open(&out, "extract", &g2);
        assert!(!r.already_done(&out, "x.txt"));
        assert_eq!(r.skipped, 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resume_rechecks_file_size() {
        let d = tmp("size");
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let game = d.join("game");
        std::fs::create_dir_all(&game).unwrap();
        {
            let mut r = Resume::open(&out, "extract", &game);
            let mut failed = 0usize;
            write_out_resume(&out, "f.bin", b"12345678", &mut failed, &mut r);
            let _ = r.finish(false);
        }
        // 用户中途改动了产物 → 大小不符，不能当作已完成
        std::fs::write(out.join("f.bin"), b"short").unwrap();
        let mut r = Resume::open(&out, "extract", &game);
        assert!(!r.already_done(&out, "f.bin"), "大小不符必须重做");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resume_disabled_never_skips() {
        let d = tmp("off");
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("f.txt"), b"abc").unwrap();
        let game = d.join("game");
        std::fs::create_dir_all(&game).unwrap();
        let opts: HashMap<String, String> = [("resume".to_string(), "0".to_string())].into_iter().collect();
        let cancel = AtomicBool::new(false);
        let prog = |_: f32, _: &str| {};
        let ctx = Ctx { root: &game, out_dir: &out, options: &opts, progress: &prog, cancel: &cancel };
        let mut r = Resume::open(&out, "extract", &game);
        // 手动登记一条后关闭续传
        r.enabled = false;
        assert!(!r.already_done(&out, "f.txt"));
        // configured() 也会按选项关闭
        let r2 = Resume::open(&out, "extract", &game).configured(&ctx);
        assert!(!r2.enabled());
        let _ = std::fs::remove_dir_all(&d);
    }

    fn par_ctx<'a>(root: &'a Path, out: &'a Path, opts: &'a HashMap<String, String>, cancel: &'a AtomicBool, prog: &'a dyn Fn(f32, &str)) -> Ctx<'a> {
        Ctx { root, out_dir: out, options: opts, progress: prog, cancel }
    }

    #[test]
    fn parallel_extract_writes_all_and_marks_resume() {
        let d = tmp("par_ok");
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let game = d.join("game");
        std::fs::create_dir_all(&game).unwrap();
        let jobs: Vec<Job<usize>> = (0..200).map(|i| Job { rel: format!("d{}/f{i}.bin", i % 5), item: i }).collect();
        let opts = HashMap::new();
        let cancel = AtomicBool::new(false);
        let prog = |_: f32, _: &str| {};
        let ctx = par_ctx(&game, &out, &opts, &cancel, &prog);

        let mut r = Resume::open(&out, "extract", &game);
        let (w, f) = parallel_extract(
            &jobs,
            &out,
            4,
            &ctx,
            || Ok::<(), String>(()),
            |_s: &mut (), i: &usize| Ok(vec![(*i % 251) as u8; (*i % 13) + 1]),
            &mut r,
        );
        assert_eq!((w, f), (200, 0), "应全部写出、零失败");
        for j in &jobs {
            let bytes = std::fs::read(safe_out_path(&out, &j.rel)).unwrap();
            assert_eq!(bytes.len(), (j.item % 13) + 1, "{} 内容长度不符", j.rel);
            assert!(bytes.iter().all(|b| *b == (j.item % 251) as u8), "{} 内容不符", j.rel);
        }
        // 台账登记齐全（finish(false) 保留，重开可全部跳过）
        let _ = r.finish(false);
        let mut r2 = Resume::open(&out, "extract", &game);
        let skipped = jobs.iter().filter(|j| r2.already_done(&out, &j.rel)).count();
        assert_eq!(skipped, 200, "全部条目应可从台账恢复");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn parallel_extract_counts_read_failures() {
        let d = tmp("par_fail");
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let game = d.join("game");
        std::fs::create_dir_all(&game).unwrap();
        let jobs: Vec<Job<usize>> = (0..50).map(|i| Job { rel: format!("f{i}.bin"), item: i }).collect();
        let opts = HashMap::new();
        let cancel = AtomicBool::new(false);
        let prog = |_: f32, _: &str| {};
        let ctx = par_ctx(&game, &out, &opts, &cancel, &prog);

        let mut r = Resume::open(&out, "extract", &game);
        let (w, f) = parallel_extract(
            &jobs,
            &out,
            4,
            &ctx,
            || Ok::<(), String>(()),
            |_s: &mut (), i: &usize| {
                if i % 10 == 3 {
                    Err(format!("boom {i}"))
                } else {
                    Ok(vec![7u8; 4])
                }
            },
            &mut r,
        );
        assert_eq!(w, 45, "45 条成功");
        assert_eq!(f, 5, "5 条（i%10==3）读失败");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn parallel_extract_single_worker_matches_many() {
        let d = tmp("par_eq");
        let game = d.join("game");
        std::fs::create_dir_all(&game).unwrap();
        let jobs: Vec<Job<usize>> = (0..300).map(|i| Job { rel: format!("g{}/f{i}.bin", i % 11), item: i }).collect();
        let opts = HashMap::new();
        let cancel = AtomicBool::new(false);
        let prog = |_: f32, _: &str| {};
        let ctx = par_ctx(&game, &d, &opts, &cancel, &prog);

        let read = |_s: &mut (), i: &usize| Ok(vec![(*i % 199) as u8; (*i % 31) + 1]);
        let out1 = d.join("one");
        let out8 = d.join("many");
        let mut r1 = Resume::open(&out1, "extract", &game);
        let (w1, f1) = parallel_extract(&jobs, &out1, 1, &ctx, || Ok::<(), String>(()), read, &mut r1);
        let mut r8 = Resume::open(&out8, "extract", &game);
        let (w8, f8) = parallel_extract(&jobs, &out8, 8, &ctx, || Ok::<(), String>(()), read, &mut r8);
        assert_eq!((w1, f1), (300, 0));
        assert_eq!((w1, f1), (w8, f8));
        for j in &jobs {
            let a = std::fs::read(safe_out_path(&out1, &j.rel)).unwrap();
            let b = std::fs::read(safe_out_path(&out8, &j.rel)).unwrap();
            assert_eq!(a, b, "{} 单线程/多线程结果应一致", j.rel);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn dir_cache_skips_recreation_but_clear_recovers() {
        let d = tmp("dircache");
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        clear_dir_cache();
        let p = safe_out_path(&out, "a/b/c.txt");
        assert!(p.parent().unwrap().is_dir(), "首次应建好父目录");
        // 缓存命中：外部删掉目录后，再次调用**不会**重复 create_dir_all（体现"只建一次"）
        std::fs::remove_dir_all(out.join("a")).unwrap();
        let _ = safe_out_path(&out, "a/b/c.txt");
        assert!(!out.join("a").exists(), "缓存命中时不应重复建目录");
        // 清缓存后恢复重建（每次操作前会清，保证不会永久失联）
        clear_dir_cache();
        let _ = safe_out_path(&out, "a/b/c.txt");
        assert!(out.join("a/b").is_dir(), "清缓存后应重建父目录");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn effective_capabilities_adds_unlock_from_catalog() {
        let reg = Registry::new();
        // 原生插件里只有 Ren'Py / Unity 显式声明了 Unlock，其余靠策略表补
        for (id, _name, caps) in reg.capability_matrix() {
            assert!(
                caps.contains(&Op::Unlock),
                "{id} 应具备 Unlock（策略表已覆盖全部引擎）"
            );
        }
        // 未知 id 走兜底策略，也应具备 Unlock
        assert!(!crate::features::unlock::routes_for("__nope__").is_empty());
    }
}
