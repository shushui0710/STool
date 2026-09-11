//! 页面：text（P2-11 从 gui.rs 拆出）。

use crate::gui::*;
use std::collections::{HashMap};
use std::path::PathBuf;
use eframe::egui::{RichText};

impl StoolApp {
    pub(crate) fn page_text(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "文本 / 汉化", "两条汉化通道：左=CSV 回填（改动游戏资源文件）；右=JSON 运行时注入（MTool 式，不改游戏文件）");
        let Some(d) = self.selected_det().cloned() else {
            ui.label("请先在首页完成检测。");
            return;
        };
        ui.label(format!("当前引擎: {} ({})", d.name, d.plugin_id));
        ui.add_space(6.0);

        // ---- 共享：机翻引擎配置（两条通道共用） ----
        ui.label(RichText::new("机翻引擎（两条通道共用；也可不用机翻，人工翻译后直接走通道第 3 步）").strong());
        ui.horizontal(|ui| {
            if ui.small_button("预设: DeepSeek").clicked() {
                self.mtl_base_url = "https://api.deepseek.com".into();
                self.mtl_model = "deepseek-chat".into();
            }
            if ui.small_button("预设: 智谱（glm-4-flash 免费）").clicked() {
                self.mtl_base_url = "https://open.bigmodel.cn/api/paas/v4".into();
                self.mtl_model = "glm-4-flash".into();
            }
            if ui.small_button("预设: 本地 Ollama（免 Key）").clicked() {
                self.mtl_base_url = "http://127.0.0.1:11434/v1".into();
                self.mtl_model = "qwen2.5:7b".into();
            }
        });
        ui.horizontal(|ui| {
            ui.label("API 地址:");
            ui.add_sized([170.0, 22.0], egui::TextEdit::singleline(&mut self.mtl_base_url));
            ui.label("Key:");
            ui.add_sized([140.0, 22.0], egui::TextEdit::singleline(&mut self.mtl_key).password(true));
            ui.label("模型:");
            ui.add_sized([130.0, 22.0], egui::TextEdit::singleline(&mut self.mtl_model));
            ui.label("每批:");
            ui.add_sized([42.0, 22.0], egui::TextEdit::singleline(&mut self.mtl_batch));
            ui.label("并发:");
            ui.add_sized([42.0, 22.0], egui::TextEdit::singleline(&mut self.mtl_jobs)).on_hover_text("同时发出的翻译批数（网络型任务，2~6 较合适；1=串行）");
        });
        egui::CollapsingHeader::new(RichText::new("📖 术语表（可选，强制人名/地名/术语译名全文一致）").small())
            .id_salt("mtl_glossary")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(RichText::new("每行一条，格式：原文=译文。示例：アリス=爱丽丝。机翻时逐条强制遵守。").weak().small());
                ui.add(
                    egui::TextEdit::multiline(&mut self.mtl_glossary)
                        .desired_rows(4)
                        .desired_width(ui.available_width())
                        .hint_text("アリス=爱丽丝\nポーション=药水"),
                );
            });
        // 配置有变化就落盘
        {
            let c = &mut self.cfg;
            let dirty = c.mtl_base_url != self.mtl_base_url
                || c.mtl_key != self.mtl_key
                || c.mtl_model != self.mtl_model
                || c.mtl_glossary != self.mtl_glossary
                || c.mtl_batch.to_string() != self.mtl_batch
                || c.mtl_jobs.to_string() != self.mtl_jobs;
            c.mtl_base_url = self.mtl_base_url.clone();
            c.mtl_key = self.mtl_key.clone();
            c.mtl_model = self.mtl_model.clone();
            c.mtl_glossary = self.mtl_glossary.clone();
            c.mtl_batch = self.mtl_batch.trim().parse().unwrap_or(c.mtl_batch);
            c.mtl_jobs = self.mtl_jobs.trim().parse().unwrap_or(c.mtl_jobs);
            if dirty {
                let _ = crate::settings::save(&self.cfg);
            }
        }
        let batch = self.mtl_batch.trim().parse::<usize>().unwrap_or(20).clamp(1, 100);
        let jobs = self.mtl_jobs.trim().parse::<usize>().unwrap_or(4).clamp(1, 32);
        let mtl_ready = !self.mtl_base_url.trim().is_empty() && !self.mtl_model.trim().is_empty();

        // ---- 两条通道并排 ----
        ui.add_space(6.0);
        ui.columns(2, |cols| {
            // ============ 左：CSV 通道 ============
            {
                let ui = &mut cols[0];
                ui.label(RichText::new("📄 CSV 通道 · 回填式汉化").strong());
                ui.label(RichText::new(
                    "数据流：游戏文件 → ①提取 → CSV → ②机翻/人工翻译 → ③回填 → 输出目录（按提示放回游戏）",
                ).weak().small());
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("CSV 文件:");
                    ui.add_sized([ui.available_width() - 40.0, 22.0], egui::TextEdit::singleline(&mut self.csv_path_str));
                    if ui.button("...").clicked() {
                        if let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).save_file() {
                            self.csv_path_str = p.display().to_string();
                            self.csv_path = p;
                        }
                    }
                });
                ui.add_space(6.0);
                // ① 提取
                if ui.add_sized([ui.available_width(), 26.0], egui::Button::new("① 提取文本 → CSV")).clicked() {
                    let plugin_id = d.plugin_id.clone();
                    let csv = self.csv_path.clone();
                    self.spawn("text_extract", crate::features::precheck::Scope::out_only(), move |shared, root, out| {
                        let opts: HashMap<String, String> = HashMap::new();
                        exec_op(Op::TextExtract, &plugin_id, shared, &root, &out, &opts, Some(&csv))
                    });
                }
                // ② 机翻
                if ui.add_enabled(mtl_ready, egui::Button::new("② 机翻 CSV（自动填充 translation 列）")).clicked() {
                    let csv = self.csv_path.clone();
                    let base = self.mtl_base_url.clone();
                    let key = self.mtl_key.clone();
                    let model = self.mtl_model.clone();
                    let glossary = self.mtl_glossary.clone();
                    self.spawn("机翻 CSV", crate::features::precheck::Scope::read_only(), move |shared, _root, _out| {
                        let tr = crate::features::translate::OpenAiCompat { base_url: base, api_key: key, model, glossary };
                        let cancel = shared.cancel.clone();
                        let ps = shared.clone();
                        let progress = move |f: f32, m: &str| {
                            if let Ok(mut p) = ps.progress.lock() {
                                *p = (f, m.to_string());
                            }
                        };
                        match crate::features::translate::translate_csv(&csv, &tr, batch, jobs, &progress, &cancel) {
                            Ok((a, t)) if a == 0 && t == 0 => crate::engines::OpOutcome::ok("没有待翻条目（译文已全部就绪）"),
                            Ok((a, t)) => crate::engines::OpOutcome::okn(format!("机翻完成 {a}/{t} 条 → {}", csv.display()), a),
                            Err(e) => crate::engines::OpOutcome::fail(e),
                        }
                    });
                }
                // ③ 回填
                if ui.add_sized([ui.available_width(), 26.0], egui::Button::new("③ 回填翻译（按 CSV translation 列写回）")).clicked() {
                    let plugin_id = d.plugin_id.clone();
                    let csv = self.csv_path.clone();
                    self.spawn("text_import", crate::features::precheck::Scope::out_only(), move |shared, root, out| {
                        let opts: HashMap<String, String> = HashMap::new();
                        exec_op(Op::TextImport, &plugin_id, shared, &root, &out, &opts, Some(&csv))
                    });
                }
                ui.add_space(4.0);
                ui.label(RichText::new(
                    "适合：能改游戏资源文件的场景（回填后把输出目录内容放回游戏）。\
                     人工翻译：用 Excel/WPS 打开 CSV 填 translation 列即可；机翻支持断点续翻。",
                ).weak().small());
            }
            // ============ 右：JSON 通道 ============
            {
                let ui = &mut cols[1];
                ui.label(RichText::new("💉 JSON 通道 · 运行时注入（MTool 式）").strong());
                ui.label(RichText::new(
                    "数据流：原文 → JSON（键=原文，值=译文）→ 机翻填空译文 → 注入 → 启动游戏即在内存里替换文本；移除注入即还原",
                ).weak().small());
                ui.add_space(6.0);
                // 明确告诉用户"这个引擎能不能注入、怎么注入、边界在哪"
                match crate::features::inject::support_of(&d.plugin_id) {
                    Some(s) => {
                        ui.label(
                            RichText::new(format!("✔ 本引擎支持运行时注入 · 适配方式：{}", s.mechanism))
                                .small()
                                .color(Color32::from_rgb(130, 205, 130)),
                        );
                        ui.label(RichText::new(format!("边界：{}", s.limits)).weak().small());
                    }
                    None => {
                        ui.label(
                            RichText::new(format!(
                                "⚠ {} 不在运行时注入范围内：它的文本封在私有封包 / 编译脚本里，没有稳定的运行时替换入口。\
                                 请改用左侧通道：“文本提取 → 机翻 → 翻译回填”，必要时再“封包回写”。",
                                d.name
                            ))
                            .small()
                            .color(Color32::from_rgb(228, 180, 110)),
                        );
                    }
                }
                ui.add_space(4.0);
                if self.inj_json_str.is_empty() {
                    self.inj_json_str = self.game_root.join("translation.json").display().to_string();
                }
                ui.label(RichText::new("原文 JSON 从哪来（三选一）：").strong().small());
                ui.label(RichText::new(
                    "① 点下方\"从 CSV 生成\"——把左通道提取的 CSV 一键转成 JSON，原文自动去重（推荐）；\
                     ② 手写 {\"原文\":\"译文\"}；③ 直接用 MTool 的 translation.json（格式兼容）",
                ).weak().small());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("JSON 文件:");
                    ui.add_sized([ui.available_width() - 40.0, 22.0], egui::TextEdit::singleline(&mut self.inj_json_str));
                    if ui.button("...").clicked() {
                        if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
                            self.inj_json_str = p.display().to_string();
                        }
                    }
                });
                ui.add_space(6.0);
                // ① 从 CSV 生成 JSON（原文来源主入口）
                if ui.add_sized([ui.available_width(), 26.0], egui::Button::new("① 从 CSV 生成 JSON 骨架（原文为键）")).clicked() {
                    let csv = self.csv_path.clone();
                    let jp = PathBuf::from(self.inj_json_str.trim());
                    if !csv.exists() {
                        self.toast = Some((
                            format!("CSV 不存在，请先在左侧通道 ① 提取文本: {}", csv.display()),
                            std::time::Instant::now(),
                        ));
                    } else {
                        match crate::features::inject::csv_to_json_skeleton(&csv, &jp) {
                            Ok((total, filled)) => {
                                let msg = format!(
                                    "已生成 {total} 条原文 → {}（{filled} 条已带译文，其余留空待机翻）",
                                    jp.display()
                                );
                                self.log(format!("✔ {msg}"));
                                self.toast = Some((msg, std::time::Instant::now()));
                            }
                            Err(e) => self.toast = Some((format!("生成失败: {e}"), std::time::Instant::now())),
                        }
                    }
                }
                // ② 机翻 JSON
                if ui.add_enabled(mtl_ready, egui::Button::new("② 机翻 JSON（填充空译文）")).clicked() {
                    let json = PathBuf::from(self.inj_json_str.trim());
                    let base = self.mtl_base_url.clone();
                    let key = self.mtl_key.clone();
                    let model = self.mtl_model.clone();
                    let glossary = self.mtl_glossary.clone();
                    self.spawn("机翻注入 JSON", crate::features::precheck::Scope::read_only(), move |shared, _root, _out| {
                        let tr = crate::features::translate::OpenAiCompat { base_url: base, api_key: key, model, glossary };
                        let cancel = shared.cancel.clone();
                        let ps = shared.clone();
                        let progress = move |f: f32, m: &str| {
                            if let Ok(mut p) = ps.progress.lock() {
                                *p = (f, m.to_string());
                            }
                        };
                        match crate::features::translate::translate_json(&json, &tr, batch, jobs, &progress, &cancel) {
                            Ok((a, t)) if a == 0 && t == 0 => crate::engines::OpOutcome::ok("没有待翻条目（译文已全部就绪）"),
                            Ok((a, t)) => crate::engines::OpOutcome::okn(format!("机翻完成 {a}/{t} 条 → {}", json.display()), a),
                            Err(e) => crate::engines::OpOutcome::fail(e),
                        }
                    });
                }
                // ③ 注入 / ④ 移除 / 刷新
                let inject_ok = crate::features::inject::is_supported(&d.plugin_id);
                let inject_w = ui.available_width();
                if ui
                    .add_enabled(
                        inject_ok,
                        egui::Button::new("③ 注入翻译（运行时替换，不改游戏文件）").min_size(egui::vec2(inject_w, 26.0)),
                    )
                    .on_disabled_hover_text("该引擎不支持运行时注入，请走左侧“文本提取 → 机翻 → 回填”通道")
                    .clicked()
                {
                    let plugin_id = d.plugin_id.clone();
                    let json = PathBuf::from(self.inj_json_str.trim());
                    self.spawn("text_inject", crate::features::precheck::Scope::root_only(), move |shared, root, out| {
                        let opts: HashMap<String, String> = HashMap::new();
                        exec_op(Op::TextInject, &plugin_id, shared, &root, &out, &opts, Some(&json))
                    });
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(inject_ok, egui::Button::new("④ 移除注入（完全还原）")).clicked() {
                        let plugin_id = d.plugin_id.clone();
                        self.spawn("text_uninject", crate::features::precheck::Scope::root_only(), move |shared, root, out| {
                            let _ = (&shared, &out);
                            match crate::features::inject::uninstall(&root, &plugin_id) {
                                Ok(m) => crate::engines::OpOutcome::ok(m),
                                Err(e) => crate::engines::OpOutcome::fail(e),
                            }
                        });
                    }
                    if ui.button("刷新状态").clicked() {
                        let st = crate::features::inject::status(&self.game_root, &d.plugin_id);
                        self.inj_status = Some((st.installed, st.entries, st.json_path));
                    }
                });
                if let Some((installed, entries, jp)) = &self.inj_status {
                    let tip = if *installed { "✔ 已注入" } else { "未注入" };
                    ui.label(RichText::new(format!(
                        "{tip} · {entries} 条译文 · 生效 JSON: {}",
                        if jp.is_empty() { "（游戏目录下未发现）" } else { jp.as_str() }
                    )).small());
                }
                ui.label(RichText::new(
                    "说明：注入按引擎选适配方式——Ren'Py 新增 game/stool_translate.rpy；RPG Maker MV/MZ 在 js/plugins.js 登记汉化插件；\
                     TyranoBuilder 与 HTML/Electron 在入口 HTML 追加脚本。被改动的原文件首次注入前都会备份（*.stool.bak），\
                     翻译复制为游戏目录下的 stool_translate.json；启动/刷新游戏即生效，未命中映射的文本保留原文；点“移除注入”可完全还原。",
                ).weak().small());
            }
        });
        ui.add_space(4.0);
        ui.label(RichText::new("机翻断点续翻：进度自动保存在旁车文件 <文件>.mtl.json，中断/失败后重跑自动从断点继续，全部完成后自动删除。本地 Ollama 需先运行 ollama serve。").weak().small());
        ui.add_space(8.0);
        ui.separator();

        // ---- 翻译包：整包备份 / 迁移 / 分享 ----
        ui.horizontal(|ui| {
            ui.label(RichText::new("翻译包").strong());
            ui.label(RichText::new("把 CSV + 注入 JSON + 清单打包成单个 zip；导入后自动归位（JSON 进游戏目录可直接注入，CSV 进输出目录可回填/继续机翻）").weak().small());
        });
        ui.horizontal(|ui| {
            let game_name = self
                .game_root
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "game".into());
            if ui.button("📦 导出翻译包...").clicked() {
                let suggested = format!("{game_name}.stoolpack.zip");
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("翻译包 (zip)", &["zip"])
                    .set_file_name(&suggested)
                    .save_file()
                {
                    let json = PathBuf::from(self.inj_json_str.trim());
                    match crate::features::tpack::export_pack(Some(&self.csv_path), Some(&json), &p, &d.plugin_id, &game_name) {
                        Ok((n, filled)) => {
                            let msg = format!("已打包 {n} 个文件 → {}（JSON 已填 {filled} 条译文）", p.display());
                            self.log(format!("✔ {msg}"));
                            self.toast = Some((msg, std::time::Instant::now()));
                        }
                        Err(e) => self.toast = Some((format!("导出失败: {e}"), std::time::Instant::now())),
                    }
                }
            }
            if ui.button("📥 导入翻译包...").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("翻译包 (zip)", &["zip"]).pick_file() {
                    match crate::features::tpack::import_pack(&p, &self.game_root, &self.out_dir) {
                        Ok(imp) => {
                            let mut parts: Vec<String> = Vec::new();
                            if let Some(jp) = &imp.json_path {
                                self.inj_json_str = jp.display().to_string();
                                parts.push(format!("JSON → {}（已填 {} 条译文，可点\"③ 注入翻译\"）", jp.display(), imp.json_filled));
                            }
                            if let Some(cp) = &imp.csv_path {
                                self.csv_path_str = cp.display().to_string();
                                self.csv_path = cp.clone();
                                parts.push(format!("CSV → {}（可回填/继续机翻）", cp.display()));
                            }
                            let msg = format!("翻译包已导入（目标引擎: {}）: {}", imp.engine, parts.join("；"));
                            self.log(format!("✔ {msg}"));
                            self.toast = Some((msg, std::time::Instant::now()));
                        }
                        Err(e) => self.toast = Some((format!("导入失败: {e}"), std::time::Instant::now())),
                    }
                }
            }
        });
    }
}
