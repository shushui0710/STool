//! 页面：preview（P2-11 从 gui.rs 拆出）。

use crate::gui::*;
use std::path::PathBuf;
use eframe::egui::{Color32, RichText};

impl StoolApp {
    pub(crate) fn page_preview(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "资源预览", "解包出来的图片可以直接看，音频可以直接试听；把文件或文件夹拖进窗口也行");

        // 目录选择
        if self.pv_dir_str.is_empty() {
            self.pv_dir_str = self.out_dir.display().to_string();
        }
        ui.horizontal(|ui| {
            ui.label("资源目录:");
            let w = (ui.available_width() - 200.0).max(80.0);
            ui.add_sized([w, 22.0], egui::TextEdit::singleline(&mut self.pv_dir_str));
            if ui.button("浏览...").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.pv_dir_str = p.display().to_string();
                    self.refresh_pv_files();
                    self.pv_sel = None;
                }
            }
            if ui.button("🔄 刷新列表").clicked() {
                self.refresh_pv_files();
            }
        });

        // 左列表 + 右预览
        egui::SidePanel::left("pv_list_panel")
            .resizable(true)
            .default_width(300.0)
            .min_width(180.0)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("筛选:");
                    ui.add_sized([ui.available_width() - 8.0, 20.0], egui::TextEdit::singleline(&mut self.pv_filter));
                });
                ui.label(RichText::new(&self.pv_msg).weak().small());
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let f = self.pv_filter.trim().to_lowercase();
                    for p in self.pv_files.clone() {
                        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if !f.is_empty() && !name.to_lowercase().contains(&f) {
                            continue;
                        }
                        let kind = preview::kind_of(&p);
                        let selected = self.pv_sel.as_deref() == Some(p.as_path());
                        if ui
                            .add(egui::SelectableLabel::new(
                                selected,
                                RichText::new(format!("{} {}", kind.icon(), name)).small(),
                            ))
                            .clicked()
                        {
                            self.pv_sel = Some(p.clone());
                            if kind == preview::MediaKind::Image {
                                self.pv_tex = None; // 触发重载
                                self.pv_tex_key.clear();
                            } else if kind == preview::MediaKind::Audio {
                                self.pv_msg.clear();
                            }
                        }
                    }
                    if self.pv_files.is_empty() {
                        ui.label(RichText::new("（这个目录下没有媒体文件）").weak());
                    }
                });
            });

        // 右侧预览区
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let Some(path) = self.pv_sel.clone() else {
                ui.add_space(ui.available_height() * 0.4);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("👈 从左侧选择一个文件预览").weak());
                });
                ui.add_space(ui.available_height() * 0.4);
                return;
            };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            ui.label(RichText::new(&name).strong());
            match preview::kind_of(&path) {
                preview::MediaKind::Image => {
                    self.load_pv_texture(&path, ui.ctx());
                    if let Some(tex) = &self.pv_tex {
                        let size = tex.size_vec2();
                        let avail = ui.available_size() - egui::vec2(16.0, 90.0);
                        let scale = (avail.x / size.x).min(avail.y / size.y).clamp(0.02, 1.0);
                        let show = size * scale;
                        ui.add_space(6.0);
                        ui.vertical_centered(|ui| {
                            ui.add(egui::Image::new(tex).max_size(show));
                        });
                    }
                    ui.label(RichText::new(&self.pv_msg).weak().small());
                    if name.to_lowercase().ends_with(".gif") {
                        ui.label(RichText::new("GIF 只显示第一帧").weak().small());
                    }
                }
                preview::MediaKind::Audio => {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("🎵").heading());
                        ui.add_space(8.0);
                        if let Some(playing) = &self.pv_playing {
                            if playing == &path {
                                let done = self.pv_audio.as_ref().map(|a| a.sink.empty()).unwrap_or(true);
                                if done {
                                    ui.label(RichText::new("▶ 播放结束").weak());
                                } else {
                                    ui.label(RichText::new("🔊 正在播放…").color(Color32::from_rgb(120, 200, 120)));
                                }
                            }
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let is_playing = self.pv_playing.as_deref() == Some(path.as_path())
                                && self.pv_audio.as_ref().map(|a| !a.sink.empty()).unwrap_or(false);
                            if ui.add_enabled(!is_playing, egui::Button::new("▶ 播放")).clicked() {
                                self.pv_play(&path);
                            }
                            if ui.button("⏹ 停止").clicked() {
                                self.pv_stop();
                            }
                            ui.label("音量:");
                            ui.add(egui::Slider::new(&mut self.pv_volume, 0.0..=1.0).show_value(false));
                        });
                    });
                    if let Some(err) = preview::audio_supported(&path) {
                        ui.label(RichText::new(err).weak().small());
                    }
                    ui.label(RichText::new(&self.pv_msg).weak().small());
                }
                preview::MediaKind::Other => {
                    ui.label(RichText::new("不支持的预览类型").weak());
                }
            }
        });
    }

    pub(crate) fn refresh_pv_files(&mut self) {
        let dir = PathBuf::from(self.pv_dir_str.trim());
        self.pv_files = preview::list_media(&dir);
        self.pv_msg = if self.pv_files.len() >= preview::MAX_MEDIA {
            format!(
                "媒体文件很多，已达上限 {} 个（其余未列出；可改用上面的名称筛选或缩小目录范围）",
                preview::MAX_MEDIA
            )
        } else {
            format!("共 {} 个媒体文件", self.pv_files.len())
        };
    }

    pub(crate) fn load_pv_texture(&mut self, path: &PathBuf, ctx: &egui::Context) {
        let key = path.display().to_string();
        if self.pv_tex_key == key && self.pv_tex.is_some() {
            return;
        }
        match std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|d| preview::decode_image_rgba(&d))
        {
            Ok((rgba, w, h)) => {
                let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                self.pv_tex = Some(ctx.load_texture("pv", img, egui::TextureOptions::LINEAR));
                self.pv_tex_key = key;
                self.pv_msg = format!("{} × {}", w, h);
            }
            Err(e) => {
                self.pv_tex = None;
                self.pv_msg = e;
            }
        }
    }

    pub(crate) fn pv_play(&mut self, path: &PathBuf) {
        // 停掉旧的
        self.pv_stop();
        let file = std::fs::File::open(path).map_err(|e| format!("打开音频失败: {e}"));
        let Ok(file) = file else { return };
        match (rodio::OutputStream::try_default(), rodio::Decoder::new(std::io::BufReader::new(file))) {
            (Ok((stream, handle)), Ok(src)) => match rodio::Sink::try_new(&handle) {
                Ok(sink) => {
                    sink.set_volume(self.pv_volume);
                    sink.append(src);
                    self.pv_audio = Some(AudioOut { _stream: stream, sink });
                    self.pv_playing = Some(path.clone());
                }
                Err(e) => self.pv_msg = format!("创建播放队列失败: {e}"),
            },
            (Err(e), _) => self.pv_msg = format!("无法访问音频设备: {e}"),
            (_, Err(e)) => self.pv_msg = format!("音频解码失败（可能是不支持的格式，如 m4a/aac）: {e}"),
        }
    }

    pub(crate) fn pv_stop(&mut self) {
        if let Some(a) = self.pv_audio.take() {
            a.sink.stop();
        }
        self.pv_playing = None;
    }
}
