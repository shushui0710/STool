//! 资源预览：识别媒体类型、列出媒体文件、把图片解码为 RGBA 像素（供 GUI 建纹理）。

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Audio,
    Other,
}

impl MediaKind {
    pub fn label(&self) -> &'static str {
        match self {
            MediaKind::Image => "图片",
            MediaKind::Audio => "音频",
            MediaKind::Other => "文件",
        }
    }
    pub fn icon(&self) -> &'static str {
        match self {
            MediaKind::Image => "🖼",
            MediaKind::Audio => "🎵",
            MediaKind::Other => "📄",
        }
    }
}

pub fn kind_of(path: &Path) -> MediaKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "bmp" | "webp" | "gif" | "tga" | "tif" | "tiff" | "qoi" | "ico" => MediaKind::Image,
        "ogg" | "oga" | "wav" | "mp3" | "flac" => MediaKind::Audio,
        _ => MediaKind::Other,
    }
}

/// 媒体列表条目上限：素材目录可能有几万个文件，全量列出会让列表构建与渲染卡顿。
/// 达到上限即提前停止遍历，调用方据「数量 == MAX_MEDIA」判断是否被截断。
pub const MAX_MEDIA: usize = 20_000;

/// 列出目录下的媒体文件（递归，最多 [`MAX_MEDIA`] 条），按 类型→文件名 排序。
pub fn list_media(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(dir).max_depth(6).into_iter().filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_file() && kind_of(p) != MediaKind::Other {
            out.push(p.to_path_buf());
            if out.len() >= MAX_MEDIA {
                break;
            }
        }
    }
    out.sort_by_key(|p| (kind_of(p) == MediaKind::Audio, p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    out
}

/// 解码图片为 RGBA8。返回 (像素, 宽, 高)。超大图拒绝解码以防卡顿。
pub fn decode_image_rgba(data: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let img = image::load_from_memory(data).map_err(|e| format!("图片解码失败: {e}"))?;
    let (w, h) = (img.width(), img.height());
    if w as u64 * h as u64 > 40_000_000 {
        return Err(format!("图片过大（{w}×{h}），不预览"));
    }
    let rgba = img.to_rgba8().into_raw();
    Ok((rgba, w, h))
}

/// 常见音频扩展名能否播放（不支持的格式提前告知，避免用户困惑）。
pub fn audio_supported(path: &Path) -> Option<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "ogg" | "oga" | "wav" | "mp3" | "flac" => None,
        "m4a" | "aac" | "wma" => Some(format!("该格式（{ext}）暂不支持试听，可先用工具转成 ogg/mp3")),
        _ => Some("未知音频格式".to_string()),
    }
}
