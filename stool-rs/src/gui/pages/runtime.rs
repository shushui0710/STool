//! 页面：runtime（P2-11 从 gui.rs 拆出）。

use crate::gui::*;
use crate::features::precheck::human_bytes;
use crate::features::xp3patch;
use std::path::PathBuf;
use std::sync::{Arc};
use std::sync::atomic::{AtomicBool, Ordering};
use eframe::egui::{Color32, RichText};

impl StoolApp {
    pub(crate) fn page_runtime(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "运行时修改", "游戏开着就能改，改了立即生效");

        // ============ 方式一：MV/MZ 调试协议 ============
        card(ui, |ui| {
            ui.label(RichText::new("方式一：RPG Maker MV / MZ 游戏（按名字精确改金币/变量/开关/物品）").strong());
            ui.horizontal(|ui| {
                ui.label("游戏程序:");
                let w = (ui.available_width() - 320.0).max(80.0);
                ui.add_sized([w, 22.0], egui::TextEdit::singleline(&mut self.rt_exe_str));
                if ui.button("浏览...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("游戏程序", &["exe"]).pick_file() {
                        self.rt_exe_str = p.display().to_string();
                    }
                }
                ui.label("端口:");
                ui.add_sized([52.0, 22.0], egui::TextEdit::singleline(&mut self.rt_port_str));
            });
            ui.horizontal(|ui| {
                if ui.button("▶ 以调试模式启动游戏").clicked() {
                    let exe = PathBuf::from(self.rt_exe_str.trim());
                    match self.rt_port_str.trim().parse::<u16>() {
                        Ok(port) => match runtime::DebugGame::launch(&exe, port) {
                            Ok(pid) => self.rt_msg = format!("游戏已启动（进程 {pid}），等游戏窗口出现后点“连接游戏”。"),
                            Err(e) => self.toast = Some((e, std::time::Instant::now())),
                        },
                        Err(_) => self.toast = Some(("端口要是 0-65535 的数字".into(), std::time::Instant::now())),
                    }
                }
                let connecting = self.rt_connecting.load(Ordering::Relaxed);
                if ui.add_enabled(!connecting && self.rt_game.is_none(), egui::Button::new(if connecting { "连接中…" } else { "🔌 连接游戏" })).clicked() {
                    match self.rt_port_str.trim().parse::<u16>() {
                        Ok(port) => self.rt_start_connect(port),
                        Err(_) => self.toast = Some(("端口要是 0-65535 的数字".into(), std::time::Instant::now())),
                    }
                }
                if self.rt_game.is_some() && ui.button("🔄 刷新数据").clicked() {
                    if let Some(g) = self.rt_game.as_mut() {
                        match g.read_state() {
                            Ok(s) => {
                                self.rt_state = s;
                                self.rt_names = g.read_names().unwrap_or(self.rt_names.take());
                                self.rt_msg = "已刷新。".into();
                            }
                            Err(e) => {
                                self.rt_msg = format!("连接已断开: {e}");
                                self.rt_game = None;
                                self.rt_state = serde_json::Value::Null;
                            }
                        }
                    }
                }
                if self.rt_game.is_some() && ui.button("断开").clicked() {
                    self.rt_game = None;
                    self.rt_state = serde_json::Value::Null;
                    self.rt_msg = "已断开。".into();
                }
            });
            if !self.rt_msg.is_empty() {
                ui.label(RichText::new(&self.rt_msg).small());
            }

            if let Some(g) = self.rt_game.as_mut() {
                let ready = self.rt_state.get("mv").and_then(|v| v.as_bool()).unwrap_or(false);
                if ready {
                    let gold = self.rt_state.get("gold").and_then(|v| v.as_i64()).unwrap_or(0);
                    ui.label(format!("当前金币: {gold}"));
                    ui.add_space(4.0);

                    // 快捷编辑器
                    ui.horizontal(|ui| {
                        ui.label("修改:");
                        egui::ComboBox::from_id_salt("rt_mode")
                            .selected_text(["金币", "变量", "开关", "物品数量"][self.rt_mode])
                            .width(90.0)
                            .show_ui(ui, |ui| {
                                for (i, m) in ["金币", "变量", "开关", "物品数量"].iter().enumerate() {
                                    ui.selectable_value(&mut self.rt_mode, i, *m);
                                }
                            });
                        if self.rt_mode != 0 {
                            ui.label("编号 ID:");
                            ui.add_sized([60.0, 22.0], egui::TextEdit::singleline(&mut self.rt_id_str));
                        }
                        ui.label("新值:");
                        if self.rt_mode == 2 {
                            ui.label(RichText::new("（true=开 / false=关）").weak().small());
                        }
                        ui.add_sized([120.0, 22.0], egui::TextEdit::singleline(&mut self.rt_val_str));
                        if ui.button("✅ 写入游戏").clicked() {
                            let res = match self.rt_mode {
                                0 => self.rt_val_str.trim().parse::<i64>().map_err(|e| format!("金币要是数字: {e}")).and_then(|n| g.set_gold(n).map(|v| format!("金币已改为 {v}"))),
                                1 => match self.rt_id_str.trim().parse::<i64>() {
                                    Ok(id) => g.set_variable(id, &parse_edit_text(&self.rt_val_str)).map(|v| format!("变量 {id} 已改为 {v}")),
                                    Err(_) => Err("变量 ID 要是数字".into()),
                                },
                                2 => match self.rt_id_str.trim().parse::<i64>() {
                                    Ok(id) => {
                                        let on = matches!(self.rt_val_str.trim(), "true" | "开" | "1" | "on" | "TRUE" | "True");
                                        g.set_switch(id, on).map(|v| format!("开关 {id} 已改为 {v}"))
                                    }
                                    Err(_) => Err("开关 ID 要是数字".into()),
                                },
                                _ => match self.rt_id_str.trim().parse::<i64>() {
                                    Ok(id) => match self.rt_val_str.trim().parse::<i64>() {
                                        Ok(n) => g.set_item(id, n).map(|v| format!("物品 {id} 已改为 {v} 个")),
                                        Err(_) => Err("物品数量要是数字".into()),
                                    },
                                    Err(_) => Err("物品 ID 要是数字".into()),
                                },
                            };
                            match res {
                                Ok(m) => {
                                    if let Ok(s) = g.read_state() {
                                        self.rt_state = s;
                                    }
                                    self.toast = Some((m, std::time::Instant::now()));
                                }
                                Err(e) => self.toast = Some((format!("写入失败: {e}"), std::time::Instant::now())),
                            }
                        }
                    });

                    // 变量 / 物品一览
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, false]).show(ui, |ui| {
                        if let Some(vars) = self.rt_state.get("variables").and_then(|v| v.as_object()) {
                            ui.label(RichText::new(format!("游戏变量（共 {} 个，点“选”填入上方编辑器）:", vars.len())).strong());
                            let mut ids: Vec<(i64, &serde_json::Value)> = vars
                                .iter()
                                .filter_map(|(k, v)| k.parse::<i64>().ok().map(|id| (id, v)))
                                .filter(|(_, v)| !v.is_null())
                                .collect();
                            ids.sort_by_key(|(id, _)| *id);
                            for (id, v) in ids.iter().take(400) {
                                let nm = json_name(&self.rt_names, "vars", *id);
                                let title = if nm.is_empty() { format!("#{id}") } else { format!("#{id} {nm}") };
                                ui.horizontal(|ui| {
                                    ui.add_sized([170.0, 18.0], egui::Label::new(RichText::new(short_text(&title, 26)).monospace()).truncate());
                                    ui.add(egui::Label::new(RichText::new(short_text(&value_edit_text(v), 60)).monospace()).truncate());
                                    if ui.small_button("选").clicked() {
                                        self.rt_mode = 1;
                                        self.rt_id_str = id.to_string();
                                        self.rt_val_str = value_edit_text(v);
                                    }
                                });
                            }
                            if ids.len() > 400 {
                                ui.label(RichText::new(format!("…其余 {} 个省略，直接输 ID 改", ids.len() - 400)).weak().small());
                            }
                        }
                        if let Some(items) = self.rt_state.get("items").and_then(|v| v.as_object()) {
                            ui.label(RichText::new(format!("持有物品（{} 种）:", items.len())).strong());
                            let mut its: Vec<(i64, i64)> = items
                                .iter()
                                .filter_map(|(k, v)| Some((k.parse::<i64>().ok()?, v.as_i64()?)))
                                .collect();
                            its.sort();
                            for (id, n) in its {
                                let nm = json_name(&self.rt_names, "items", id);
                                let title = if nm.is_empty() { format!("物品 #{id} × {n}") } else { format!("{nm} × {n}") };
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(title).monospace());
                                    if ui.small_button("选").clicked() {
                                        self.rt_mode = 3;
                                        self.rt_id_str = id.to_string();
                                        self.rt_val_str = n.to_string();
                                    }
                                });
                            }
                        }
                    });
                } else {
                    ui.label(RichText::new("游戏核心还没加载：请先进入游戏标题或读一个存档，然后点“刷新数据”。").weak());
                }
            } else {
                ui.label(RichText::new("MV/MZ 游戏在首页检测通过后，进入本页会自动启动并连接（游戏会自动弹出，属正常现象）。如果自动连接失败（比如游戏已经手动开着），请先关掉游戏，再点上面的按钮重试。其他引擎的游戏请用下面的方式二。").weak());
            }
        });

        ui.add_space(8.0);

        // ============ 方式二：通用内存扫描 ============
        card(ui, |ui| {
            ui.label(RichText::new("方式二：通用内存扫描（任何游戏都能用，替代 Cheat Engine）").strong());
            ui.label(RichText::new("用法和 Cheat Engine 一样：先记住当前数值（比如金币 100）→ 首次扫描 → 回游戏让数值变化 → 再次扫描过滤 → 剩下的地址里写入新值。").weak());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("🔄 刷新进程列表").clicked() {
                    self.ms_procs = memscan::list_processes();
                    self.toast = Some((format!("共 {} 个进程", self.ms_procs.len()), std::time::Instant::now()));
                }
                ui.label("筛选:");
                ui.add_sized([140.0, 22.0], egui::TextEdit::singleline(&mut self.ms_proc_filter));
            });
            ui.horizontal(|ui| {
                ui.label("目标进程:");
                let filtered: Vec<&(u32, String)> = self
                    .ms_procs
                    .iter()
                    .filter(|(_, n)| self.ms_proc_filter.trim().is_empty() || n.to_lowercase().contains(&self.ms_proc_filter.trim().to_lowercase()))
                    .take(400)
                    .collect();
                // 选中结果先记下来，出了 ComboBox 闭包再改状态（闭包内借用着 ms_procs，不能动 &mut self）
                let mut picked: Option<(u32, String)> = None;
                egui::ComboBox::from_id_salt("ms_proc")
                    .selected_text(&self.ms_pid_label)
                    .width(320.0)
                    .show_ui(ui, |ui| {
                        for (pid, name) in filtered {
                            let label = format!("{pid} — {name}");
                            if ui.selectable_label(self.ms_pid == *pid, &label).clicked() {
                                picked = Some((*pid, label));
                            }
                        }
                    });
                if let Some((pid, label)) = picked {
                    self.ms_pid = pid;
                    self.ms_pid_label = label;
                    // 选中即建立扫描会话：否则「首次扫描」只会报「请先选择目标进程」
                    self.ms_open_scanner();
                }
            });
            ui.horizontal(|ui| {
                ui.label("数值类型:");
                egui::ComboBox::from_id_salt("ms_ty")
                    .selected_text(self.ms_ty.label())
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for t in memscan::ScanType::ALL {
                            if ui.selectable_label(self.ms_ty == t, t.label()).clicked() && self.ms_ty != t {
                                self.ms_ty = t;
                                // 类型变了：旧会话（含命中）作废、锁定停掉，再按新类型重开会话
                                self.ms_open_scanner();
                            }
                        }
                    });
                ui.label("数值:");
                ui.add(egui::TextEdit::singleline(&mut self.ms_value).desired_width(140.0));
                let busy = self.ms_sh.lock().map(|s| s.busy).unwrap_or(false);
                if ui.add_enabled(!busy && self.ms_pid != 0, egui::Button::new("🔎 首次扫描")).clicked() {
                    self.ms_start_scan(true);
                }
                ui.label("过滤:");
                egui::ComboBox::from_id_salt("ms_filter")
                    .selected_text(self.ms_filter.label())
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for f in memscan::Filter::ALL {
                            ui.selectable_value(&mut self.ms_filter, f, f.label());
                        }
                    });
                if ui.add_enabled(!busy && self.ms_pid != 0, egui::Button::new("再次扫描")).clicked() {
                    self.ms_start_scan(false);
                }
            });
            ui.horizontal(|ui| {
                let busy = self.ms_sh.lock().map(|s| s.busy).unwrap_or(false);
                ui.label(RichText::new("扫描完成后:").weak());
                if ui.add_enabled(!busy, egui::Button::new("✅ 写入所有命中")).clicked() {
                    self.ms_write_all();
                }
                if ui.add_enabled(!busy && self.ms_pid != 0, egui::Button::new(if self.ms_frozen { "🔓 解锁数值" } else { "🔒 锁定数值" })).clicked() {
                    self.ms_toggle_freeze();
                }
                if ui.add_enabled(!busy, egui::Button::new("↩ 撤销写入")).clicked() {
                    self.ms_undo();
                }
                if ui
                    .add_enabled(!busy && self.ms_pid != 0, egui::Button::new("🛡 强制写入（解除页保护）"))
                    .on_hover_text(
                        "页被游戏设成只读 / Guard 时，普通「写入」会被系统拒绝（看起来像「改了没反应」）。\n\
                         这里会临时把该页改成可写、写完立刻恢复原保护。",
                    )
                    .clicked()
                {
                    self.ms_force_write();
                }
                if self.ms_frozen {
                    ui.label(RichText::new("● 已锁定").color(Color32::from_rgb(220, 120, 120)).small());
                }
            });
            ui.checkbox(
                &mut self.ms_ack,
                "我已了解写入内存的风险（可能让游戏状态或存档异常）；勾选后不再每次弹窗确认",
            );
            if let Ok(st) = self.ms_sh.lock() {
                if !st.msg.is_empty() {
                    ui.label(RichText::new(&st.msg).small());
                }
            }
            // 命中列表
            let mut diag_addr: Option<usize> = None;
            if let Ok(opt) = self.ms_scanner.lock() {
                if let Some(s) = opt.as_ref() {
                    if s.first_done {
                        let ro = s.readonly_hit_count();
                        if ro > 0 {
                            ui.label(
                                RichText::new(format!(
                                    "⚠ 有 {ro} 处命中所在页不可直接写（只读 / Guard）：普通「写入」会失败，请用上面的「强制写入」。"
                                ))
                                .color(Color32::from_rgb(200, 150, 60))
                                .small(),
                            );
                        }
                        ui.label(RichText::new(format!("当前命中 {} 处（显示前 200）：", s.hit_count())).strong());
                        egui::ScrollArea::vertical().max_height(240.0).auto_shrink([false, false]).show(ui, |ui| {
                            for h in s.hits.iter().take(200) {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(format!("0x{:016X}", h.addr)).weak().monospace().small());
                                    ui.label(RichText::new(h.value.display()).monospace());
                                    if ui
                                        .small_button("诊断")
                                        .on_hover_text("把该地址填到下面的「反修改保护识别」并立刻诊断")
                                        .clicked()
                                    {
                                        diag_addr = Some(h.addr);
                                    }
                                });
                            }
                        });
                    }
                }
            }
            if let Some(a) = diag_addr {
                self.gd_addr = format!("0x{a:X}");
                self.gd_start();
            }
        });

        ui.add_space(8.0);

        // ============ 方式二·五：反修改保护识别（改了立刻被还原 / 改了没用）============
        card(ui, |ui| {
            ui.label(RichText::new("方式二·五：反修改保护识别（改了立刻被还原 / 改了没用）").strong());
            ui.label(
                RichText::new(
                    "先用上面「方式二」把地址扫出来，把地址填到这里。\
                     诊断会临时写入一个探测值、测它多久被改回去（随后立刻恢复原值），再顺带查页保护、\
                     同值副本、已知加壳/反作弊模块、主程序的反调试导入，并在可执行区搜「还原指令」的落点。",
                )
                .weak()
                .small(),
            );
            ui.add_space(4.0);

            let gd = match self.gd_sh.lock() {
                Ok(st) => (
                    st.busy,
                    st.msg.clone(),
                    st.report.clone(),
                    st.regions_busy,
                    st.regions_msg.clone(),
                    st.regions.clone(),
                ),
                Err(_) => (false, String::new(), None, false, String::new(), None),
            };
            let (gd_busy, gd_msg, gd_report, gd_reg_busy, gd_reg_msg, gd_regions) = gd;

            ui.horizontal(|ui| {
                ui.label("地址:");
                ui.add(egui::TextEdit::singleline(&mut self.gd_addr).desired_width(190.0).hint_text("0x7FF6A000"));
                ui.label("探测值:");
                ui.add(egui::TextEdit::singleline(&mut self.gd_probe).desired_width(90.0).hint_text("留空=自动"));
                ui.label(RichText::new(format!("类型 {}（同上）", self.ms_ty.label())).weak().small());
                if ui
                    .add_enabled(!gd_busy && self.ms_pid != 0, egui::Button::new("🔬 诊断保护机制"))
                    .on_hover_text("会临时写入探测值（随即恢复原值），属于「动内存」操作。")
                    .clicked()
                {
                    self.gd_start();
                }
                if gd_busy {
                    ui.spinner();
                }
                if ui
                    .add_enabled(!gd_reg_busy && self.ms_pid != 0, egui::Button::new("🗺 页保护分布"))
                    .on_hover_text(
                        "只读遍历目标进程的整块内存：有多少只读页 / Guard 页 / 「可写且可执行」页（加壳或自修改代码的常见特征），\
                         以及各区域的保护与类型。不写内存，不需要风险确认。",
                    )
                    .clicked()
                {
                    self.gd_regions_start();
                }
                if gd_reg_busy {
                    ui.spinner();
                }
            });

            if !gd_msg.is_empty() {
                ui.label(RichText::new(&gd_msg).small());
            }

            if let Some(rep) = &gd_report {
                ui.add_space(4.0);
                egui::Grid::new("gd_facts").num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
                    ui.label(RichText::new("回滚判定").strong());
                    let color = match rep.verdict {
                        crate::features::guard::Verdict::Stable => Color32::from_rgb(90, 160, 90),
                        crate::features::guard::Verdict::Instant { .. } => Color32::from_rgb(210, 110, 110),
                        _ => Color32::from_rgb(200, 150, 60),
                    };
                    ui.label(RichText::new(format!("{} —— {}", rep.verdict.label(), rep.verdict.measure())).color(color));
                    ui.end_row();

                    ui.label(RichText::new("页保护").strong());
                    match &rep.page {
                        Some(p) => ui.label(format!(
                            "{}{}（{}）",
                            p.protect_name(),
                            if rep.page_blocked { " · 不可直接写" } else { "" },
                            p.kind_name()
                        )),
                        None => ui.label(RichText::new("地址不在已提交区域（请重新扫描）").weak()),
                    };
                    ui.end_row();

                    ui.label(RichText::new("同值副本").strong());
                    ui.label(format!(
                        "{} 个{}",
                        rep.mirrors.len(),
                        if rep.mirror_truncated { "（已截断）" } else { "" }
                    ));
                    ui.end_row();

                    if let Some(rw) = &rep.rewrite {
                        ui.label(RichText::new("写入被改写").strong());
                        ui.label(rw.describe());
                        ui.end_row();
                    }

                    if !rep.ac_modules.is_empty() || !rep.prot_modules.is_empty() {
                        ui.label(RichText::new("保护机制").strong());
                        let names: Vec<String> = rep
                            .ac_modules
                            .iter()
                            .chain(rep.prot_modules.iter())
                            .map(|m| format!("{}（{}）", m.name, m.module))
                            .collect();
                        ui.label(names.join("、"));
                        ui.end_row();
                    }

                    if let Some(src) = rep.source_addr {
                        ui.label(RichText::new("数据源").strong());
                        ui.label(RichText::new(crate::features::guard::fmt_addr(src)).monospace());
                        ui.end_row();
                    }

                    if !rep.writers.is_empty() {
                        ui.label(RichText::new("还原指令落点").strong());
                        let addrs: Vec<String> = rep
                            .writers
                            .iter()
                            .take(4)
                            .map(|w| crate::features::guard::fmt_addr(w.addr))
                            .collect();
                        ui.label(RichText::new(addrs.join("  ")).monospace());
                        ui.end_row();
                    }
                });

                ui.add_space(4.0);
                ui.label(RichText::new("应对方案（按可行性排序）").strong());
                let mut action: Option<(u64, usize, String)> = None;
                let mut force: Option<usize> = None;
                let mut source: Option<usize> = None;
                for (i, p) in rep.plans.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let col = match p.feasibility {
                            crate::features::guard::Feasibility::High => C_OK,
                            crate::features::guard::Feasibility::Medium => C_WARN,
                            crate::features::guard::Feasibility::Low => C_MUTED,
                            crate::features::guard::Feasibility::NotAdvised => C_DANGER,
                        };
                        ui.label(RichText::new(format!("{}. [{}]", i + 1, p.feasibility.label())).color(col).small());
                        ui.label(RichText::new(&p.title).strong());
                        if let Some(auto) = p.auto.as_ref() {
                            // 只有「锁值 / 强制写入」两种方案带可直接执行的按钮；
                            // 「改数据源」靠扫描框另走一套流程，这里不出按钮。
                            let btn = match auto {
                                crate::features::guard::AutoAction::Freeze { .. } => Some((
                                    "按此周期锁值",
                                    "对诊断过的这个地址、按报告的周期反复写回当前「数值」框里的值",
                                )),
                                crate::features::guard::AutoAction::ForceWrite => Some((
                                    "强制写入该地址",
                                    "临时解除页保护后写入当前「数值」框里的值",
                                )),
                                crate::features::guard::AutoAction::WriteSource { .. } => Some((
                                    "写入这个数据源",
                                    "把当前「数值」框里的值写到这个地址 —— 它才是游戏真正采纳的「源」，\
                                     写它之后原地址会随之更新（尤其适合「写原地址没用 / 同值副本很多」）",
                                )),
                            };
                            if let Some((label, hover)) = btn {
                                let clicked = ui.small_button(label).on_hover_text(hover).clicked();
                                if clicked {
                                    match auto {
                                        crate::features::guard::AutoAction::Freeze { period_ms } => {
                                            action = Some((*period_ms, rep.addr, rep.probe.display()));
                                        }
                                        crate::features::guard::AutoAction::ForceWrite => {
                                            force = Some(rep.addr);
                                        }
                                        crate::features::guard::AutoAction::WriteSource { addr } => {
                                            source = Some(*addr);
                                        }
                                    }
                                }
                            }
                        }
                    });
                    ui.label(RichText::new(format!("　{}", p.detail)).weak().small());
                    for s in &p.steps {
                        ui.label(RichText::new(format!("　· {s}")).weak().small());
                    }
                }
                if let Some((ms, addr, probe)) = action {
                    self.gd_lock(ms, addr, &probe);
                }
                if let Some(addr) = force {
                    self.gd_force_write(addr);
                }
                if let Some(addr) = source {
                    self.gd_write_source(addr);
                }
            }

            // ---- 进程级页保护分布（只读遍历，不需要写内存确认）----
            if !gd_reg_msg.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(&gd_reg_msg).small());
            }
            if let Some((stats, sum)) = &gd_regions {
                ui.label(RichText::new(format!(
                    "可写 {} · 只读 {} · 可执行 {} · 可写可执行 {} · Guard {}",
                    sum.writable, sum.readonly, sum.execute, sum.rwx, sum.guard
                ))
                .small());
                ui.label(
                    RichText::new(format!("按类型：映像 {} · 映射 {} · 私有 {}", sum.image, sum.mapped, sum.private))
                        .weak()
                        .small(),
                );
                if sum.rwx > 0 {
                    ui.label(
                        RichText::new(format!(
                            "⚠ 有 {} 个「可写且可执行」页（正常程序极少）—— 加壳 / 自修改代码的常见特征。",
                            sum.rwx
                        ))
                        .color(C_WARN)
                        .small(),
                    );
                }
                let shown = stats.len().min(20);
                egui::CollapsingHeader::new(
                    RichText::new(format!("区域明细（{} 条，显示前 {shown}）", stats.len())).small(),
                )
                .show(ui, |ui| {
                    egui::Grid::new("gd_regions_grid").num_columns(5).spacing([12.0, 3.0]).show(ui, |ui| {
                        for h in ["基址", "大小", "保护", "类型", "合并"] {
                            ui.label(RichText::new(h).strong().small());
                        }
                        ui.end_row();
                        for s in stats.iter().take(shown) {
                            ui.label(RichText::new(crate::features::guard::fmt_addr(s.base)).monospace().small());
                            ui.label(RichText::new(crate::features::precheck::human_bytes(s.size as u64)).small());
                            ui.label(RichText::new(crate::memapi::protect_name(s.protect)).small());
                            ui.label(RichText::new(crate::memapi::kind_name(s.kind)).small());
                            ui.label(RichText::new(s.merged.to_string()).small());
                            ui.end_row();
                        }
                    });
                    if stats.len() > shown {
                        ui.label(RichText::new(format!("…其余 {} 条省略", stats.len() - shown)).weak().small());
                    }
                });
            }
        });

        ui.add_space(8.0);

        // ============ 方式三：封包类引擎的运行时补丁包（不改原封包）============
        card(ui, |ui| {
            ui.label(RichText::new("方式三：封包类引擎的补丁包（KiriKiri / 吉里吉里：不改原封包，删掉即还原）").strong());
            ui.label(RichText::new("游戏按 `Data 目录 → data.xp3 → patch.xp3 → patch2.xp3 → …` 搜索资源，越靠后优先级越高。把改好的文件（保持封包内原始相对路径）打成下一个空号的 patchN.xp3，引擎就会自动优先加载它——几 GB 的 data.xp3 一个字节都不用动。").weak().small());
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label("改动目录:");
                let w = (ui.available_width() - 300.0).max(80.0);
                ui.add_sized([w, 22.0], egui::TextEdit::singleline(&mut self.xp_src_str));
                if ui.button("浏览...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.xp_src_str = p.display().to_string();
                    }
                }
                if ui.button("🔄 刷新列表").clicked() {
                    self.xp_refresh();
                }
            });
            ui.horizontal(|ui| {
                ui.label("补丁包名:");
                ui.add_sized([140.0, 22.0], egui::TextEdit::singleline(&mut self.xp_name_str))
                    .on_hover_text("留空 = 自动取下一个空号（如已有 patch.xp3 / patch2.xp3 就生成 patch3.xp3）。绝不用来覆盖游戏自带的补丁包。");
                if ui.button("📦 生成补丁包").clicked() {
                    let src = PathBuf::from(self.xp_src_str.trim());
                    let name = self.xp_name_str.trim();
                    let name = if name.is_empty() { None } else { Some(name.to_string()) };
                    match xp3patch::build(&self.game_root, &src, name.as_deref()) {
                        Ok(m) => {
                            self.toast = Some((m.clone(), std::time::Instant::now()));
                            self.xp_msg = m;
                        }
                        Err(e) => self.toast = Some((format!("✘ {e}"), std::time::Instant::now())),
                    }
                    self.xp_refresh();
                }
                if ui
                    .button("🔍 只打包改动（对比原封包）")
                    .on_hover_text(
                        "上面填「解包产物目录」：会自动和游戏现有封包逐字节比对，只把真正改过的文件打进补丁包。\n\
                         适合「解包 → 汉化 → 打补丁」流程，未改动的文件不会白占体积。",
                    )
                    .clicked()
                {
                    let src = PathBuf::from(self.xp_src_str.trim());
                    let name = self.xp_name_str.trim();
                    let name = if name.is_empty() { None } else { Some(name.to_string()) };
                    match xp3patch::build_changed(&self.game_root, &src, name.as_deref()) {
                        Ok(m) => {
                            self.toast = Some(("已生成补丁包（仅含改动文件）".into(), std::time::Instant::now()));
                            self.xp_msg = m;
                        }
                        Err(e) => self.toast = Some((format!("✘ {e}"), std::time::Instant::now())),
                    }
                    self.xp_refresh();
                }
                if ui.button("🗑 移除本工具建的补丁包").clicked() {
                    let name = self.xp_name_str.trim();
                    if name.is_empty() {
                        self.toast = Some(("请先在「补丁包名」里填要移除的包名".into(), std::time::Instant::now()));
                    } else {
                        match xp3patch::remove(&self.game_root, name) {
                            Ok(m) => {
                                self.toast = Some((m.clone(), std::time::Instant::now()));
                                self.xp_msg = m;
                            }
                            Err(e) => self.toast = Some((format!("✘ {e}"), std::time::Instant::now())),
                        }
                        self.xp_refresh();
                    }
                }
            });
            if !self.xp_msg.is_empty() {
                ui.label(RichText::new(&self.xp_msg).small());
            }

            if !self.xp_items.is_empty() {
                ui.add_space(2.0);
                ui.label(RichText::new("当前封包（引擎搜索顺序，越靠后优先级越高）:").strong().small());
                egui::ScrollArea::vertical().max_height(120.0).auto_shrink([false, false]).show(ui, |ui| {
                    for (name, bytes, entries, ours, enc) in &self.xp_items {
                        let mut note = if *ours { "本工具创建".to_string() } else { "非本工具创建".to_string() };
                        if let Some(n) = entries {
                            note.push_str(&format!("，{n} 条目"));
                        }
                        if *enc {
                            note.push_str("，内容加密（STool 不解密）");
                        }
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(short_text(name, 28)).monospace());
                            ui.label(RichText::new(human_bytes(*bytes)).weak().small());
                            ui.label(RichText::new(note).weak().small());
                            if *ours && name.to_ascii_lowercase().ends_with(".xp3") && ui.small_button("选").clicked() {
                                self.xp_name_str = name.clone();
                            }
                        });
                    }
                });
            } else if let Some(t) = xp3patch::target_of("kirikiri") {
                ui.label(RichText::new(format!("提示: {}", t.caveat)).weak().small());
            }
        });

        // ---- 写内存二次确认（Cheat Engine 类工具会直接改目标进程内存，首次写入必须让用户明确知晓）----
        if let Some(pending) = self.ms_pending {
            let mut go = false;
            let mut cancel = false;
            egui::Window::new("⚠ 写入游戏内存前确认")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(RichText::new("内存扫描会直接改写目标进程的内存，属于修改器行为。").strong());
                    ui.label("可能导致：游戏状态异常、存档损坏、游戏崩溃，或被反作弊判定为异常。");
                    ui.label("建议：动手前先在游戏里手动存一次档；不确定的地址不要写。");
                    ui.add_space(6.0);
                    let act = match pending {
                        MsPending::WriteAll => "写入所有命中地址",
                        MsPending::Freeze => "锁定数值（按设定周期反复写入）",
                        MsPending::ForceWrite => "强制写入（临时解除页保护后写入，随即恢复页保护）",
                        MsPending::GuardProbe => "保护诊断（临时写入一个探测值、测完立刻恢复原值；并扫描可执行区）",
                    };
                    ui.label(RichText::new(format!("即将执行：{act}　新值：{}", self.ms_value.trim())).monospace());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("我明白，继续").clicked() {
                            go = true;
                        }
                        if ui.button("取消").clicked() {
                            cancel = true;
                        }
                    });
                });
            if go {
                self.ms_ack = true;
                self.ms_pending = None;
                match pending {
                    MsPending::WriteAll => self.ms_write_all_now(),
                    MsPending::Freeze => self.ms_toggle_freeze_now(),
                    MsPending::ForceWrite => self.ms_force_write_now(),
                    MsPending::GuardProbe => self.gd_start_now(),
                }
            } else if cancel {
                self.ms_pending = None;
            }
        }
    }

    /// 刷新补丁包列表（进入页面 / 生成 / 移除后调用）。
    pub(crate) fn xp_refresh(&mut self) {
        self.xp_items = xp3patch::list(&self.game_root)
            .into_iter()
            .map(|p| (p.name, p.bytes, p.entries, p.ours, p.encrypted))
            .collect();
    }

    /// MV/MZ 游戏自动接入：调试端口已开就直接连，否则自动找 Game.exe 带参启动再连。
    pub(crate) fn rt_auto_connect(&mut self) {
        if self.rt_game.is_some() || self.rt_connecting.load(Ordering::Relaxed) {
            return;
        }
        let is_mv = self
            .selected_det()
            .map(|d| d.ok() && d.plugin_id == "rpgmaker_mv")
            .unwrap_or(false);
        if !is_mv {
            return;
        }
        let Ok(port) = self.rt_port_str.trim().parse::<u16>() else { return };
        if runtime::probe_port(port) {
            self.rt_msg = "检测到游戏调试端口已开启，正在自动连接…".into();
        } else {
            let Some(exe) = runtime::find_game_exe(&self.game_root) else {
                self.rt_msg = "检测到 MV/MZ 游戏，但没找到启动程序（Game.exe），请手动选择游戏程序后点启动。".into();
                return;
            };
            match runtime::DebugGame::launch(&exe, port) {
                Ok(pid) => self.rt_msg = format!("已自动以调试模式启动游戏（进程 {pid}），正在连接…（若游戏原本已手动开着，请先关闭它避免多开）"),
                Err(e) => {
                    self.rt_msg = format!("自动启动失败：{e}。可手动选择游戏程序后点启动。");
                    return;
                }
            }
        }
        self.rt_start_connect(port);
    }

    /// 后台线程连接调试端口（完成后结果由 update() 收取）。
    pub(crate) fn rt_start_connect(&mut self, port: u16) {
        if self.rt_connecting.load(Ordering::Relaxed) {
            return;
        }
        self.rt_connecting.store(true, Ordering::Relaxed);
        self.rt_msg = "正在连接（游戏启动慢的话最多等 15 秒）…".into();
        let flag = self.rt_connecting.clone();
        let result = self.rt_result.clone();
        std::thread::spawn(move || {
            let g = runtime::DebugGame::connect(port);
            if let Ok(mut r) = result.lock() {
                *r = Some(g);
            }
            flag.store(false, Ordering::Relaxed);
        });
    }

    /// 选中/切换目标进程（或切换数值类型）后建立扫描会话；失败时立刻说明原因。
    ///
    /// 扫描会话（[`memscan::Scanner`]）保存着进程句柄与命中列表，**必须先建立**才能扫描；
    /// 之前只有「类型变化时置 None」而没有任何建立点，导致「首次扫描」永远报「请先选择目标进程」。
    pub(crate) fn ms_open_scanner(&mut self) {
        // 换目标 / 换类型都意味着旧会话作废，顺带把锁值停掉（否则会往旧进程的地址写）
        if self.ms_frozen {
            self.ms_freeze_stop.store(true, Ordering::Relaxed);
            self.ms_frozen = false;
        }
        self.ms_freeze_addr = None;

        let pid = self.ms_pid;
        if pid == 0 {
            if let Ok(mut opt) = self.ms_scanner.lock() {
                *opt = None;
            }
            return;
        }
        let ty = self.ms_ty;
        match memscan::Scanner::open(pid, ty) {
            Ok(s) => {
                if let Ok(mut opt) = self.ms_scanner.lock() {
                    *opt = Some(s);
                }
                if let Ok(mut st) = self.ms_sh.lock() {
                    st.msg = format!("已连接进程 {pid}（{}）。填好数值后点「🔎 首次扫描」。", ty.label());
                }
            }
            Err(e) => {
                if let Ok(mut opt) = self.ms_scanner.lock() {
                    *opt = None;
                }
                if let Ok(mut st) = self.ms_sh.lock() {
                    st.msg = format!("✘ 打不开进程 {pid}：{e}");
                }
            }
        }
    }

    /// 后台线程执行首次/再次扫描。
    pub(crate) fn ms_start_scan(&mut self, first: bool) {
        let value = self.ms_value.trim().to_string();
        let filter = self.ms_filter;
        let pid = self.ms_pid;
        let ty = self.ms_ty;
        let scanner = self.ms_scanner.clone();
        let sh = self.ms_sh.clone();
        {
            let mut st = sh.lock().unwrap();
            if st.busy {
                self.toast = Some(("扫描还在进行中，稍等…".into(), std::time::Instant::now()));
                return;
            }
            st.busy = true;
            st.msg = if first { "正在扫描全部内存，可能要几秒到几十秒…".into() } else { "正在过滤…".into() };
        }
        std::thread::spawn(move || {
            let msg = ms_do_scan(&scanner, pid, ty, &value, filter, first);
            if let Ok(mut st) = sh.lock() {
                st.busy = false;
                st.msg = msg;
            }
        });
    }

    /// 撤销写入：恢复所有被写过的地址的原始值。
    pub(crate) fn ms_undo(&mut self) {
        let result = match self.ms_scanner.lock() {
            Ok(mut opt) => match opt.as_mut() {
                Some(s) if s.has_saved() => {
                    let n = s.undo();
                    s.refresh();
                    Ok::<usize, String>(n)
                }
                Some(_) => Err("没有可撤销的写入".into()),
                None => Err("请先扫描".into()),
            },
            Err(_) => Err("扫描器状态异常".into()),
        };
        match result {
            Ok(n) => self.toast = Some((format!("已恢复 {n} 处原值"), std::time::Instant::now())),
            Err(e) => self.toast = Some((format!("✘ {e}"), std::time::Instant::now())),
        }
    }

    /// 数值锁定入口：未确认过写内存风险则先弹确认框。
    /// 从扫描面板进入时会把「只锁某一个地址」清掉（锁定全部命中）。
    pub(crate) fn ms_toggle_freeze(&mut self) {
        // 从扫描面板进入 = 锁定全部命中（清掉「只锁某一个地址」）
        self.ms_freeze_addr = None;
        if !self.ms_frozen && !self.ms_ack {
            self.ms_pending = Some(MsPending::Freeze);
            return;
        }
        self.ms_toggle_freeze_now();
    }

    /// 数值锁定：后台线程按 `ms_freeze_ms` 反复写入当前输入框的值（Cheat Engine 的 Freeze）。
    /// `ms_freeze_addr` 为 Some 时只写那一个地址（保护诊断面板会用它锁定诊断过的地址）。
    pub(crate) fn ms_toggle_freeze_now(&mut self) {
        if self.ms_frozen {
            self.ms_freeze_stop.store(true, Ordering::Relaxed);
            self.ms_frozen = false;
            self.toast = Some(("已解除锁定".into(), std::time::Instant::now()));
            return;
        }
        let value = self.ms_value.trim().to_string();
        if value.is_empty() {
            self.toast = Some(("请先在数值框填入要锁定的值".into(), std::time::Instant::now()));
            return;
        }
        // 没有会话就没有可写地址：直接说清楚，别把「已锁定」显示出来骗人
        if self.ms_scanner.lock().map(|o| o.is_none()).unwrap_or(true) {
            self.toast = Some((
                "还没有扫描会话：请先选好目标进程、填数值并点「🔎 首次扫描」".into(),
                std::time::Instant::now(),
            ));
            return;
        }
        let scanner = self.ms_scanner.clone();
        let stop = Arc::new(AtomicBool::new(false));
        self.ms_freeze_stop = stop.clone();
        self.ms_frozen = true;
        let period = self.ms_freeze_ms.max(1);
        let only = self.ms_freeze_addr;
        let scope = match only {
            Some(a) => crate::features::guard::fmt_addr(a),
            None => "全部命中地址".to_string(),
        };
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Ok(mut opt) = scanner.lock() {
                    if let Some(s) = opt.as_mut() {
                        // 普通写入优先；页被锁成只读时退回「强制写入」（临时解除页保护）
                        if s.write_raw(&value, only).is_err() {
                            let _ = s.force_write(&value, only);
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(period));
            }
        });
        self.toast = Some((
            format!("已按每 {period} 毫秒锁定 {scope}（游戏里改不掉；要改锁定值请先解锁）"),
            std::time::Instant::now(),
        ));
    }

    /// 写入所有命中入口：未确认过写内存风险则先弹确认框。
    pub(crate) fn ms_write_all(&mut self) {
        if !self.ms_ack {
            self.ms_pending = Some(MsPending::WriteAll);
            return;
        }
        self.ms_write_all_now();
    }

    /// 真正把新值写入所有命中地址（用当前输入框的值）。
    pub(crate) fn ms_write_all_now(&mut self) {
        let value = self.ms_value.trim().to_string();
        let result = {
            let mut opt = match self.ms_scanner.lock() {
                Ok(o) => o,
                Err(_) => return,
            };
            match opt.as_mut() {
                Some(s) => s.write(&value, None).map(|n| {
                    s.refresh();
                    format!("已把 {n} 处地址改为 {value}，回游戏看看效果！")
                }),
                None => Err("请先扫描".into()),
            }
        };
        match result {
            Ok(m) => self.toast = Some((m, std::time::Instant::now())),
            Err(e) => self.toast = Some((format!("✘ {e}"), std::time::Instant::now())),
        }
    }

    // ---------------------------------------------------------------
    // 强制写入 / 反修改保护诊断
    // ---------------------------------------------------------------

    /// 强制写入入口：未确认过写内存风险则先弹确认框。
    pub(crate) fn ms_force_write(&mut self) {
        if !self.ms_ack {
            self.ms_pending = Some(MsPending::ForceWrite);
            return;
        }
        self.ms_force_write_now();
    }

    /// 强制写入：页只读 / Guard 导致普通写入失败时，临时解除页保护再写。
    pub(crate) fn ms_force_write_now(&mut self) {
        let value = self.ms_value.trim().to_string();
        let result = {
            let mut opt = match self.ms_scanner.lock() {
                Ok(o) => o,
                Err(_) => return,
            };
            match opt.as_mut() {
                Some(s) => s.force_write(&value, None).map(|(n, fix)| {
                    s.refresh();
                    match fix {
                        crate::memapi::PageFix::NotNeeded => format!("已把 {n} 处地址改为 {value}（页本来就可写）"),
                        crate::memapi::PageFix::Reprotected { .. } => {
                            format!("已把 {n} 处地址改为 {value}（临时解除页保护后写入，原保护已恢复）")
                        }
                    }
                }),
                None => Err("请先扫描".into()),
            }
        };
        match result {
            Ok(m) => self.toast = Some((m, std::time::Instant::now())),
            Err(e) => self.toast = Some((format!("✘ {e}"), std::time::Instant::now())),
        }
    }

    /// 保护诊断入口：未确认过写内存风险则先弹确认框。
    pub(crate) fn gd_start(&mut self) {
        if !self.ms_ack {
            self.ms_pending = Some(MsPending::GuardProbe);
            return;
        }
        self.gd_start_now();
    }

    /// 后台线程跑 [`guard::analyze`]：写入探测 + 页保护 + 保护机制线索 + 代码落点 + 方案。
    pub(crate) fn gd_start_now(&mut self) {
        if self.ms_pid == 0 {
            self.toast = Some(("请先在上面的「目标进程」里选一个进程".into(), std::time::Instant::now()));
            return;
        }
        let Some(addr) = guard::parse_addr(&self.gd_addr) else {
            self.toast = Some(("地址要是十六进制，例如 0x7FF6A000（也可从命中列表点「诊断」自动填入）".into(), std::time::Instant::now()));
            return;
        };
        let ty = self.ms_ty;
        if !memscan::is_numeric(ty) {
            self.toast = Some((format!("{} 不支持保护探测：请把「数值类型」切成整数 / 小数", ty.label()), std::time::Instant::now()));
            return;
        }
        let probe = self.gd_probe.trim().to_string();
        let sh = self.gd_sh.clone();
        {
            let mut st = match sh.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if st.busy {
                self.toast = Some(("诊断还在进行中，稍等…".into(), std::time::Instant::now()));
                return;
            }
            st.busy = true;
            st.report = None;
            st.msg = "正在诊断：写入探测（约 1 秒）+ 同值副本 + 页保护 + 模块与导入表 + 可执行区扫描，请稍候…".into();
        }
        let pid = self.ms_pid;
        std::thread::spawn(move || {
            let opts = guard::ProbeOpts {
                probe: if probe.is_empty() { None } else { Some(probe) },
                ..Default::default()
            };
            let r = guard::analyze(pid, addr, ty, &opts);
            if let Ok(mut st) = sh.lock() {
                st.busy = false;
                match r {
                    Ok(rep) => {
                        st.msg = rep.headline();
                        st.report = Some(rep);
                    }
                    Err(e) => st.msg = format!("✘ {e}"),
                }
            }
        });
    }

    /// 按诊断出的周期锁定诊断过的那个地址（写当前「数值」框的值，空则用探测值）。
    pub(crate) fn gd_lock(&mut self, period_ms: u64, addr: usize, probe_display: &str) {
        if self.ms_scanner.lock().map(|o| o.is_none()).unwrap_or(true) {
            self.toast = Some(("请先在上面「方式二」做一次扫描，锁值需要一个扫描会话".into(), std::time::Instant::now()));
            return;
        }
        if self.ms_value.trim().is_empty() {
            self.ms_value = probe_display.to_string();
        }
        if self.ms_frozen {
            self.ms_freeze_stop.store(true, Ordering::Relaxed);
            self.ms_frozen = false;
        }
        self.ms_freeze_ms = period_ms.max(1);
        self.ms_freeze_addr = Some(addr);
        self.ms_toggle_freeze_now();
    }

    /// 页保护分布：**只读**遍历目标进程内存区域（不写内存，因此不需要风险确认）。
    pub(crate) fn gd_regions_start(&mut self) {
        if self.ms_pid == 0 {
            self.toast = Some(("请先在上面的「目标进程」里选一个进程".into(), std::time::Instant::now()));
            return;
        }
        let pid = self.ms_pid;
        let sh = self.gd_sh.clone();
        {
            let mut st = match sh.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if st.regions_busy {
                self.toast = Some(("页保护分布还在统计中，稍等…".into(), std::time::Instant::now()));
                return;
            }
            st.regions_busy = true;
            st.regions_msg = "正在遍历内存区域（只读，不动目标进程）…".into();
        }
        std::thread::spawn(move || {
            let r = guard::regions_of(pid);
            if let Ok(mut st) = sh.lock() {
                st.regions_busy = false;
                match r {
                    Ok((stats, sum)) => {
                        st.regions_msg = format!(
                            "共 {} 个已提交区域 / {}（相邻同保护区域已合并）",
                            sum.committed,
                            crate::features::precheck::human_bytes(sum.total_bytes as u64)
                        );
                        st.regions = Some((stats, sum));
                    }
                    Err(e) => {
                        st.regions_msg = format!("✘ {e}");
                        st.regions = None;
                    }
                }
            }
        });
    }

    /// 取「数值」框里的待写值，并校验进程/输入是否就绪；失败时已设好 toast。
    fn gd_take_value(&mut self) -> Option<String> {
        let value = self.ms_value.trim().to_string();
        if value.is_empty() {
            self.toast = Some((
                "请先在上面「方式二」的「数值」框填入要写入的新值".into(),
                std::time::Instant::now(),
            ));
            return None;
        }
        if self.ms_pid == 0 {
            return None;
        }
        Some(value)
    }

    /// 对诊断过的地址做强制写入（用当前「数值」框的值）。
    pub(crate) fn gd_force_write(&mut self, addr: usize) {
        let Some(value) = self.gd_take_value() else { return };
        let note = match guard::force_write(self.ms_pid, addr, self.ms_ty, &value) {
            Ok(crate::memapi::PageFix::NotNeeded) => {
                format!("已把 {} 改为 {value}（页本来就可写）", guard::fmt_addr(addr))
            }
            Ok(crate::memapi::PageFix::Reprotected { .. }) => format!(
                "已把 {} 改为 {value}（临时解除页保护后写入，原保护已恢复）",
                guard::fmt_addr(addr)
            ),
            Err(e) => format!("✘ {e}"),
        };
        self.toast = Some((note, std::time::Instant::now()));
    }

    /// 把当前「数值」框的值写到「数据源」地址（游戏真正采纳的源，原地址会随之更新）。
    pub(crate) fn gd_write_source(&mut self, addr: usize) {
        let Some(value) = self.gd_take_value() else { return };
        let note = match guard::force_write(self.ms_pid, addr, self.ms_ty, &value) {
            Ok(crate::memapi::PageFix::NotNeeded) => format!(
                "已写入数据源 {} = {value}（它才是被采纳的源，原地址会随之更新）",
                guard::fmt_addr(addr)
            ),
            Ok(crate::memapi::PageFix::Reprotected { .. }) => format!(
                "已写入数据源 {} = {value}（临时解除页保护后写入，原保护已恢复；原地址会随之更新）",
                guard::fmt_addr(addr)
            ),
            Err(e) => format!("✘ {e}"),
        };
        self.toast = Some((note, std::time::Instant::now()));
    }
}

/// 扫描的执行体（在后台线程里跑）。
///
/// 会话缺失时**先补一次** [`memscan::Scanner::open`]：选中进程时已经开好了，但如果用户是在
/// 类型切换、进程重启、或会话被清空之后直接点扫描，这里能自愈，而不是丢一句「请先选择目标进程」。
fn ms_do_scan(
    scanner: &Arc<std::sync::Mutex<Option<memscan::Scanner>>>,
    pid: u32,
    ty: memscan::ScanType,
    value: &str,
    filter: memscan::Filter,
    first: bool,
) -> String {
    if pid == 0 {
        return "✘ 请先在上面的「目标进程」里选一个进程".into();
    }
    let mut opt = match scanner.lock() {
        Ok(o) => o,
        Err(_) => return "✘ 扫描器状态异常（内部锁已中毒），请重启 STool".into(),
    };
    if opt.is_none() {
        match memscan::Scanner::open(pid, ty) {
            Ok(s) => *opt = Some(s),
            Err(e) => return format!("✘ 打不开进程 {pid}：{e}"),
        }
    }
    let Some(s) = opt.as_mut() else {
        return "✘ 扫描器状态异常（内部锁已中毒），请重启 STool".into();
    };
    let r = if first {
        s.first_scan(value)
            .map(|n| format!("首次扫描完成：命中 {n} 处。回游戏改变数值后用「再次扫描」过滤。"))
    } else {
        s.next_scan(value, filter).map(|n| format!("过滤完成：剩 {n} 处。"))
    };
    match r {
        Ok(m) => m,
        Err(e) => format!("✘ {e}"),
    }
}
