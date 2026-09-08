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
            .map_err(|e| format!("请求 {endpoint} 失败: {e}", endpoint = self.endpoint()))?
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
            .map_err(|e| format!("机翻输出不是 JSON 数组: {e}\n原始输出前 500 字: {}", &c.chars().take(500).collect::<String>()))?;
        if arr.len() != texts.len() {
            return Err(format!("机翻返回条数不匹配：{} != {}", arr.len(), texts.len()));
        }
        Ok(arr)
    }
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
/// 只翻"原文非空且译文为空"的行；断点续翻；每批落盘一次。
/// 返回 (本次新翻条数, 待翻总数)。
pub fn translate_csv(
    csv_path: &Path,
    tr: &dyn Translator,
    batch: usize,
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

    // 3) 分批翻译
    let mut applied = 0usize;
    let batch = batch.max(1);
    for chunk in todo.chunks(batch) {
        if cancel.load(Ordering::Relaxed) {
            crate::features::text::write_csv_rows(csv_path, &rows)?;
            save_checkpoint(&cp_path, &cp)?;
            return Err(format!("已取消。已机翻 {applied}/{total} 条并写入 CSV，重试将从断点继续"));
        }
        let texts: Vec<String> = chunk.iter().map(|&i| rows[i][3].clone()).collect();
        let out = tr.translate_batch(&texts).map_err(|e| {
            let _ = crate::features::text::write_csv_rows(csv_path, &rows);
            let _ = save_checkpoint(&cp_path, &cp);
            format!("机翻请求失败: {e}（已完成部分已写入 CSV，重试将从断点继续）")
        })?;
        for (&i, t) in chunk.iter().zip(out.into_iter()) {
            rows[i][4] = t.clone();
            cp.insert(rows[i][3].clone(), t);
            applied += 1;
        }
        crate::features::text::write_csv_rows(csv_path, &rows)?;
        save_checkpoint(&cp_path, &cp)?;
        progress(applied as f32 / total as f32, &format!("机翻中 {applied}/{total}"));
    }
    let _ = std::fs::remove_file(&cp_path);
    Ok((applied, total))
}

// ---------------------------------------------------------------------------
// 注入 JSON 管线
// ---------------------------------------------------------------------------

/// 机翻注入 JSON：值为空串/缺失的键以键（原文）为输入翻译后写回。
/// 支持分组嵌套（拍平输出为扁平 {原文:译文}）；已有非空译文一律保留。
pub fn translate_json(
    json_path: &Path,
    tr: &dyn Translator,
    batch: usize,
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
    for chunk in todo.chunks(batch) {
        if cancel.load(Ordering::Relaxed) {
            write_flat(json_path, &map)?;
            save_checkpoint(&cp_path, &cp)?;
            return Err(format!("已取消。已机翻 {applied}/{total} 条并写回 JSON，重试将从断点继续"));
        }
        let texts: Vec<String> = chunk.to_vec();
        let out = tr.translate_batch(&texts).map_err(|e| {
            let _ = write_flat(json_path, &map);
            let _ = save_checkpoint(&cp_path, &cp);
            format!("机翻请求失败: {e}（已完成部分已写回 JSON，重试将从断点继续）")
        })?;
        for (k, t) in chunk.iter().zip(out.into_iter()) {
            map.insert(k.clone(), json!(t));
            cp.insert(k.clone(), t);
            applied += 1;
        }
        write_flat(json_path, &map)?;
        save_checkpoint(&cp_path, &cp)?;
        progress(applied as f32 / total as f32, &format!("机翻中 {applied}/{total}"));
    }
    let _ = std::fs::remove_file(&cp_path);
    Ok((applied, total))
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
        let (applied, total) = translate_csv(&p, &Mock, 2, &|_, _| {}, &cancel).unwrap();
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
        let r = translate_csv(&p, &Mock, 2, &|_, _| {}, &cancel);
        assert!(r.is_err());
        // 解除取消后续翻
        cancel.store(false, Ordering::Relaxed);
        let (applied, _) = translate_csv(&p, &Mock, 2, &|_, _| {}, &cancel).unwrap();
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
        let (applied, total) = translate_json(&p, &Mock, 10, &|_, _| {}, &cancel).unwrap();
        assert_eq!((applied, total), (2, 2));
        let out = std::fs::read_to_string(&p).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["こんにちは"], "译:こんにちは");
        assert_eq!(v["選択肢"], "译:選択肢"); // 嵌套拍平
        assert_eq!(v["你好"], "已有译文"); // 已有译文保留
        assert!(!checkpoint_path(&p).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
