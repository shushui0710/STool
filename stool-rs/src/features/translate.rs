//! 机翻管线：翻译引擎插件 + 批量翻译（CSV / 注入 JSON），带断点续翻。
//!
//! 设计：
//! - `Translator` trait 是翻译引擎插件口：任何"文本批次 → 译文批次"的服务都能接入
//!   （OpenAI 兼容 API / 本地 Ollama / Sakura / 未来的 Argos 等）；
//! - `OpenAiCompat` 覆盖绝大多数免费/自有 Key 渠道：DeepSeek、智谱、Ollama（http://127.0.0.1:11434）、
//!   llama.cpp server 等，只要暴露 OpenAI 兼容的 /chat/completions 即可；
//! - 断点续翻：进度保存在旁车文件 `<目标>.mtl.json`（键=原文，值=译文），
//!   中断/失败后重跑自动跳过已完成条目；全部完成后旁车文件删除；
//! - 未翻条目判定：CSV 的 translation 列为空 / JSON 的值为空串。已有译文一律保留。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use serde_json::json;

/// 翻译引擎插件口。
pub trait Translator: Send + Sync {
    fn name(&self) -> &str;
    /// 批量翻译：输入原文数组，返回**等长**的译文数组。
    fn translate_batch(&self, texts: &[String]) -> Result<Vec<String>, String>;
}

const SYS_PROMPT: &str = "你是游戏文本翻译引擎。用户会发来一个 JSON 字符串数组，请把其中每一条游戏文本（日文/英文等）翻译成简体中文。要求：口语化、符合游戏语境；保留文本中的控制标签（如 [color]...[/color]、\\C[3]、{w} 等代码与转义）；保留 %s/%d/{0} 等占位符；同一术语全文译法一致。只输出一个 JSON 字符串数组，顺序与输入一致，不要输出任何解释或代码块围栏。";

/// OpenAI 兼容机翻引擎（DeepSeek / 智谱 / Ollama / llama.cpp / vLLM 等通用）。
pub struct OpenAiCompat {
    pub base_url: String, // 如 https://api.deepseek.com 或 http://127.0.0.1:11434/v1
    pub api_key: String,  // 本地模型可为空
    pub model: String,
    /// 术语表：每行一条 `原文=译文`，追加进 system prompt 强制统一译名（可为空）
    pub glossary: String,
}

impl OpenAiCompat {
    fn endpoint(&self) -> String {
        let b = self.base_url.trim().trim_end_matches('/');
        if b.ends_with("/chat/completions") {
            b.to_string()
        } else {
            format!("{b}/chat/completions")
        }
    }

    /// 组装 system prompt：基础指令 + 术语表（如有）。
    fn sys_prompt(&self) -> String {
        let g = self.glossary.trim();
        if g.is_empty() {
            SYS_PROMPT.to_string()
        } else {
            format!(
                "{SYS_PROMPT}\n\n以下是本游戏的术语表（人名/地名/专有名词），必须逐条严格遵守， \
                 不得使用表外译法：\n{g}"
            )
        }
    }
}

impl Translator for OpenAiCompat {
    fn name(&self) -> &str {
        "openai-compat"
    }
    fn translate_batch(&self, texts: &[String]) -> Result<Vec<String>, String> {
        let mut b = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(600))
            .user_agent("STool/0.1 (machine translate)");
        // 与 tools_dl 相同：ureq 2.12 需显式接线 native-tls
        b = b.tls_connector(std::sync::Arc::new(
            ureq::native_tls::TlsConnector::new().map_err(|e| format!("TLS 初始化失败: {e}"))?,
        ));
        let body = json!({
            "model": self.model,
            "temperature": 0.2,
            "messages": [
                {"role": "system", "content": self.sys_prompt()},
                {"role": "user", "content": serde_json::to_string(texts).map_err(|e| e.to_string())?}
            ],
            "stream": false,
        });
        let mut req = b
            .build()
            .post(&self.endpoint())
            .set("Content-Type", "application/json");
        if !self.api_key.trim().is_empty() {
            req = req.set("Authorization", &format!("Bearer {}", self.api_key.trim()));
        }
        let resp: serde_json::Value = req
            .send_string(&body.to_string())
            .map_err(|e| match e {
                // 把 HTTP 状态码写进错误串，供重试判定区分「限流/服务端临时故障」与「客户端错」
                ureq::Error::Status(code, r) => {
                    let body = r.into_string().unwrap_or_default();
                    let snippet: String = body.chars().take(300).collect();
                    format!("HTTP {code}：{snippet}")
                }
                other => format!("网络错误: {other}"),
            })?
            .into_json()
            .map_err(|e| format!("机翻响应不是 JSON: {e}"))?;
        if let Some(msg) = resp.get("error") {
            return Err(format!("机翻服务报错: {msg}"));
        }
        let content = resp["choices"][0]["message"]["content"]
            .as_str()
            .ok_or("机翻响应缺少 choices[0].message.content")?;
        // 剥掉模型可能自作主张加的 ```json 围栏
        let c = content.trim();
        let c = c
            .strip_prefix("```json")
            .or_else(|| c.strip_prefix("```"))
            .unwrap_or(c)
            .strip_suffix("```")
            .unwrap_or(c)
            .trim();
        let arr: Vec<String> = serde_json::from_str(c)
            .map_err(|e| format!("机翻输出不是 JSON 数组: {e}\n原始输出前 500 字: {}", c.chars().take(500).collect::<String>()))?;
        if arr.len() != texts.len() {
            return Err(format!("机翻返回条数不匹配：{} != {}", arr.len(), texts.len()));
        }
        Ok(arr)
    }
}

// ---------------------------------------------------------------------------
// 重试退避 + 并发批次（P2-3）
// ---------------------------------------------------------------------------

/// 每批最多尝试次数（1 次首发 + 3 次重试）。
pub const RETRY_ATTEMPTS: usize = 4;

/// 错误是否值得重试：限流 / 服务端临时故障 / 网络抖动。
///
/// `OpenAiCompat` 会把 HTTP 状态码写进错误串（如 `HTTP 429：...`），据此判定；
/// 客户端错（400/401/403/404 等）立即失败，不浪费重试。
fn looks_retryable(msg: &str) -> bool {
    const HTTP_KEYS: &[&str] = &["http 408", "http 425", "http 429", "http 500", "http 502", "http 503", "http 504"];
    const NET_KEYS: &[&str] = &[
        "rate limit",
        "too many requests",
        "timed out",
        "timeout",
        "网络错误",
        "connection",
        "reset by peer",
        "broken pipe",
        "unexpected eof",
        "temporarily",
    ];
    let m = msg.to_ascii_lowercase();
    HTTP_KEYS.iter().any(|k| m.contains(k)) || NET_KEYS.iter().any(|k| m.contains(k))
}

/// 可取消的睡眠：分片睡，期间轮询取消标志（避免取消后还要干等 30 秒）。
fn sleep_cancellable(ms: u64, cancel: &AtomicBool) -> Result<(), String> {
    let mut left = ms;
    while left > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        let step = left.min(100);
        std::thread::sleep(std::time::Duration::from_millis(step));
        left -= step;
    }
    Ok(())
}

/// 单批翻译 + 指数退避重试。
///
/// 退避序列 1s/2s/4s/8s…（上限 30s，`attempts` 次后放弃）；
/// 不可重试的错误（如 400）立即返回，不做多余请求。
fn retry_batch(tr: &dyn Translator, texts: &[String], attempts: usize, cancel: &AtomicBool) -> Result<Vec<String>, String> {
    let attempts = attempts.max(1);
    let mut last = String::new();
    for a in 0..attempts {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        match tr.translate_batch(texts) {
            Ok(v) if v.len() == texts.len() => return Ok(v),
            Ok(v) => last = format!("机翻返回条数不匹配：{} != {}", v.len(), texts.len()),
            Err(e) => last = e,
        }
        if a + 1 < attempts && looks_retryable(&last) {
            let wait = (1000u64 << a.min(5)).min(30_000);
            crate::diag::log("WARN", &format!("机翻第 {}/{} 次失败，{wait}ms 后重试：{last}", a + 1, attempts));
            sleep_cancellable(wait, cancel)?;
            continue;
        }
        break;
    }
    Err(last)
}

/// 并发跑「一批 = 一段原文」的翻译任务。
///
/// - `batches`：已分好的批次；`workers`：并发度（网络型任务，2~6 合适）；
/// - `on_batch(idx, 译文)`：在**主线程**按完成顺序回调，用于回填 + 落盘；
///   返回 `Err` 即视为致命错误，立即停止调度新批次；
/// - 任一在途批次最终失败 → 记录首个错误并停止调度，但**已成功批次照常回调**
///   （不浪费已经花掉的翻译请求）；
/// - 取消：`cancel` 置位后 worker 在批次边界退出。
fn run_batches(
    tr: &dyn Translator,
    batches: Vec<Vec<String>>,
    workers: usize,
    attempts: usize,
    cancel: &AtomicBool,
    mut on_batch: impl FnMut(usize, &[String]) -> Result<(), String>,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool as AB, AtomicUsize};
    use std::sync::mpsc;

    let total = batches.len();
    if total == 0 {
        return Ok(());
    }
    let workers = workers.max(1).min(total);
    let cursor = AtomicUsize::new(0);
    let abort = AB::new(false);
    let (tx, rx) = mpsc::channel::<(usize, Result<Vec<String>, String>)>();
    let batches = &batches;
    let mut first_err: Option<String> = None;

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            let cursor = &cursor;
            let abort = &abort;
            scope.spawn(move || loop {
                if abort.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                    break;
                }
                let i = cursor.fetch_add(1, Ordering::Relaxed);
                if i >= total {
                    break;
                }
                if tx.send((i, retry_batch(tr, &batches[i], attempts, cancel))).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        loop {
            if cancel.load(Ordering::Relaxed) {
                abort.store(true, Ordering::Relaxed);
            }
            match rx.recv_timeout(std::time::Duration::from_millis(150)) {
                Ok((idx, r)) => match r {
                    Ok(trans) => {
                        // 已成功的批次照常回填（即使此前已有别的批次失败）
                        if let Err(e) = on_batch(idx, &trans) {
                            if first_err.is_none() {
                                first_err = Some(e);
                            }
                            abort.store(true, Ordering::Relaxed);
                        }
                    }
                    Err(e) => {
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                        abort.store(true, Ordering::Relaxed);
                    }
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    if let Some(e) = first_err {
        return Err(e);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("已取消".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 断点续翻：旁车文件 <目标>.mtl.json = { "原文": "译文" }
// ---------------------------------------------------------------------------

fn checkpoint_path(target: &Path) -> PathBuf {
    let mut n = target.file_name().map(|s| s.to_os_string()).unwrap_or_default();
    n.push(".mtl.json");
    target.with_file_name(n)
}

fn load_checkpoint(p: &Path) -> Result<HashMap<String, String>, String> {
    if !p.exists() {
        return Ok(HashMap::new());
    }
    let txt = std::fs::read_to_string(p).map_err(|e| format!("读取断点文件失败: {e}"))?;
    serde_json::from_str(&txt).map_err(|e| format!("断点文件损坏（可删除 {} 重来）: {e}", p.display()))
}

fn save_checkpoint(p: &Path, cp: &HashMap<String, String>) -> Result<(), String> {
    let txt = serde_json::to_string_pretty(cp).map_err(|e| e.to_string())?;
    crate::settings::ensure_parent(p);
    std::fs::write(p, txt).map_err(|e| format!("写入断点文件失败: {e}"))
}

// ---------------------------------------------------------------------------
// CSV 管线
// ---------------------------------------------------------------------------

/// 机翻 CSV 的 translation 列（第 5 列，r[4]）。
/// 只翻"原文非空且译文为空"的行；断点续翻；`jobs` 批并发；每批完成即落盘。
/// 返回 (本次新翻条数, 待翻总数)。
pub fn translate_csv(
    csv_path: &Path,
    tr: &dyn Translator,
    batch: usize,
    jobs: usize,
    progress: &dyn Fn(f32, &str),
    cancel: &AtomicBool,
) -> Result<(usize, usize), String> {
    let mut rows = crate::features::text::read_csv(csv_path)?;
    if rows.is_empty() {
        return Err("CSV 为空（请先做文本提取）".into());
    }
    let cp_path = checkpoint_path(csv_path);
    let mut cp = load_checkpoint(&cp_path)?;

    // 1) 断点回填
    for r in rows.iter_mut() {
        if r[4].trim().is_empty() {
            if let Some(t) = cp.get(&r[3]) {
                r[4] = t.clone();
            }
        }
    }

    // 2) 待翻清单
    let todo: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| !r[3].trim().is_empty() && r[4].trim().is_empty())
        .map(|(i, _)| i)
        .collect();
    let total = todo.len();
    if total == 0 {
        let _ = std::fs::remove_file(&cp_path);
        crate::features::text::write_csv_rows(csv_path, &rows)?;
        return Ok((0, 0));
    }

    // 3) 分批并发翻译（每批带指数退避重试）
    let mut applied = 0usize;
    let batch = batch.max(1);
    let key_chunks: Vec<Vec<usize>> = todo.chunks(batch).map(|c| c.to_vec()).collect();
    let batches: Vec<Vec<String>> = key_chunks.iter().map(|c| c.iter().map(|&i| rows[i][3].clone()).collect()).collect();

    let r = run_batches(tr, batches, jobs, RETRY_ATTEMPTS, cancel, |idx, trans| {
        for (&i, t) in key_chunks[idx].iter().zip(trans) {
            rows[i][4] = t.clone();
            cp.insert(rows[i][3].clone(), t.clone());
            applied += 1;
        }
        crate::features::text::write_csv_rows(csv_path, &rows)?;
        save_checkpoint(&cp_path, &cp)?;
        progress(applied as f32 / total as f32, &format!("机翻中 {applied}/{total}"));
        Ok(())
    });

    // 无论成败都落盘一次，保证已完成部分不丢
    let _ = crate::features::text::write_csv_rows(csv_path, &rows);
    let _ = save_checkpoint(&cp_path, &cp);
    match r {
        Ok(()) => {
            let _ = std::fs::remove_file(&cp_path);
            Ok((applied, total))
        }
        Err(e) if e == "已取消" => Err(format!("已取消。已机翻 {applied}/{total} 条并写入 CSV，重试将从断点继续")),
        Err(e) => Err(format!("机翻请求失败: {e}（已完成部分已写入 CSV，重试将从断点继续）")),
    }
}

// ---------------------------------------------------------------------------
// 注入 JSON 管线
// ---------------------------------------------------------------------------

/// 机翻注入 JSON：值为空串/缺失的键以键（原文）为输入翻译后写回。
/// 支持分组嵌套（拍平输出为扁平 {原文:译文}）；已有非空译文一律保留；`jobs` 批并发。
pub fn translate_json(
    json_path: &Path,
    tr: &dyn Translator,
    batch: usize,
    jobs: usize,
    progress: &dyn Fn(f32, &str),
    cancel: &AtomicBool,
) -> Result<(usize, usize), String> {
    let txt = std::fs::read_to_string(json_path).map_err(|e| format!("读取翻译 JSON 失败: {e}"))?;
    let txt = txt.trim_start_matches('\u{feff}');
    let v: serde_json::Value = serde_json::from_str(txt).map_err(|e| format!("翻译 JSON 不是合法 JSON: {e}"))?;
    let obj = v.as_object().ok_or("翻译 JSON 顶层必须是对象")?;

    // 拍平（保留空值待翻）
    fn flatten(obj: &serde_json::Map<String, serde_json::Value>, out: &mut serde_json::Map<String, serde_json::Value>, depth: usize) {
        if depth > 4 {
            return;
        }
        for (k, val) in obj {
            match val {
                serde_json::Value::String(s) => {
                    out.insert(k.clone(), json!(s));
                }
                serde_json::Value::Object(o) => flatten(o, out, depth + 1),
                _ => {}
            }
        }
    }
    let mut map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    flatten(obj, &mut map, 0);
    if map.is_empty() {
        return Err("翻译 JSON 里没有任何键".into());
    }

    let cp_path = checkpoint_path(json_path);
    let mut cp = load_checkpoint(&cp_path)?;

    // 断点回填
    for (k, val) in map.iter_mut() {
        if val.as_str().map(|s| s.trim().is_empty()).unwrap_or(true) {
            if let Some(t) = cp.get(k) {
                *val = json!(t);
            }
        }
    }

    let todo: Vec<String> = map
        .iter()
        .filter(|(k, val)| val.as_str().map(|s| s.trim().is_empty()).unwrap_or(true) && !cp.contains_key(*k))
        .map(|(k, _)| k.clone())
        .collect();
    let total = todo.len();
    if total == 0 {
        let _ = std::fs::remove_file(&cp_path);
        write_flat(json_path, &map)?;
        return Ok((0, 0));
    }

    let mut applied = 0usize;
    let batch = batch.max(1);
    let key_chunks: Vec<Vec<String>> = todo.chunks(batch).map(|c| c.to_vec()).collect();
    let batches: Vec<Vec<String>> = key_chunks.clone();

    let r = run_batches(tr, batches, jobs, RETRY_ATTEMPTS, cancel, |idx, trans| {
        for (k, t) in key_chunks[idx].iter().zip(trans) {
            map.insert(k.clone(), json!(t));
            cp.insert(k.clone(), t.clone());
            applied += 1;
        }
        write_flat(json_path, &map)?;
        save_checkpoint(&cp_path, &cp)?;
        progress(applied as f32 / total as f32, &format!("机翻中 {applied}/{total}"));
        Ok(())
    });

    // 无论成败都落盘一次，保证已完成部分不丢
    let _ = write_flat(json_path, &map);
    let _ = save_checkpoint(&cp_path, &cp);
    match r {
        Ok(()) => {
            let _ = std::fs::remove_file(&cp_path);
            Ok((applied, total))
        }
        Err(e) if e == "已取消" => Err(format!("已取消。已机翻 {applied}/{total} 条并写回 JSON，重试将从断点继续")),
        Err(e) => Err(format!("机翻请求失败: {e}（已完成部分已写回 JSON，重试将从断点继续）")),
    }
}

fn write_flat(json_path: &Path, map: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let txt = serde_json::to_string_pretty(&serde_json::Value::Object(map.clone())).map_err(|e| e.to_string())?;
    crate::settings::ensure_parent(json_path);
    std::fs::write(json_path, txt).map_err(|e| format!("写入翻译 JSON 失败: {e}"))
}

// 防止未使用告警的小工具（保留扩展口）
#[allow(dead_code)]
fn _unused_read(r: &mut dyn Read) -> Result<(), String> {
    let mut b = [0u8; 1];
    r.read(&mut b).map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    struct Mock;
    impl Translator for Mock {
        fn name(&self) -> &str {
            "mock"
        }
        fn translate_batch(&self, texts: &[String]) -> Result<Vec<String>, String> {
            Ok(texts.iter().map(|t| format!("译:{t}")).collect())
        }
    }

    /// 可按调用次数制造失败的 Mock：前 `fail_first` 次返回 `err`，之后成功（`ok` 为 true 时始终成功）。
    struct FlakyMock {
        calls: AtomicUsize,
        fail_first: usize,
        err: &'static str,
        ok: bool,
    }
    impl FlakyMock {
        fn new(fail_first: usize, err: &'static str, ok: bool) -> Self {
            FlakyMock { calls: AtomicUsize::new(0), fail_first, err, ok }
        }
    }
    impl Translator for FlakyMock {
        fn name(&self) -> &str {
            "flaky"
        }
        fn translate_batch(&self, texts: &[String]) -> Result<Vec<String>, String> {
            let n = self.calls.fetch_add(1, Ordering::Relaxed);
            if self.ok || n >= self.fail_first {
                Ok(texts.iter().map(|t| format!("译:{t}")).collect())
            } else {
                Err(self.err.to_string())
            }
        }
    }

    /// 记录被哪些线程调用的 Mock，用于验证真并发。
    struct TrackingMock {
        threads: std::sync::Mutex<std::collections::HashSet<std::thread::ThreadId>>,
    }
    impl Translator for TrackingMock {
        fn name(&self) -> &str {
            "tracking"
        }
        fn translate_batch(&self, texts: &[String]) -> Result<Vec<String>, String> {
            self.threads.lock().unwrap().insert(std::thread::current().id());
            std::thread::sleep(std::time::Duration::from_millis(5));
            Ok(texts.iter().map(|t| format!("译:{t}")).collect())
        }
    }

    fn write_test_csv(p: &Path) {
        crate::features::text::write_csv_rows(
            p,
            &[
                ["1".into(), "f.csv".into(), "ctx".into(), "こんにちは".into(), "".into()],
                ["2".into(), "f.csv".into(), "ctx".into(), "選択肢".into(), "".into()],
                ["3".into(), "f.csv".into(), "ctx".into(), "已翻".into(), "已译".into()],
                ["4".into(), "f.csv".into(), "ctx".into(), "".into(), "".into()], // 空原文：跳过
            ],
        )
        .unwrap();
    }

    #[test]
    fn test_translate_csv_roundtrip() {
        let dir = std::env::temp_dir().join(format!("stool_mt_csv_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("text.csv");
        write_test_csv(&p);
        let cancel = AtomicBool::new(false);
        let (applied, total) = translate_csv(&p, &Mock, 2, 4, &|_, _| {}, &cancel).unwrap();
        assert_eq!((applied, total), (2, 2));
        let rows = crate::features::text::read_csv(&p).unwrap();
        assert_eq!(rows[0][4], "译:こんにちは");
        assert_eq!(rows[1][4], "译:選択肢");
        assert_eq!(rows[2][4], "已译"); // 已有译文保留
        assert_eq!(rows[3][4], ""); // 空原文跳过
        assert!(!checkpoint_path(&p).exists()); // 完成后删断点
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_translate_csv_resume_after_cancel() {
        let dir = std::env::temp_dir().join(format!("stool_mt_csv2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("text.csv");
        write_test_csv(&p);
        // 预先置取消：第一轮直接中断（此时 0 条完成，验证取消路径不炸）
        let cancel = Arc::new(AtomicBool::new(true));
        let r = translate_csv(&p, &Mock, 2, 4, &|_, _| {}, &cancel);
        assert!(r.is_err());
        // 解除取消后续翻
        cancel.store(false, Ordering::Relaxed);
        let (applied, _) = translate_csv(&p, &Mock, 2, 4, &|_, _| {}, &cancel).unwrap();
        assert_eq!(applied, 2);
        let rows = crate::features::text::read_csv(&p).unwrap();
        assert_eq!(rows[0][4], "译:こんにちは");
        assert!(!checkpoint_path(&p).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_translate_json_nested_and_keep() {
        let dir = std::env::temp_dir().join(format!("stool_mt_json_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("translation.json");
        std::fs::write(
            &p,
            "{\"こんにちは\":\"\", \"会話\": {\"選択肢\": \"\"}, \"你好\": \"已有译文\"}",
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let (applied, total) = translate_json(&p, &Mock, 10, 4, &|_, _| {}, &cancel).unwrap();
        assert_eq!((applied, total), (2, 2));
        let out = std::fs::read_to_string(&p).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["こんにちは"], "译:こんにちは");
        assert_eq!(v["選択肢"], "译:選択肢"); // 嵌套拍平
        assert_eq!(v["你好"], "已有译文"); // 已有译文保留
        assert!(!checkpoint_path(&p).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn looks_retryable_classifies_transient_vs_client() {
        // 应重试：限流 / 服务端 / 网络抖动
        assert!(looks_retryable("HTTP 429：rate limit exceeded"));
        assert!(looks_retryable("HTTP 503：service unavailable"));
        assert!(looks_retryable("HTTP 502：bad gateway"));
        assert!(looks_retryable("网络错误: timed out"));
        assert!(looks_retryable("connection reset by peer"));
        // 不应重试：客户端错 / 逻辑错
        assert!(!looks_retryable("HTTP 400：bad request"));
        assert!(!looks_retryable("HTTP 401：unauthorized"));
        assert!(!looks_retryable("HTTP 404：model not found"));
        assert!(!looks_retryable("机翻输出不是 JSON 数组"));
    }

    #[test]
    fn retry_batch_retries_transient_failure_then_succeeds() {
        // 首发失败（503）→ 退避 1s → 第 2 次成功
        let mock = FlakyMock::new(1, "HTTP 503：busy", false);
        let cancel = AtomicBool::new(false);
        let out = retry_batch(&mock, &["x".to_string()], 3, &cancel).unwrap();
        assert_eq!(out, vec!["译:x".to_string()]);
        assert_eq!(mock.calls.load(Ordering::Relaxed), 2, "首发失败 + 1 次重试成功");
    }

    #[test]
    fn retry_batch_gives_up_immediately_on_client_error() {
        let mock = FlakyMock::new(99, "HTTP 400：bad request", false);
        let cancel = AtomicBool::new(false);
        let t0 = std::time::Instant::now();
        let e = retry_batch(&mock, &["x".to_string()], 4, &cancel).unwrap_err();
        assert!(e.contains("400"));
        assert_eq!(mock.calls.load(Ordering::Relaxed), 1, "不可重试 → 只发一次");
        assert!(t0.elapsed() < std::time::Duration::from_millis(800), "客户端错不应退避等待");
    }

    #[test]
    fn retry_batch_stops_on_cancel_during_backoff() {
        let mock = FlakyMock::new(99, "HTTP 429：rate limit", false);
        let cancel = AtomicBool::new(true); // 已取消：退避睡眠应立刻中断
        let t0 = std::time::Instant::now();
        let e = retry_batch(&mock, &["x".to_string()], 4, &cancel).unwrap_err();
        assert_eq!(e, "已取消");
        assert!(t0.elapsed() < std::time::Duration::from_millis(500), "取消应立刻生效");
    }

    #[test]
    fn run_batches_uses_multiple_workers_and_applies_all() {
        let mock = TrackingMock { threads: std::sync::Mutex::new(std::collections::HashSet::new()) };
        let batches: Vec<Vec<String>> = (0..24).map(|i| vec![format!("t{i}")]).collect();
        let cancel = AtomicBool::new(false);
        let mut got: Vec<(usize, String)> = Vec::new();
        let r = run_batches(&mock, batches, 4, 1, &cancel, |idx, trans| {
            got.push((idx, trans[0].clone()));
            Ok(())
        });
        assert!(r.is_ok());
        assert_eq!(got.len(), 24, "每批都应回调且不重不漏");
        let mut idxs: Vec<usize> = got.iter().map(|(i, _)| *i).collect();
        idxs.sort_unstable();
        assert_eq!(idxs, (0..24).collect::<Vec<_>>());
        let used = mock.threads.lock().unwrap().len();
        assert!(used >= 2, "应真的并发执行（用到 {used} 个线程）");
    }

    #[test]
    fn run_batches_returns_error_but_keeps_successes() {
        struct OneBad;
        impl Translator for OneBad {
            fn name(&self) -> &str {
                "onebad"
            }
            fn translate_batch(&self, texts: &[String]) -> Result<Vec<String>, String> {
                if texts[0] == "bad" {
                    return Err("HTTP 400：bad request".into());
                }
                Ok(texts.iter().map(|t| format!("译:{t}")).collect())
            }
        }
        let mut batches: Vec<Vec<String>> = (0..10).map(|i| vec![format!("t{i}")]).collect();
        batches.push(vec!["bad".to_string()]);
        let cancel = AtomicBool::new(false);
        let applied = AtomicUsize::new(0);
        let r = run_batches(&OneBad, batches, 2, 1, &cancel, |_idx, _t| {
            applied.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("400"), "应回传首个错误");
        assert!(applied.load(Ordering::Relaxed) >= 1, "已成功批次应照常回填，不浪费请求");
    }
}
