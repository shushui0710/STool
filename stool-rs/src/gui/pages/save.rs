//! 页面：save（P2-11 从 gui.rs 拆出）。

use crate::gui::*;
use std::path::PathBuf;
use eframe::egui::{Color32, RichText};

impl StoolApp {
    pub(crate) fn page_save(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "存档编辑器", "打开存档 → 自动识别格式 → 左侧浏览、右侧修改 → 保存回原文件（自动备份）");

        let mut doc_opt = self.save_doc.take();

        // --- 工具栏 ---
        ui.horizontal(|ui| {
            if ui.button("📂 打开存档文件...").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("存档", &["rpgsave", "rmmzsave", "rmzsave", "json", "rxdata", "rvdata", "rvdata2", "dat", "sav", "save"])
                    .add_filter("全部文件", &["*"])
                    .pick_file()
                {
                    self.save_path_str = p.display().to_string();
                    match saves::SaveDoc::load(&p) {
                        Ok(d) => {
                            self.save_hits.clear();
                            self.save_sel = None;
                            self.save_dirty = false;
                            self.save_more.clear();
                            doc_opt = Some(d);
                            self.toast = Some(("存档已加载".into(), std::time::Instant::now()));
                        }
                        Err(e) => self.toast = Some((format!("加载失败: {e}"), std::time::Instant::now())),
                    }
                }
            }
            if ui.button("按路径加载").clicked() {
                let p = PathBuf::from(self.save_path_str.trim());
                match saves::SaveDoc::load(&p) {
                    Ok(d) => {
                        self.save_hits.clear();
                        self.save_sel = None;
                        self.save_dirty = false;
                        self.save_more.clear();
                        doc_opt = Some(d);
                        self.toast = Some(("存档已加载".into(), std::time::Instant::now()));
                    }
                    Err(e) => self.toast = Some((format!("加载失败: {e}"), std::time::Instant::now())),
                }
            }
            if ui.button("🔄 重新加载").clicked() && doc_opt.is_some() {
                let p = doc_opt.as_ref().unwrap().path.clone();
                match saves::SaveDoc::load(&p) {
                    Ok(d) => {
                        self.save_hits.clear();
                        self.save_sel = None;
                        self.save_dirty = false;
                        self.save_more.clear();
                        doc_opt = Some(d);
                    }
                    Err(e) => self.toast = Some((format!("重载失败: {e}"), std::time::Instant::now())),
                }
            }
            let can_save = doc_opt.as_ref().map(|d| d.format.writable()).unwrap_or(false);
            if ui.add_enabled(can_save, egui::Button::new("💾 保存回写（自动备份）")).clicked() {
                if let Some(doc) = doc_opt.as_ref() {
                    match doc.save() {
                        Ok(m) => {
                            self.save_dirty = false;
                            self.toast = Some((m, std::time::Instant::now()));
                        }
                        Err(e) => self.toast = Some((format!("保存失败: {e}"), std::time::Instant::now())),
                    }
                }
            }
            if ui.add_enabled(doc_opt.is_some(), egui::Button::new("⬇ 导出 JSON")).clicked() {
                if let Some(doc) = doc_opt.as_ref() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).save_file() {
                        let text = serde_json::to_string_pretty(&doc.root).unwrap_or_default();
                        match std::fs::write(&p, text) {
                            Ok(()) => self.toast = Some((format!("已导出 {}", p.display()), std::time::Instant::now())),
                            Err(e) => self.toast = Some((format!("导出失败: {e}"), std::time::Instant::now())),
                        }
                    }
                }
            }
            if ui.add_enabled(can_save, egui::Button::new("⬆ 导入 JSON")).clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
                    match std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
                        Some(v) => {
                            if let Some(doc) = doc_opt.as_mut() {
                                doc.root = v;
                                self.save_dirty = true;
                                self.toast = Some(("JSON 已导入（记得保存回写）".into(), std::time::Instant::now()));
                            }
                        }
                        None => self.toast = Some(("导入失败: 不是有效的 JSON 文件".into(), std::time::Instant::now())),
                    }
                }
            }
            if self.save_dirty {
                ui.label(RichText::new("● 有未保存的修改").color(Color32::from_rgb(230, 170, 60)).small());
            }
        });

        ui.horizontal(|ui| {
            ui.label("文件:");
            let w = (ui.available_width() - 80.0).max(80.0);
            ui.add_sized([w, 20.0], egui::TextEdit::singleline(&mut self.save_path_str));
        });

        if let Some(doc) = doc_opt.as_ref() {
            let writable = if doc.format.writable() { "" } else { "（只读视图，不能修改）" };
            ui.label(RichText::new(format!("识别格式: {} {writable}", doc.format.label())).strong());
        }
        ui.add_space(4.0);

        let Some(doc) = doc_opt.as_mut() else {
            self.save_doc = doc_opt;
            ui.separator();
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("💾").heading());
                ui.label(RichText::new("把存档文件直接拖进本窗口，或点上方“打开存档文件”").heading());
                ui.add_space(4.0);
                ui.label(RichText::new("支持 RPG Maker MV / MZ 存档与 JSON 存档；改前自动备份，改坏可还原").weak().small());
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for (label, dir) in &self.save_locations {
                    ui.add_space(2.0);
                    ui.label(RichText::new(label).strong());
                    let files = saves::list_files(dir, &["rpgsave", "rmmzsave", "rmzsave", "json", "rxdata", "rvdata", "rvdata2"]);
                    for f in files.iter().take(40) {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).small());
                            if ui.small_button("打开").clicked() {
                                self.save_path_str = f.display().to_string();
                            }
                        });
                    }
                    if files.len() > 40 {
                        ui.label(RichText::new(format!("…共 {} 个文件，可在加载框输入完整路径打开", files.len())).weak().small());
                    }
                }
                if self.save_locations.is_empty() {
                    ui.label("（未发现存档目录，可直接打开任意存档文件）");
                }
            });
            return;
        };

        // --- 主区：左侧只读结构树 / 右侧搜索 + 编辑 ---
        // 布局重设计的初衷：过去把“浏览”和“编辑”挤在同一个内联树里，
        // 每个字段都要建一个 TextEdit，展开大容器时每帧布局上千个输入框 → 滚动卡死。
        // 现在树只负责“选”（纯文本行，零输入控件），编辑收敛到右侧单一面板。
        ui.add_space(4.0);
        let full_w = ui.available_width();
        let full_h = ui.available_height().max(160.0);
        let left_w = (full_w * 0.56).clamp(280.0, (full_w - 300.0).max(280.0));
        let right_w = (full_w - left_w - 18.0).max(220.0);

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left_w, full_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.save_tree_panel(ui, doc, left_w, full_h),
            );
            ui.add_space(10.0);
            ui.allocate_ui_with_layout(
                egui::vec2(right_w, full_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    self.save_search_panel(ui, doc, right_w);
                    self.save_editor_panel(ui, doc, right_w);
                },
            );
        });

        self.save_doc = doc_opt;
    }

    /// 左栏：只读结构树。点条目即选中（右侧编辑）；大容器分页展开，避免一次布局上千行。
    pub(crate) fn save_tree_panel(&mut self, ui: &mut egui::Ui, doc: &mut saves::SaveDoc, width: f32, height: f32) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("📚 存档结构").strong());
            ui.label(RichText::new("点条目 → 右侧修改").weak().small());
        });
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width((width - 14.0).max(140.0));
            egui::ScrollArea::vertical()
                .id_salt("save_tree_scroll")
                .auto_shrink([false, false])
                .max_height((height - 40.0).max(120.0))
                .show(ui, |ui| {
                    let mut budget = RenderBudget { rows: SAVE_ROW_BUDGET, hint_shown: false };
                    let clicked = render_tree(
                        ui,
                        &mut doc.root,
                        "",
                        0,
                        self.save_sel.as_deref(),
                        self.save_auto_open,
                        &mut self.save_more,
                        &mut budget,
                    );
                    if let Some(p) = clicked {
                        self.save_sel_buf = doc.get(&p).map(value_edit_text).unwrap_or_default();
                        self.save_sel = Some(p);
                    }
                });
        });
    }

    /// 右栏上半：搜索定位（大存档的主导航方式）。
    pub(crate) fn save_search_panel(&mut self, ui: &mut egui::Ui, doc: &mut saves::SaveDoc, width: f32) {
        ui.label(RichText::new("🔍 搜索定位").strong());
        let w = (width - 16.0).max(120.0);
        ui.add_sized(
            [w, 22.0],
            egui::TextEdit::singleline(&mut self.save_query).hint_text("键名或数值，如 gold / 9999 / 角色名"),
        );
        ui.horizontal(|ui| {
            ui.label(RichText::new("范围").weak().small());
            egui::ComboBox::from_id_salt("save_scope")
                .selected_text(self.save_scope.label())
                .width(92.0)
                .show_ui(ui, |ui| {
                    for sc in saves::SearchScope::ALL {
                        ui.selectable_value(&mut self.save_scope, sc, sc.label());
                    }
                });
            if ui.button("搜索").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.save_hits = doc.search(&self.save_query, self.save_scope);
                if self.save_hits.is_empty() {
                    self.toast = Some(("没有找到匹配项".into(), std::time::Instant::now()));
                }
            }
            if !self.save_hits.is_empty() && ui.button("清空").clicked() {
                self.save_hits.clear();
            }
        });
        if self.save_hits.is_empty() {
            ui.label(RichText::new("可搜键名或数值，回车即搜；大存档建议先搜再改。").weak().small());
            return;
        }
        ui.label(RichText::new(format!("找到 {} 处，点一行即定位：", self.save_hits.len())).small());
        egui::ScrollArea::vertical()
            .id_salt("save_hits")
            .max_height(150.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut jump: Option<String> = None;
                for p in self.save_hits.iter() {
                    let val_text = doc.get(p).map(value_edit_text).unwrap_or_default();
                    let row = format!("{}  =  {}", p, short_text(&val_text, 34));
                    let selected = self.save_sel.as_deref() == Some(p.as_str());
                    if ui
                        .add(egui::SelectableLabel::new(selected, RichText::new(row).monospace().small()))
                        .clicked()
                    {
                        jump = Some(p.clone());
                    }
                }
                if let Some(p) = jump {
                    self.save_sel_buf = doc.get(&p).map(value_edit_text).unwrap_or_default();
                    self.save_sel = Some(p);
                }
            });
    }

    /// 右栏下半：选中字段的编辑面板（路径 / 类型 / 当前值 / 新值 / 快捷操作）。
    pub(crate) fn save_editor_panel(&mut self, ui: &mut egui::Ui, doc: &mut saves::SaveDoc, width: f32) {
        ui.add_space(8.0);
        ui.separator();
        let Some(sel) = self.save_sel.clone() else {
            ui.add_space(4.0);
            ui.label(RichText::new("在左侧结构树或上方结果里点一个条目，这里会出现可编辑详情。").weak().small());
            return;
        };
        let writable = doc.format.writable();
        let cur_val = doc.get(&sel).cloned().unwrap_or(serde_json::Value::Null);
        let ty = type_hint(&cur_val);
        let cur_text = value_edit_text(&cur_val);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width((width - 28.0).max(160.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new("✏ 修改字段").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕ 关闭").clicked() {
                        self.save_sel = None;
                    }
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("路径").weak().small());
                ui.label(RichText::new(sel.clone()).monospace().small());
            });
            ui.label(RichText::new(format!("类型 {ty}　当前值 {}", short_text(&cur_text, 46))).weak().small());
            if !writable {
                ui.label(RichText::new("⚠ 该格式为只读视图，不能修改").color(Color32::from_rgb(230, 170, 60)).small());
            }
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 76.0).max(80.0);
                let resp = ui.add_enabled(
                    writable,
                    egui::TextEdit::singleline(&mut self.save_sel_buf)
                        .desired_width(w)
                        .hint_text("新值（回车应用）"),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add_enabled(writable, egui::Button::new("应用")).clicked() || enter {
                    self.apply_save_edit(doc, &sel);
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("快捷").weak().small());
                if ui.add_enabled(writable, egui::Button::new("重置")).clicked() {
                    self.save_sel_buf = cur_text.clone();
                }
                match &cur_val {
                    serde_json::Value::Bool(b) => {
                        let next = !*b;
                        if ui.add_enabled(writable, egui::Button::new(format!("设为 {next}"))).clicked() {
                            self.save_sel_buf = next.to_string();
                            self.apply_save_edit(doc, &sel);
                        }
                    }
                    serde_json::Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            if ui.add_enabled(writable, egui::Button::new("−1")).clicked() {
                                self.save_sel_buf = (i - 1).to_string();
                                self.apply_save_edit(doc, &sel);
                            }
                            if ui.add_enabled(writable, egui::Button::new("+1")).clicked() {
                                self.save_sel_buf = (i + 1).to_string();
                                self.apply_save_edit(doc, &sel);
                            }
                        }
                    }
                    _ => {}
                }
            });
        });
    }

    /// 把右栏编辑框里的新值写到选中路径。
    pub(crate) fn apply_save_edit(&mut self, doc: &mut saves::SaveDoc, sel: &str) {
        match doc.set(sel, parse_edit_text(&self.save_sel_buf)) {
            Ok(()) => {
                self.save_dirty = true;
                self.toast = Some(("已修改（记得保存回写）".into(), std::time::Instant::now()));
            }
            Err(e) => self.toast = Some((format!("修改失败: {e}"), std::time::Instant::now())),
        }
    }
}
