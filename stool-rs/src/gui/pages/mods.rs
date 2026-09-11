//! 页面：mods（P2-11 从 gui.rs 拆出）。

use crate::gui::*;
use eframe::egui::{Color32, RichText};

impl StoolApp {
    pub(crate) fn page_mods(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "补丁 / MOD 管理", "安装补丁目录覆盖到游戏，可停用/卸载并自动还原");
        ui.label(RichText::new("安装 = 把补丁目录覆盖到游戏目录（原文件自动备份到 stool_mods/_backups/）。").weak());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("名称:");
            ui.add_sized([180.0, 22.0], egui::TextEdit::singleline(&mut self.mod_name));
            if ui.button("选择补丁目录...").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    match crate::features::mods::install_mod(&self.game_root, &p, &self.mod_name, &|_, _| {}, self.mod_force) {
                        Ok(e) => {
                            self.toast = Some((format!("已安装 {}", e.name), std::time::Instant::now()));
                            self.mod_name.clear();
                            self.mods = crate::features::mods::list_mods(&self.game_root);
                        }
                        Err(e) => self.toast = Some((e, std::time::Instant::now())),
                    }
                }
            }
            ui.checkbox(&mut self.mod_force, "允许冲突覆盖")
                .on_hover_text("默认拒绝与已启用 MOD 覆盖同一文件；勾选后强制覆盖（仅在确实要替换时使用）");
        });
        // 冲突告警（P2-10）：多个已启用 MOD 争用同一文件时高亮提示
        let conflicts = crate::features::mods::conflicts(&self.game_root);
        if !conflicts.is_empty() {
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("⚠ 检测到 {} 处冲突：多个已启用 MOD 覆盖同一文件", conflicts.len()))
                    .color(Color32::from_rgb(230, 180, 90)),
            );
            for c in conflicts.iter().take(8) {
                ui.label(RichText::new(format!("   · {} ← {}", c.rel, c.mods.join(" / "))).weak().small());
            }
        }
        ui.add_space(10.0);
        ui.heading("已安装");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for m in self.mods.clone() {
                ui.horizontal(|ui| {
                    let state = if m.enabled { "[启用]" } else { "[停用]" };
                    ui.label(format!("{state} {} — {} 文件（覆盖 {}）", m.name, m.files.len(), m.overwritten.len()));
                    if ui.button(if m.enabled { "停用" } else { "启用" }).clicked() {
                        if let Err(e) = crate::features::mods::toggle_mod(&self.game_root, &m.name, !m.enabled) {
                            self.toast = Some((e, std::time::Instant::now()));
                        }
                        self.mods = crate::features::mods::list_mods(&self.game_root);
                    }
                    if ui.button("卸载").clicked() {
                        match crate::features::mods::uninstall_mod(&self.game_root, &m.name) {
                            Ok(()) => {
                                self.toast = Some((format!("已卸载 {}", m.name), std::time::Instant::now()));
                                self.mods = crate::features::mods::list_mods(&self.game_root);
                            }
                            Err(e) => self.toast = Some((e, std::time::Instant::now())),
                        }
                    }
                });
            }
            if self.mods.is_empty() {
                ui.label("（无）");
            }
        });
    }
}
