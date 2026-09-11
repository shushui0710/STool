//! 页面：home（P2-11 从 gui.rs 拆出）。

use crate::gui::*;
use eframe::egui::{Color32, RichText};

impl StoolApp {
    pub(crate) fn page_home(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "游戏目录检测", "识别这个游戏是用什么引擎做的，并列出判定证据");
        ui.horizontal(|ui| {
            ui.label("目录:");
            let w = ui.available_width() - 170.0;
            ui.add_sized([w.max(80.0), 22.0], egui::TextEdit::singleline(&mut self.game_root_str));
            if ui.button("浏览...").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.game_root_str = p.display().to_string();
                    self.refresh_detections();
                }
            }
            if ui.button("检测").clicked() {
                self.refresh_detections();
            }
        });
        ui.horizontal(|ui| {
            if ui
                .button("🩺 游戏体检")
                .on_hover_text("排查「游戏跑不起来」：系统区域设置 / 日文字体 / 运行库 DLL / 路径 / 写权限")
                .clicked()
            {
                self.run_health();
            }
            if ui
                .button("📦 批量检测该目录下的多个游戏")
                .on_hover_text("扫描所选目录（含子目录）下的多个游戏，逐个识别引擎并汇总（结果见日志页）")
                .clicked()
            {
                self.run_batch_detect();
            }
            if ui
                .button("🧩 封包自检")
                .on_hover_text("对游戏里的封包做「解包→重打包→逐条目比对」，确认 STool 读写无损（结果见日志页）")
                .clicked()
            {
                self.run_selfcheck();
            }
        });
        ui.add_space(10.0);
        if self.detections.is_empty() {
            ui.add_space(ui.available_height() * 0.16);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("🎮").heading());
                ui.add_space(8.0);
                ui.label(RichText::new("把游戏文件夹直接拖进本窗口，或按下面三步开始").heading());
                ui.add_space(12.0);
                ui.label(RichText::new("①  在上方输入（或点“浏览...”选择）游戏根目录").small());
                ui.label(RichText::new("②  点“检测”，STool 会识别引擎并列出判定依据").small());
                ui.label(RichText::new("③  到左侧对应页面：解包资源 · 预览 · 汉化文本 · 改存档 · 运行时修改").small());
            });
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let visuals = ui.visuals().clone();
            for (i, d) in self.detections.iter().enumerate() {
                let sel = self.selected == Some(i);
                let fill = if sel {
                    visuals.selection.bg_fill
                } else if d.ok() {
                    visuals.faint_bg_color
                } else {
                    visuals.extreme_bg_color
                };
                let stroke = if sel { visuals.selection.stroke } else { visuals.widgets.noninteractive.bg_stroke };
                let resp = egui::Frame::group(ui.style())
                    .fill(fill)
                    .stroke(stroke)
                    .show(ui, |ui| {
                        // set_max_width（而非 set_width）：卡片宽度由外层可用宽度决定，
                        // 但内容换行按"减去边框内边距后的宽度"算，避免长文本把卡片顶出屏幕。
                        ui.set_max_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{} ({})", d.name, d.plugin_id)).strong());
                            let conf = d.confidence();
                            let (mark, col) = match conf {
                                Confidence::High => ("●", Color32::from_rgb(110, 210, 110)),
                                Confidence::Medium => ("●", Color32::from_rgb(150, 200, 120)),
                                Confidence::Low => ("◐", Color32::from_rgb(225, 190, 105)),
                                Confidence::None => ("○", visuals.weak_text_color()),
                            };
                            ui.label(RichText::new(format!("{mark} {}", conf.label())).small().color(col));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(format!("{} 分", d.score)).weak().small());
                            });
                        });
                        ui.label(RichText::new(d.evidence.join("；")).small());
                        // 备注按"；"拆成多行：识别建议 + 近失提示都是长句子，
                        // 堆成一段在卡片里会横向溢出，分行后每行都短、可读。
                        for seg in d.notes.split('；').map(str::trim).filter(|s| !s.is_empty()) {
                            ui.label(RichText::new(seg).small().weak());
                        }
                    })
                    .response;
                if resp.clicked() {
                    self.selected = Some(i);
                }
                ui.add_space(4.0);
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.small_button("📂 打开游戏目录").clicked() {
                open_in_explorer(&self.game_root);
            }
            if let Some((_, dir)) = self.save_locations.first() {
                if ui.small_button("📂 打开存档目录").clicked() {
                    open_in_explorer(dir);
                }
            }
        });
        // ---- 推荐流程卡（按检测到的引擎给出下一步） ----
        if let Some(d) = self.selected_det() {
            let (flow, go_page, go_label) = if !d.ok() {
                (
                    "未确认识别结果：最高分引擎仍未达判定线。常见原因：① 目录选到了上一层或子目录（应选含主程序 exe 的那一层）；② 游戏已被别人解包/改名，特征文件缺失；③ 属于尚未收录的引擎。请对照下方各条“证据/备注”人工核对。",
                    Page::Extract,
                    "仍去解包页",
                )
            } else {
                match d.plugin_id.as_str() {
                "rpgmaker_mv" => (
                    "该游戏可直接运行时汉化：① 文本页提取 → 机翻/人工翻译 → 注入 JSON（不用改游戏文件）；改数值请用“运行时修改”页连接游戏。",
                    Page::Text,
                    "去文本汉化页",
                ),
                "renpy" => (
                    "Ren'Py 游戏：解包页反编译脚本 → 文本页提取 CSV → 机翻 → 回填；或直接写 {\"原文\":\"译文\"} JSON 注入。",
                    Page::Text,
                    "去文本汉化页",
                ),
                "kirikiri" => (
                    "KiriKiri 游戏：解包页解出数据 → 文本页提取 → 机翻 → 回填 → 解包页“封包”写回（自动备份，可用设置页“切换原版/汉化封包”）。",
                    Page::Extract,
                    "去解包页",
                ),
                "rpgmaker_rgss" => (
                    "RGSS（XP/VX/VX Ace）游戏：解包页解包 → 文本页提取 CSV → 机翻 → 回填；改数值可用“内存扫描”。",
                    Page::Text,
                    "去文本汉化页",
                ),
                "html_game" => (
                    "HTML/Electron 游戏：写 {\"原文\":\"译文\"} 的 JSON → 文本页注入，运行时替换文本。",
                    Page::Text,
                    "去文本汉化页",
                ),
                "tyrano" => (
                    "TyranoBuilder 游戏：剧本在 data/scenario/*.ks（可“文本提取 → 机翻 → 回填”），也可直接注入 JSON 做运行时 DOM 替换。",
                    Page::Text,
                    "去文本汉化页",
                ),
                "nscripter" => (
                    "NScripter 游戏：文本页提取 nscript.dat 对白 → 翻译 → 回填（自动重新加密，原文件备份）。",
                    Page::Text,
                    "去文本汉化页",
                ),
                "wolf" | "unity" | "godot" => (
                    "该引擎经外部工具（WolfDec/AssetRipper/GDRE）解包：先到“设置”页点“⬇ 下载”获取工具，再回解包页解包，之后走文本汉化流程。",
                    Page::Settings,
                    "去设置页下载工具",
                ),
                _ => (
                    "已识别引擎。左侧页面按用途排列：解包资源、文本汉化、存档编辑、MOD 补丁。",
                    Page::Extract,
                    "去解包页",
                ),
                }
            };
            ui.add_space(8.0);
            egui::Frame::group(ui.style()).fill(ui.visuals().faint_bg_color).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("💡 推荐流程").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(go_label).clicked() {
                            self.page = go_page;
                        }
                    });
                });
                ui.label(RichText::new(flow).small());
            });
        }
    }
}
