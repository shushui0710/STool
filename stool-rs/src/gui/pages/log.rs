//! 页面：log（P2-11 从 gui.rs 拆出）。

use crate::gui::*;
use std::sync::atomic::{Ordering};
use eframe::egui::{Color32, RichText};

impl StoolApp {
    pub(crate) fn page_log(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("运行日志");
            let busy = self.diag_busy.load(Ordering::Relaxed);
            let label = if busy { "导出中…" } else { "导出诊断包" };
            if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                self.start_diag_export();
            }
        });
        ui.label(
            RichText::new(
                "诊断包 = 日志 + 环境（API Key 已脱敏）+ 引擎检测 + 备份清单 → 单个 zip；排障先看 warnings.txt",
            )
            .small()
            .weak(),
        );
        ui.add_space(4.0);
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            if let Ok(logs) = self.shared.logs.lock() {
                let text_color = ui.visuals().text_color();
                let weak = ui.visuals().weak_text_color();
                for line in logs.iter() {
                    let color = if line.contains("✔") {
                        Color32::from_rgb(120, 200, 120)
                    } else if line.contains("✘") {
                        Color32::from_rgb(235, 120, 120)
                    } else if line.contains("▶") {
                        Color32::from_rgb(120, 170, 235)
                    } else {
                        text_color
                    };
                    // 时间戳淡显，正文按状态着色
                    if let Some((ts, body)) = line.split_once(']') {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(format!("{ts}]")).small().color(weak).monospace());
                            ui.label(RichText::new(body.trim_start()).small().color(color));
                        });
                    } else {
                        ui.label(RichText::new(line).small().color(color));
                    }
                }
            }
        });
    }
}
