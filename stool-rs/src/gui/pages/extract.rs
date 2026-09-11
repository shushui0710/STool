//! 页面：extract（P2-11 从 gui.rs 拆出）。

use crate::gui::*;
use std::collections::{HashMap};
use eframe::egui::{RichText};

impl StoolApp {
    pub(crate) fn page_extract(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "资源解包 / 封包", "");
        let Some(d) = self.selected_det().cloned() else {
            ui.label("请先在首页完成检测。");
            return;
        };
        ui.label(format!("当前引擎: {} ({})", d.name, d.plugin_id));
        if let Some((_, detail)) = self.engine_detail.iter().find(|(id, _)| id == &d.plugin_id) {
            ui.label(RichText::new(format!("引擎详情: {detail}")).small().weak());
        }
        ui.label(format!("输出目录: {}", self.out_dir.display()));
        ui.add_space(8.0);
        let caps: Vec<Op> = self
            .registry
            .get(&d.plugin_id)
            .map(crate::engines::effective_capabilities)
            .unwrap_or_default();
        let supports_unlock = caps.contains(&Op::Unlock);
        ui.horizontal_wrapped(|ui| {
            for cap in caps {
                let enabled = matches!(cap, Op::Extract | Op::Decompile | Op::Save | Op::Unlock);
                let tip = if enabled {
                    ""
                } else if matches!(cap, Op::TextExtract | Op::TextImport | Op::TextInject) {
                    "（见文本汉化页）"
                } else {
                    "（请用 CLI 执行）"
                };
                let btn = egui::Button::new(format!("{}{}", cap.label(), tip));
                if ui.add_enabled(enabled || matches!(cap, Op::Repack), btn).clicked() {
                    let mut opts: HashMap<String, String> = HashMap::new();
                    if cap == Op::Unlock {
                        // 全 CG 解锁：默认只读预览，勾选后才真正落地
                        if self.unlock_apply {
                            opts.insert("apply".into(), "1".into());
                        }
                        let f = self.unlock_filter.trim();
                        if !f.is_empty() {
                            opts.insert("filter".into(), f.to_string());
                        }
                        if let Some(r) = self.unlock_route {
                            opts.insert("route".into(), r.key().into());
                        }
                    }
                    self.run_plugin_op(cap, opts);
                }
            }
        });
        if supports_unlock {
            ui.add_space(8.0);
            let spec = crate::features::unlock::spec_or_generic(&d.plugin_id);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                ui.label(
                    RichText::new("全 CG 解锁（统一抽象：自带存档 / 注册表 / 存档位 / 配置）")
                        .strong(),
                );
                ui.label(
                    RichText::new(format!("识别依据：{}\n解锁动作：{}", spec.basis, spec.action))
                        .small()
                        .weak(),
                );
                ui.label(
                    RichText::new(
                        "先点「解锁辅助」做只读预览，确认无误后再勾选落地。\
                         覆盖写盘前会自动备份（.stool.bak），可用「还原」回滚。",
                    )
                    .small()
                    .weak(),
                );
                ui.horizontal(|ui| {
                    ui.label("解锁手段：");
                    let cur = self.unlock_route.map(|r| r.label()).unwrap_or("自动（推荐）");
                    egui::ComboBox::from_id_salt("unlock_route")
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(self.unlock_route.is_none(), "自动（推荐）")
                                .clicked()
                            {
                                self.unlock_route = None;
                            }
                            for r in crate::features::unlock::UnlockRoute::all() {
                                let selected = self.unlock_route == Some(*r);
                                let label = format!(
                                    "{} — {}",
                                    r.label(),
                                    if r.automated() { "自动" } else { "仅指引" }
                                );
                                if ui.selectable_label(selected, label).clicked() {
                                    self.unlock_route = Some(*r);
                                }
                            }
                        });
                });
                ui.checkbox(&mut self.unlock_apply, "实际落地（取消勾选 = 仅只读预览）");
                ui.horizontal(|ui| {
                    ui.label("键名过滤（子串，可留空；注册表 / 存档位路线适用）：");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.unlock_filter)
                            .hint_text("如 CG / Scene / 回想")
                            .desired_width(180.0),
                    );
                });
            });
        }
        ui.add_space(12.0);
        ui.label(RichText::new("提示：解包结果输出到输出目录，不会改动游戏文件；封包/回写前自动备份原文件（.stool.bak）。").weak());
    }

    /// 一键导出诊断包（P1-1）：日志 + 环境（脱敏）+ 引擎检测 + 备份清单 → 单个 zip。
    pub(crate) fn start_diag_export(&mut self) {
        if self.diag_busy.load(Ordering::Relaxed) {
            self.toast = Some(("诊断包正在导出中…".into(), std::time::Instant::now()));
            return;
        }
        let out = self
            .out_dir
            .join(format!("stool_diag_{}.zip", crate::diag::stamp_compact()));
        let game = if self.game_root.as_os_str().is_empty() {
            None
        } else {
            Some(self.game_root.clone())
        };
        self.diag_busy.store(true, Ordering::Relaxed);
        self.log("▶ 开始导出诊断包".to_string());
        self.toast = Some(("正在导出诊断包…".into(), std::time::Instant::now()));
        let result = self.diag_result.clone();
        std::thread::spawn(move || {
            let r = crate::features::diagpack::export(game.as_deref(), &out)
                .map(|rep| (rep.zip_path, rep.warnings));
            if let Ok(mut slot) = result.lock() {
                *slot = Some(r);
            }
        });
    }

    /// 后台线程下载外部工具并自动写入配置。
    pub(crate) fn start_tool_download(&mut self, key: &str) {
        if self.dl_busy.load(Ordering::Relaxed) {
            self.toast = Some(("已有下载在进行中…".into(), std::time::Instant::now()));
            return;
        }
        self.dl_busy.store(true, Ordering::Relaxed);
        let proxy = self.cfg.proxy.clone();
        let key = key.to_string();
        let result = self.dl_result.clone();
        self.log(format!("▶ 开始下载外部工具: {key}"));
        std::thread::spawn(move || {
            let res = tools_dl::download(&key, &proxy, &|frac, msg| {
                let _ = frac; // 大里程碑由日志体现，避免刷屏
                if frac >= 0.99 {
                    let _ = msg;
                }
            });
            if let Ok(mut r) = result.lock() {
                *r = Some((key, res));
            }
        });
    }
}
