//! 页面：save（P2-11 从 gui.rs 拆出；本轮按「搜索优先」重设计）。
//!
//! ## 为什么要改布局
//! 存档编辑的真实流程是「**找到字段 → 改值**」，不是「在 JSON 树里逐层点」。
//! 旧版把「只读结构树」摆在左半边（占屏 56%）：
//!   * 对只想改个金币的玩家来说，它是一大块**看不出用途**、带滚动条、又密又长的区域；
//!   * 展开 `switches` / `variables`（RPG Maker 存档常见上千项）后每帧要布局上千行，
//!     滚动明显发涩；
//!   * 11 号等宽小字在长列表里可读性差。
//!
//! 现在：
//!   ① 搜索 = 主视图，结果是**虚拟化列表**（只布局可见行，几千条也不卡），
//!      行内「路径 + 值 + 类型」三色分栏，点一行即选中；
//!   ② 编辑卡片紧跟在搜索下面，改完只重算这一行缓存；
//!   ③ 原始 JSON 结构降级为**底部默认折叠**的辅助视图，标题里写明用途与渲染上限。

use crate::gui::*;
use std::path::PathBuf;
use eframe::egui::RichText;

/// 搜索结果列表的单行高度（虚拟化渲染要按固定行高算视口）。
const HIT_ROW_H: f32 = 22.0;
/// 结果列表的最大高度（再高只是浪费竖向空间）。
const HIT_LIST_H: f32 = 300.0;
/// 原始结构树展开后的固定高度（辅助视图，不该占满整屏）。
const RAW_TREE_H: f32 = 340.0;
/// 结果行 / 结构树行的等宽字号。
const ROW_FONT: f32 = 12.0;

impl StoolApp {
    pub(crate) fn page_save(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "存档编辑器", "打开存档 → 搜索字段 → 改值 → 保存回原文件（改前自动备份）");

        let mut doc_opt = self.save_doc.take();
        self.save_toolbar(ui, &mut doc_opt);

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("文件").weak().small());
            let w = (ui.available_width() - 12.0).max(80.0);
            ui.add_sized(
                [w, 22.0],
                egui::TextEdit::singleline(&mut self.save_path_str).hint_text("存档文件完整路径（也可把文件直接拖进窗口）"),
            );
        });

        let Some(doc) = doc_opt.as_mut() else {
            self.save_doc = doc_opt;
            self.save_empty_state(ui);
            return;
        };

        let writable = doc.format.writable();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("格式：{}", doc.format.label())).strong().small());
            if !writable {
                ui.label(RichText::new("只读视图（该格式不支持回写）").color(C_WARN).small());
            }
            if self.save_dirty {
                ui.label(RichText::new("● 有未保存的修改").color(C_WARN).small());
            }
        });
        ui.add_space(6.0);

        // ① 搜索（主入口）
        self.save_search_card(ui, doc);
        ui.add_space(6.0);
        // ② 选中字段的编辑卡片
        self.save_editor_card(ui, doc, writable);
        ui.add_space(6.0);
        // ③ 原始结构（辅助视图，默认折叠）
        self.save_raw_tree(ui, doc);

        self.save_doc = doc_opt;
    }

    /// 顶部工具栏：打开 / 重载 / 保存 / 导入导出 JSON。
    fn save_toolbar(&mut self, ui: &mut egui::Ui, doc_opt: &mut Option<saves::SaveDoc>) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("📂 打开存档文件...").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("存档", &["rpgsave", "rmmzsave", "rmzsave", "json", "rxdata", "rvdata", "rvdata2", "dat", "sav", "save"])
                    .add_filter("全部文件", &["*"])
                    .pick_file()
                {
                    self.save_path_str = p.display().to_string();
                    self.save_apply_loaded(saves::SaveDoc::load(&p), doc_opt);
                }
            }
            if ui.button("按路径加载").clicked() {
                let p = PathBuf::from(self.save_path_str.trim());
                self.save_apply_loaded(saves::SaveDoc::load(&p), doc_opt);
            }
            if ui.button("🔄 重新加载").clicked() {
                if let Some(d) = doc_opt.as_ref() {
                    let p = d.path.clone();
                    self.save_apply_loaded(saves::SaveDoc::load(&p), doc_opt);
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
        });
    }

    /// 统一处理「加载完成」：清空上一次的搜索结果 / 选中 / 分页状态。
    fn save_apply_loaded(&mut self, res: Result<saves::SaveDoc, String>, doc_opt: &mut Option<saves::SaveDoc>) {
        match res {
            Ok(d) => {
                self.save_hits.clear();
                self.save_rows.clear();
                self.save_sel = None;
                self.save_sel_buf.clear();
                self.save_dirty = false;
                self.save_more.clear();
                *doc_opt = Some(d);
                self.toast = Some(("存档已加载".into(), std::time::Instant::now()));
            }
            Err(e) => self.toast = Some((format!("加载失败: {e}"), std::time::Instant::now())),
        }
    }

    /// ① 搜索卡片：搜索框 + 虚拟化结果列表（点一行即选中）。
    fn save_search_card(&mut self, ui: &mut egui::Ui, doc: &saves::SaveDoc) {
        card(ui, |ui| {
            card_title(ui, "🔍 搜索定位", "键名或数值都行，回车即搜；点一行即选中，下面就能改");
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 236.0).max(150.0);
                let resp = ui.add_sized(
                    [w, 24.0],
                    egui::TextEdit::singleline(&mut self.save_query).hint_text("如 gold / 9999 / 角色名"),
                );
                ui.label(RichText::new("范围").weak().small());
                egui::ComboBox::from_id_salt("save_scope")
                    .selected_text(self.save_scope.label())
                    .width(96.0)
                    .show_ui(ui, |ui| {
                        for sc in saves::SearchScope::ALL {
                            ui.selectable_value(&mut self.save_scope, sc, sc.label());
                        }
                    });
                // 回车提交：只在输入框刚失焦时判定，避免在别的框里打字也触发搜索
                let submitted = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("搜索").clicked() || submitted {
                    self.save_hits = doc.search(&self.save_query, self.save_scope);
                    self.save_rows = save_rows_of(doc, &self.save_hits);
                    if self.save_hits.is_empty() {
                        self.toast = Some(("没有找到匹配项".into(), std::time::Instant::now()));
                    }
                }
                if !self.save_rows.is_empty() && ui.button("清空").clicked() {
                    self.save_hits.clear();
                    self.save_rows.clear();
                }
            });

            if self.save_rows.is_empty() {
                ui.add_space(2.0);
                ui.label(
                    RichText::new("结果会显示在这里。大存档（几千个 switches / variables）建议先搜再改，比在结构树里翻快得多。")
                        .weak()
                        .small(),
                );
                return;
            }

            ui.label(
                RichText::new(format!("找到 {} 处（列表只渲染可见行，滚动不卡）", self.save_rows.len()))
                    .weak()
                    .small(),
            );
            let dark = ui.visuals().dark_mode;
            let total = self.save_rows.len();
            let mut jump: Option<String> = None;
            egui::ScrollArea::vertical()
                .id_salt("save_hits")
                .auto_shrink([false, false])
                .max_height(HIT_LIST_H)
                .show_rows(ui, HIT_ROW_H, total, |ui, range| {
                    for i in range {
                        let Some(row) = self.save_rows.get(i) else { continue };
                        let selected = self.save_sel.as_deref() == Some(row.path.as_str());
                        let strong = ui.visuals().strong_text_color();
                        let weak = ui.visuals().weak_text_color();
                        let vcolor = if selected { strong } else { val_kind_color(dark, row.kind) };
                        let path_color = if selected { strong } else { weak };
                        ui.horizontal(|ui| {
                            let row_w = (ui.available_width() - 52.0).max(80.0);
                            let font = egui::FontId::monospace(ROW_FONT);
                            let mut job = egui::text::LayoutJob::default();
                            job.append(&short_text(&row.path, 52), 0.0, egui::TextFormat { font_id: font.clone(), color: path_color, ..Default::default() });
                            job.append("   ", 0.0, egui::TextFormat { font_id: font.clone(), color: weak, ..Default::default() });
                            job.append(&row.preview, 0.0, egui::TextFormat { font_id: font, color: vcolor, ..Default::default() });
                            let resp = ui.add_sized([row_w, HIT_ROW_H - 2.0], egui::SelectableLabel::new(selected, job));
                            let resp = resp.on_hover_ui(|ui| {
                                ui.label(RichText::new(&row.path).monospace().small());
                                ui.label(RichText::new(format!("类型：{}", row.ty)).small());
                            });
                            if resp.clicked() {
                                jump = Some(row.path.clone());
                            }
                            ui.label(RichText::new(row.ty).weak().small());
                        });
                    }
                });
            if let Some(p) = jump {
                self.save_hit_select(doc, &p);
            }
        });
    }

    /// 选中某个字段：同步编辑框内容。
    fn save_hit_select(&mut self, doc: &saves::SaveDoc, path: &str) {
        self.save_sel_buf = doc.get(path).map(value_edit_text).unwrap_or_default();
        self.save_sel = Some(path.to_string());
    }

    /// ② 编辑卡片：路径 / 类型 / 当前值 / 新值 + 快捷操作。
    fn save_editor_card(&mut self, ui: &mut egui::Ui, doc: &mut saves::SaveDoc, writable: bool) {
        let Some(sel) = self.save_sel.clone() else {
            card(ui, |ui| {
                card_title(ui, "✏ 修改字段", "");
                ui.label(
                    RichText::new("还没有选中字段：在上面搜索结果里点一行（或在「原始结构」里点一个条目）。")
                        .weak()
                        .small(),
                );
            });
            return;
        };
        let cur_val = doc.get(&sel).cloned().unwrap_or(serde_json::Value::Null);
        let ty = type_hint(&cur_val);
        let cur_text = value_edit_text(&cur_val);
        let dark = ui.visuals().dark_mode;

        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("✏ 修改字段").strong());
                ui.label(RichText::new(format!("类型 {ty}")).weak().small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕ 取消选中").clicked() {
                        self.save_sel = None;
                    }
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("路径").weak().small());
                ui.label(RichText::new(sel.clone()).monospace().small());
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("当前值").weak().small());
                ui.label(
                    RichText::new(short_text(&cur_text, 70))
                        .monospace()
                        .color(val_kind_color(dark, val_kind(&cur_val)))
                        .small(),
                );
            });
            if !writable {
                ui.label(RichText::new("⚠ 该格式为只读视图，不能修改").color(C_WARN).small());
            }
            ui.add_space(2.0);
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

    /// ③ 原始结构：辅助视图（排错 / 精确定位），**默认折叠**，展开后固定高度。
    fn save_raw_tree(&mut self, ui: &mut egui::Ui, doc: &mut saves::SaveDoc) {
        let top = container_len(&doc.root);
        egui::CollapsingHeader::new(RichText::new(format!("🗂 原始 JSON 结构（辅助，{top} 个顶层字段）")).strong())
            .id_salt("save_raw_tree")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "存档的原始结构，用于排错或精确定位；**日常改数值请用上面的搜索**。\
                         为保持流畅，一帧最多渲染 600 行，超出请折叠分支或改用搜索。",
                    )
                    .weak()
                    .small(),
                );
                ui.add_space(2.0);
                egui::ScrollArea::vertical()
                    .id_salt("save_tree_scroll")
                    .auto_shrink([false, false])
                    .max_height(RAW_TREE_H)
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

    /// 未加载存档时的空状态：列出检测到的存档位置（文件列表**缓存**，不每帧读盘）。
    fn save_empty_state(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("💾").heading());
            ui.label(RichText::new("把存档文件拖进本窗口，或点上方「打开存档文件」").strong());
            ui.add_space(2.0);
            ui.label(RichText::new("支持 RPG Maker MV / MZ 存档与 JSON 存档；改前自动备份，改坏可还原").weak().small());
        });
        ui.add_space(8.0);
        self.save_locs_sync();
        card(ui, |ui| {
            card_title(ui, "📂 检测到的存档位置", "来自首页的游戏目录检测；点「打开」即填入路径");
            if self.save_locs_files.is_empty() {
                ui.label(
                    RichText::new("还没发现存档目录：可以回首页检测一下游戏目录（多数存档在「文档」或游戏目录下），或直接打开任意存档文件。")
                        .weak()
                        .small(),
                );
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("save_locs")
                .auto_shrink([false, false])
                .max_height(220.0)
                .show(ui, |ui| {
                    for (label, files) in &self.save_locs_files {
                        ui.add_space(2.0);
                        ui.label(RichText::new(label).strong().small());
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
                });
        });
    }

    /// 确认「存档位置 → 文件列表」缓存是否与当前检测结果一致，不一致才重建。
    ///
    /// 旧实现在渲染循环里直接调 `list_files`：**每帧都读一次盘**，存档目录多/文件多时
    /// 空状态页自己就会卡。这里按「位置清单」做一次指纹，只有变了才重新扫描。
    fn save_locs_sync(&mut self) {
        let key = self
            .save_locations
            .iter()
            .map(|(l, d)| format!("{l}|{}", d.display()))
            .collect::<Vec<_>>()
            .join(";");
        if key == self.save_locs_key {
            return;
        }
        let exts = ["rpgsave", "rmmzsave", "rmzsave", "json", "rxdata", "rvdata", "rvdata2"];
        self.save_locs_files = self
            .save_locations
            .iter()
            .map(|(label, dir)| (label.clone(), saves::list_files(dir, &exts)))
            .collect();
        self.save_locs_key = key;
    }

    /// 把编辑框里的新值写到选中路径；只重算这一行的结果缓存。
    pub(crate) fn apply_save_edit(&mut self, doc: &mut saves::SaveDoc, sel: &str) {
        match doc.set(sel, parse_edit_text(&self.save_sel_buf)) {
            Ok(()) => {
                self.save_dirty = true;
                refresh_save_row(doc, &mut self.save_rows, sel);
                self.toast = Some(("已修改（记得保存回写）".into(), std::time::Instant::now()));
            }
            Err(e) => self.toast = Some((format!("修改失败: {e}"), std::time::Instant::now())),
        }
    }
}
