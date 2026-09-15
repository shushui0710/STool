//! 存档结构树的只读渲染（性能要点见文件内注释）。

use crate::features::saves;
use eframe::egui;
use super::util::*;
use std::collections::{HashMap};
use eframe::egui::{Color32, RichText};

// ---------------------------------------------------------------------------
// 存档结构树渲染
//
// 性能要点（这是存档页滚动卡顿的根因所在）：
//   egui 的 ScrollArea **不做视口裁剪**——它会把所有子控件都布局一遍来测量内容高度，
//   无论是否滚出屏幕；而 TextEdit 是 egui 里最重的控件之一（要建 galley、分配 ID、
//   维护光标/选择/IME 状态）。旧实现给每个字段都建一个 TextEdit，展开
//   switches/variables（上千项）后每帧要重建上千个 TextEdit → 帧时间线性暴涨、滚动卡死。
//
// 现在的做法：
//   1) 叶子 = 单行 SelectableLabel（纯文本、无状态），编辑收敛到右侧单一面板；
//   2) 单个容器最多渲染 SAVE_RENDER_CAP 行，超出用“显示更多”分批展开；
//   3) 加一道单帧总行数预算 SAVE_ROW_BUDGET 兜底，防止多个大分支同时展开；
//   4) 悬停详情用 `on_hover_ui`（**惰性**）：旧写法每帧都为每一行拼一个 `String` 提示，
//      1500 行就是 1500 次分配，纯属白费；
//   5) 结构树本身已降级为「原始数据浏览」辅助视图（默认折叠，见 save.rs），
//      日常改值走搜索结果列表（虚拟化渲染，只布局可见行）。
// ---------------------------------------------------------------------------

/// 单个容器一次最多渲染的行数（超出用“显示更多”分批展开）。
pub(crate) const SAVE_RENDER_CAP: usize = 200;
/// 每次点“显示更多”追加的行数。
pub(crate) const SAVE_RENDER_STEP: usize = 200;
/// 一帧内整棵树最多渲染的行数（兜底）。
///
/// 600 行 ≈ 明暗两套主题下都能保持 60fps 的量级；整棵树只作辅助浏览，
/// 真要在大存档里找字段请用搜索（结果列表是虚拟化渲染，不受这个预算限制）。
pub(crate) const SAVE_ROW_BUDGET: usize = 600;

/// 单帧渲染预算：用完后停止渲染并只提示一次，避免多分支重复刷提示。
pub(crate) struct RenderBudget {
    pub(crate) rows: usize,
    pub(crate) hint_shown: bool,
}

/// 递归渲染只读结构树；返回本帧被点击的叶子路径（若有）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_tree(
    ui: &mut egui::Ui,
    node: &mut serde_json::Value,
    ptr: &str,
    depth: usize,
    selected: Option<&str>,
    auto_open: bool,
    more: &mut HashMap<String, usize>,
    budget: &mut RenderBudget,
) -> Option<String> {
    let mut clicked: Option<String> = None;
    match node {
        serde_json::Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            let total = keys.len();
            let limit = (SAVE_RENDER_CAP + more.get(ptr).copied().unwrap_or(0)).min(total);
            for k in keys.into_iter().take(limit) {
                if budget.rows == 0 {
                    budget_hint(ui, budget);
                    return clicked;
                }
                budget.rows -= 1;
                let child_ptr = format!("{ptr}/{}", escape_ptr(&k));
                let Some(v) = map.get_mut(&k) else { continue };
                if v.is_object() || v.is_array() {
                    let title = format!("📁 {k}（{}）", container_len(v));
                    if let Some(c) = render_branch(ui, v, &child_ptr, &title, depth, selected, auto_open, more, budget) {
                        clicked = Some(c);
                    }
                } else if render_row(ui, &k, v, &child_ptr, selected) {
                    clicked = Some(child_ptr);
                }
            }
            if total > limit {
                show_more_button(ui, ptr, total, limit, more);
            }
        }
        serde_json::Value::Array(arr) => {
            let total = arr.len();
            let limit = (SAVE_RENDER_CAP + more.get(ptr).copied().unwrap_or(0)).min(total);
            for (i, v) in arr.iter_mut().enumerate().take(limit) {
                if budget.rows == 0 {
                    budget_hint(ui, budget);
                    return clicked;
                }
                budget.rows -= 1;
                let child_ptr = format!("{ptr}/{i}");
                let label = format!("[{i}]");
                if v.is_object() || v.is_array() {
                    let title = format!("📁 {label}（{}）", container_len(v));
                    if let Some(c) = render_branch(ui, v, &child_ptr, &title, depth, selected, auto_open, more, budget) {
                        clicked = Some(c);
                    }
                } else if render_row(ui, &label, v, &child_ptr, selected) {
                    clicked = Some(child_ptr);
                }
            }
            if total > limit {
                show_more_button(ui, ptr, total, limit, more);
            }
        }
        other => {
            // 根节点直接是标量（罕见）
            if render_row(ui, "(根)", other, ptr, selected) {
                clicked = Some(ptr.to_string());
            }
        }
    }
    clicked
}

/// 渲染一个可折叠容器（默认折叠，egui 会记住展开状态；折叠时不会布局子项）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_branch(
    ui: &mut egui::Ui,
    node: &mut serde_json::Value,
    ptr: &str,
    title: &str,
    depth: usize,
    selected: Option<&str>,
    auto_open: bool,
    more: &mut HashMap<String, usize>,
    budget: &mut RenderBudget,
) -> Option<String> {
    let mut clicked = None;
    egui::CollapsingHeader::new(RichText::new(title))
        .id_salt(ptr)
        .default_open(auto_open && depth == 0)
        .show(ui, |ui| {
            clicked = render_tree(ui, node, ptr, depth + 1, selected, auto_open, more, budget);
        });
    clicked
}

/// 渲染一个叶子行：`键 : 值`，点击选中。全程不含输入控件。
pub(crate) fn render_row(ui: &mut egui::Ui, key: &str, node: &serde_json::Value, ptr: &str, selected: Option<&str>) -> bool {
    let is_sel = selected == Some(ptr);
    let dark = ui.visuals().dark_mode;
    let weak_c = ui.visuals().weak_text_color();
    let sel_c = Color32::from_rgb(246, 249, 255);
    let (key_c, sep_c, val_c) = if is_sel {
        (sel_c, sel_c, sel_c)
    } else {
        (weak_c, weak_c, val_kind_color(dark, val_kind(node)))
    };
    let font = egui::FontId::monospace(ROW_FONT_SIZE);
    let mut job = egui::text::LayoutJob::default();
    job.append(&short_text(key, 38), 0.0, egui::TextFormat { font_id: font.clone(), color: key_c, ..Default::default() });
    job.append("  :  ", 0.0, egui::TextFormat { font_id: font.clone(), color: sep_c, ..Default::default() });
    job.append(&value_preview(node), 0.0, egui::TextFormat { font_id: font, color: val_c, ..Default::default() });
    // 悬停详情用惰性回调：只有真的悬停时才拼字符串（旧写法每帧每行都拼一次，白费分配）
    ui.add(egui::SelectableLabel::new(is_sel, job))
        .on_hover_ui(|ui| {
            ui.label(RichText::new(ptr).monospace().small());
            ui.label(RichText::new(format!("类型：{}", type_hint(node))).small());
        })
        .clicked()
}

/// 结构树行的等宽字号（11 号在长列表里太挤，可读性差）。
pub(crate) const ROW_FONT_SIZE: f32 = 12.0;

/// 值的类别。只存类别、颜色在渲染时按当前主题算，这样主题切换不用重建缓存。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValKind {
    Other,
    Str,
    Num,
    Bool,
}

pub(crate) fn val_kind(v: &serde_json::Value) -> ValKind {
    match v {
        serde_json::Value::String(_) => ValKind::Str,
        serde_json::Value::Number(_) => ValKind::Num,
        serde_json::Value::Bool(_) => ValKind::Bool,
        _ => ValKind::Other,
    }
}

/// 值按类型着色，方便在长列表里快速分辨（明暗主题各一套）。
pub(crate) fn val_kind_color(dark: bool, k: ValKind) -> Color32 {
    match (k, dark) {
        (ValKind::Str, true) => Color32::from_rgb(140, 205, 160),
        (ValKind::Str, false) => Color32::from_rgb(30, 120, 70),
        (ValKind::Num, true) => Color32::from_rgb(130, 180, 250),
        (ValKind::Num, false) => Color32::from_rgb(30, 80, 190),
        (ValKind::Bool, true) => Color32::from_rgb(232, 180, 110),
        (ValKind::Bool, false) => Color32::from_rgb(170, 100, 20),
        (ValKind::Other, true) => Color32::from_gray(150),
        (ValKind::Other, false) => Color32::from_gray(110),
    }
}

// ---------------------------------------------------------------------------
// 搜索结果行（预计算 + 虚拟化渲染）
// ---------------------------------------------------------------------------

/// 搜索结果的一行。**预先算好**，渲染时不再查树、不再格式化。
///
/// 旧实现每帧对每条命中做 `doc.get()` + `to_string()` + `format!()` + `short_text()`，
/// 命中上千条时每帧上千次堆分配 —— 这就是「搜索结果一多就卡」的原因。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SaveRow {
    pub(crate) path: String,
    /// 可直接编辑的文本形态（字符串带引号、数字原样）。
    pub(crate) value: String,
    /// 单行预览（截断后的展示用文本）。
    pub(crate) preview: String,
    pub(crate) kind: ValKind,
    pub(crate) ty: &'static str,
}

impl SaveRow {
    fn of(doc: &saves::SaveDoc, path: &str) -> SaveRow {
        match doc.get(path) {
            Some(v) => {
                let value = value_edit_text(v);
                SaveRow {
                    path: path.to_string(),
                    preview: short_text(&value, 42),
                    value,
                    kind: val_kind(v),
                    ty: type_hint(v),
                }
            }
            None => SaveRow {
                path: path.to_string(),
                value: String::new(),
                preview: "(已不存在)".into(),
                kind: ValKind::Other,
                ty: "-",
            },
        }
    }
}

/// 按路径列表构建结果行（搜索后调用一次）。
pub(crate) fn save_rows_of(doc: &saves::SaveDoc, paths: &[String]) -> Vec<SaveRow> {
    paths.iter().map(|p| SaveRow::of(doc, p)).collect()
}

/// 某一行被改动后重算它自己（避免整表重建）。
pub(crate) fn refresh_save_row(doc: &saves::SaveDoc, rows: &mut [SaveRow], path: &str) {
    if let Some(r) = rows.iter_mut().find(|r| r.path == path) {
        *r = SaveRow::of(doc, path);
    }
}

/// “显示更多”按钮：把该容器的分页上限再抬高 SAVE_RENDER_STEP。
pub(crate) fn show_more_button(ui: &mut egui::Ui, ptr: &str, total: usize, shown: usize, more: &mut HashMap<String, usize>) {
    let remain = total - shown;
    let label = format!("⋯ 还有 {remain} 项未显示（{shown}/{total}），点此再展开 {SAVE_RENDER_STEP} 项，或用搜索定位");
    if ui.button(RichText::new(label).small()).clicked() {
        *more.entry(ptr.to_string()).or_insert(0) += SAVE_RENDER_STEP;
    }
}

/// 单帧预算耗尽提示（只提示一次）。
pub(crate) fn budget_hint(ui: &mut egui::Ui, budget: &mut RenderBudget) {
    if !budget.hint_shown {
        budget.hint_shown = true;
        ui.label(
            RichText::new(format!(
                "⋯ 已达单帧渲染上限（{SAVE_ROW_BUDGET} 行）。折叠部分分支或改用搜索定位，可保持滚动流畅。"
            ))
            .weak()
            .small(),
        );
    }
}

/// 叶子值的单行预览：字符串带引号以区别于数值，过长则截断。
pub(crate) fn value_preview(v: &serde_json::Value) -> String {
    let raw = match v {
        serde_json::Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    };
    short_text(&raw, 60)
}
