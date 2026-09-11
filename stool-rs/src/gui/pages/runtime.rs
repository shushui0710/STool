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
        egui::Frame::group(ui.style()).show(ui, |ui| {
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
        egui::Frame::group(ui.style()).show(ui, |ui| {
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
                egui::ComboBox::from_id_salt("ms_proc")
                    .selected_text(&self.ms_pid_label)
                    .width(320.0)
                    .show_ui(ui, |ui| {
                        for (pid, name) in filtered {
                            let label = format!("{pid} — {name}");
                            if ui.selectable_label(self.ms_pid == *pid, &label).clicked() {
                                self.ms_pid = *pid;
                                self.ms_pid_label = label.clone();
                            }
                        }
                    });
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
                                // 类型变了，旧会话作废；锁定也要停掉
                                if self.ms_frozen {
                                    self.ms_freeze_stop.store(true, Ordering::Relaxed);
                                    self.ms_frozen = false;
                                }
                                *self.ms_scanner.lock().unwrap() = None;
                                if let Ok(mut st) = self.ms_sh.lock() {
                                    st.msg.clear();
                                }
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
                if self.ms_frozen {
                    ui.label(RichText::new("● 已锁定").color(Color32::from_rgb(220, 120, 120)).small());
                }
            });
            if let Ok(st) = self.ms_sh.lock() {
                if !st.msg.is_empty() {
                    ui.label(RichText::new(&st.msg).small());
                }
            }
            // 命中列表
            if let Ok(opt) = self.ms_scanner.lock() {
                if let Some(s) = opt.as_ref() {
                    if s.first_done {
                        ui.label(RichText::new(format!("当前命中 {} 处（显示前 200）：", s.hit_count())).strong());
                        egui::ScrollArea::vertical().max_height(240.0).auto_shrink([false, false]).show(ui, |ui| {
                            for h in s.hits.iter().take(200) {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(format!("0x{:016X}", h.addr)).weak().monospace().small());
                                    ui.label(RichText::new(h.value.display()).monospace());
                                });
                            }
                        });
                    }
                }
            }
        });

        ui.add_space(8.0);

        // ============ 方式三：封包类引擎的运行时补丁包（不改原封包）============
        egui::Frame::group(ui.style()).show(ui, |ui| {
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

    /// 后台线程执行首次/再次扫描。
    pub(crate) fn ms_start_scan(&mut self, first: bool) {
        let value = self.ms_value.trim().to_string();
        let filter = self.ms_filter;
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
            let msg = match scanner.lock() {
                Ok(mut opt) => match opt.as_mut() {
                    Some(s) => {
                        let r = if first {
                            s.first_scan(&value).map(|n| format!("首次扫描完成：命中 {n} 处。回游戏改变数值后用“再次扫描”过滤。"))
                        } else {
                            s.next_scan(&value, filter).map(|n| format!("过滤完成：剩 {n} 处。"))
                        };
                        match r {
                            Ok(m) => m,
                            Err(e) => format!("✘ {e}"),
                        }
                    }
                    None => "✘ 请先选择目标进程".into(),
                },
                Err(_) => "✘ 扫描器状态异常".into(),
            };
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

    /// 数值锁定：后台线程每 150ms 反复写入当前输入框的值（Cheat Engine 的 Freeze）。
    pub(crate) fn ms_toggle_freeze(&mut self) {
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
        let scanner = self.ms_scanner.clone();
        let stop = Arc::new(AtomicBool::new(false));
        self.ms_freeze_stop = stop.clone();
        self.ms_frozen = true;
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Ok(opt) = scanner.lock() {
                    if let Some(s) = opt.as_ref() {
                        let _ = s.write_raw(&value, None);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        });
        self.toast = Some(("已锁定该数值（游戏里改不掉；要改锁定值请先解锁再锁定）".into(), std::time::Instant::now()));
    }

    /// 把新值写入所有命中地址（用当前输入框的值）。
    pub(crate) fn ms_write_all(&mut self) {
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
}
