//! GUI 通用小工具：页头、路径转义、JSON 值显示/解析、小工具函数。

use eframe::egui;
use eframe::egui::{RichText};

/// 统一页头：大标题 + 弱副标题 + 分隔间距。
pub(crate) fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(title).heading().strong());
    if !subtitle.is_empty() {
        ui.add_space(2.0);
        ui.label(RichText::new(subtitle).weak().small());
    }
    ui.add_space(8.0);
}

/// 转义 JSON Pointer 里的 ~ 和 /。
pub(crate) fn escape_ptr(k: &str) -> String {
    k.replace('~', "~0").replace('/', "~1")
}

pub(crate) fn container_len(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::Object(m) => m.len(),
        serde_json::Value::Array(a) => a.len(),
        _ => 0,
    }
}

pub(crate) fn type_hint(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::String(_) => "文本",
        serde_json::Value::Number(_) => "数值",
        serde_json::Value::Bool(_) => "布尔",
        serde_json::Value::Null => "空",
        _ => "",
    }
}

/// 值的编辑文本表示（字符串去引号，其他用 JSON 形式）。
pub(crate) fn value_edit_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 编辑文本解析回 JSON 值：true/false/null → 对应类型，数字 → 数值，其余 → 字符串。
pub(crate) fn parse_edit_text(s: &str) -> serde_json::Value {
    let t = s.trim();
    match t {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        "null" => serde_json::Value::Null,
        _ => {
            if let Ok(i) = t.parse::<i64>() {
                serde_json::json!(i)
            } else if let Ok(f) = t.parse::<f64>() {
                serde_json::json!(f)
            } else {
                serde_json::Value::String(s.to_string())
            }
        }
    }
}

pub(crate) fn short_text(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

/// 从运行时名称表里取第 id 个名字（没有就返回空串）。
pub(crate) fn json_name(names: &serde_json::Value, key: &str, id: i64) -> String {
    if id < 0 {
        return String::new();
    }
    names
        .get(key)
        .and_then(|v| v.get(id as usize))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// 用系统文件管理器打开目录。
#[cfg(windows)]
pub(crate) fn open_in_explorer(p: &std::path::Path) {
    let _ = std::process::Command::new("explorer").arg(p).spawn();
}
#[cfg(not(windows))]
pub(crate) fn open_in_explorer(_p: &std::path::Path) {}
