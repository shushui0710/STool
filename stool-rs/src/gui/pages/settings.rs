//! 页面：settings（P2-11 从 gui.rs 拆出）。

use crate::gui::*;
use std::sync::atomic::{Ordering};
use eframe::egui::{RichText};

impl StoolApp {
    pub(crate) fn page_settings(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "设置", "外部工具路径与代理；现在可以一键自动下载并配置");
        let dl_busy = self.dl_busy.load(Ordering::Relaxed);
        ui.label(RichText::new("不用自己找工具：点“⬇ 下载”会自动从官方 GitHub 下载到 ~/.stool/tools/ 并填好路径（走下方代理）。").weak());
        ui.add_space(4.0);
        let mut dl_key: Option<String> = None;
        {
            let cfg = &mut self.cfg;
            for (label, key) in [
                ("WolfDec.exe（Wolf 解包）", "wolfdec"),
                ("AssetRipper CLI（Unity 解包）", "asset_ripper"),
                ("GARbro CLI（未知格式兜底）", "garbro"),
                ("GDRE Tools（Godot 反编译）", "gdre_tools"),
                ("unrpyc.py（Ren'Py 反编译）", "unrpyc"),
                ("Python 解释器（外挂用）", "python"),
                ("代理", "proxy"),
            ] {
                let has_dl = tools_dl::spec_by_key(key).is_some();
                ui.label(RichText::new(label).weak().small());
                ui.horizontal(|ui| {
                    let v = match key {
                        "wolfdec" => &mut cfg.wolfdec,
                        "asset_ripper" => &mut cfg.asset_ripper,
                        "garbro" => &mut cfg.garbro,
                        "gdre_tools" => &mut cfg.gdre_tools,
                        "unrpyc" => &mut cfg.unrpyc,
                        "python" => &mut cfg.python,
                        _ => &mut cfg.proxy,
                    };
                    let w = ui.available_width() - if has_dl { 96.0 } else { 12.0 };
                    ui.add_sized([w.max(200.0), 22.0], egui::TextEdit::singleline(v));
                    if has_dl && ui.add_enabled(!dl_busy, egui::Button::new("⬇ 下载")).clicked() {
                        dl_key = Some(key.to_string());
                    }
                });
                ui.add_space(4.0);
            }
        }
        if dl_busy {
            ui.label(RichText::new("⏳ 正在后台下载（大工具可能要一两分钟，完成后会提示）…").small());
        }
        if let Some(key) = dl_key {
            self.start_tool_download(&key);
        }
        ui.add_space(8.0);
        if ui.button("保存设置").clicked() {
            match crate::settings::save(&self.cfg) {
                Ok(()) => {
                    self.settings_saved_at = Some(std::time::Instant::now());
                    self.toast = Some(("设置已保存".into(), std::time::Instant::now()));
                }
                Err(e) => self.toast = Some((format!("保存失败: {e}"), std::time::Instant::now())),
            }
        }
        if let Some(t) = self.settings_saved_at {
            ui.label(RichText::new(format!("已保存（{:?} 前）", t.elapsed())).weak().small());
        }
        ui.add_space(12.0);
        ui.separator();
        ui.label(RichText::new("备份与还原").strong());
        ui.label(RichText::new(
            "所有会改写游戏文件的操作（回填/封包/注入/存档回写）都会先在原文件旁留 .stool.bak 备份；\
             对 repack 过的封包（xp3/pck/asar/rpa）可一键在 原版/汉化 之间切换，无需重新封包。",
        ).weak().small());
        ui.horizontal(|ui| {
            if ui.button("🔍 扫描游戏目录备份").clicked() {
                self.bak_list = crate::features::restore::find_backups(&self.game_root);
                self.bak_scan_done = true;
            }
            if ui.button("⇄ 切换 原版/汉化 封包").clicked() {
                match crate::features::restore::archive_toggle(&self.game_root) {
                    Ok(m) => self.toast = Some((m, std::time::Instant::now())),
                    Err(e) => self.toast = Some((format!("切换失败: {e}"), std::time::Instant::now())),
                }
            }
            if self.bak_scan_done {
                ui.label(RichText::new(format!("共 {} 个备份", self.bak_list.len())).weak().small());
            }
        });
        if self.bak_scan_done {
            if self.bak_list.is_empty() {
                ui.label(RichText::new("未发现 .stool.bak 备份（做过回填/封包/注入后会出现）").weak().small());
            } else {
                egui::ScrollArea::vertical().max_height(180.0).auto_shrink([false, false]).show(ui, |ui| {
                    let mut restore_idx: Option<usize> = None;
                    for (i, bak) in self.bak_list.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(bak.display().to_string()).small());
                            if ui.small_button("还原").clicked() {
                                restore_idx = Some(i);
                            }
                        });
                    }
                    if let Some(i) = restore_idx {
                        let bak = self.bak_list[i].clone();
                        match crate::features::restore::restore_one(&bak) {
                            Ok(m) => self.toast = Some((m, std::time::Instant::now())),
                            Err(e) => self.toast = Some((format!("还原失败: {e}"), std::time::Instant::now())),
                        }
                        self.bak_list = crate::features::restore::find_backups(&self.game_root);
                    }
                });
            }
        }
        ui.add_space(12.0);
        ui.separator();
        ui.label(RichText::new("外部工具说明").strong());
        ui.label("推荐直接点上方“⬇ 下载”按钮：工具会从官方 GitHub Release 自动下载、解压到 ~/.stool/tools/ 并自动填好路径。下载不动时检查上方代理设置。手动安装的话：把可执行文件完整路径填到上方保存即可。");
    }
}
