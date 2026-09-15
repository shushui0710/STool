//! GUI 通用小工具：页头、路径转义、JSON 值显示/解析、小工具函数、语义色与卡片容器。

use eframe::egui;
use eframe::egui::{Color32, RichText};

// ---------------------------------------------------------------------------
// 语义色：全界面只从这里取色，别再在页面里散写 Color32::from_rgb(...)
// （散写会导致「同一个意思、不同页面颜色不一样」，改主题时还要满仓找）
// ---------------------------------------------------------------------------

/// 成功 / 可用 / 高可行性。
pub(crate) const C_OK: Color32 = Color32::from_rgb(90, 160, 90);
/// 提醒 / 需注意 / 中等可行性。
pub(crate) const C_WARN: Color32 = Color32::from_rgb(200, 150, 60);
/// 危险 / 失败 / 不建议（注意：涨跌用红绿，这里只用于状态语义）。
pub(crate) const C_DANGER: Color32 = Color32::from_rgb(210, 110, 110);
/// 中性 / 低优先级 / 次要文字。
pub(crate) const C_MUTED: Color32 = Color32::from_rgb(150, 150, 150);

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

/// 统一的「卡片」容器：浅色圆角底 + 内边距，让页面分区一眼可辨。
///
/// 返回内部 [`egui::InnerResponse`]，与 `Frame::group(...).show(...)` 用法一致。
pub(crate) fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    let dark = ui.visuals().dark_mode;
    let fill = if dark { Color32::from_gray(32) } else { Color32::from_gray(252) };
    egui::Frame::none()
        .fill(fill)
        .rounding(egui::Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .show(ui, |ui| {
            ui.set_width(ui.available_width().max(120.0));
            add(ui)
        })
}

/// 卡片标题行：大一点的标题 + 右侧可选说明。
pub(crate) fn card_title(ui: &mut egui::Ui, title: &str, hint: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong());
        if !hint.is_empty() {
            ui.label(RichText::new(hint).weak().small());
        }
    });
    ui.add_space(2.0);
}

/// 「还没检测游戏」的空状态：说明为什么走不下去 + 一个能直接跳回首页的按钮。
///
/// 旧的写法只丢一句「请先在首页完成检测。」，用户在页面里**没有出路**——
/// 得自己想起来点左侧的「引擎检测」。这里改成带动作的空状态。
/// 返回 `true` 表示用户点了按钮，调用方负责 `self.page = Page::Home`。
pub(crate) fn need_detect(ui: &mut egui::Ui, what: &str) -> bool {
    let mut go = false;
    ui.add_space(16.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("🎮").heading());
        ui.label(RichText::new(format!("「{what}」要先知道这是什么游戏")).strong());
        ui.add_space(2.0);
        ui.label(
            RichText::new("回「引擎检测」页选好游戏目录、点「检测」即可；检测结果会自动带到这一页。")
                .weak()
                .small(),
        );
        ui.add_space(8.0);
        go = ui.button("🏠 去首页检测").clicked();
    });
    go
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

// 值的显示/解析口径统一由内核提供（`features::saves`），这里只做转发：
// egui 版与 Tauri 版改同一个字段时，解析规则不会各写一套、日后漂移。
pub(crate) use crate::features::saves::{parse_edit_text, type_hint, value_edit_text};

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
