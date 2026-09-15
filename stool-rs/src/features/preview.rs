//! 资源预览：识别媒体类型、列出媒体文件、把图片解码为 RGBA 像素（供 GUI 建纹理）。
//!
//! 「这是什么类型的文件」「这段字节是什么编码」这两件事**只在这里定义一次** ——
//! 界面（Tauri 版）从这里取，别再写一份分类表（漂移的表现是
//! 「同一个文件在预览页能看、在别处被当成不支持」）。

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Audio,
    Text,
    Other,
}

impl MediaKind {
    pub fn label(&self) -> &'static str {
        match self {
            MediaKind::Image => "图片",
            MediaKind::Audio => "音频",
            MediaKind::Text => "文本",
            MediaKind::Other => "文件",
        }
    }
    pub fn icon(&self) -> &'static str {
        match self {
            MediaKind::Image => "🖼",
            MediaKind::Audio => "🎵",
            MediaKind::Text => "📝",
            MediaKind::Other => "📄",
        }
    }
    /// 给界面用的短 id。前端只认这个字符串，**不要去 match 中文标签**。
    pub fn id(&self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::Audio => "audio",
            MediaKind::Text => "text",
            MediaKind::Other => "other",
        }
    }
}

/// 图片扩展名。注意其中 `.tga` / `.tif` / `.qoi` 浏览器渲染不了 ——
/// 它们仍归为「图片」，但 [`mime_of`] 认不出来，界面据此降级成「用系统程序打开」。
const IMAGE_EXTS: [&str; 11] =
    ["png", "jpg", "jpeg", "bmp", "webp", "gif", "tga", "tif", "tiff", "qoi", "ico"];

/// 音频扩展名（拿不到内核解码器的格式不列，免得列出来却放不了）。
const AUDIO_EXTS: [&str; 5] = ["ogg", "oga", "wav", "mp3", "flac"];

/// 按**文本**处理的扩展名 —— `.ks` / `.tjs` 是 KiriKiri 的明文脚本，
/// `.csv` / `.json` 是翻译中间产物，都是这条工具链里真会去翻的东西。
const TEXT_EXTS: [&str; 14] =
    ["txt", "ks", "tjs", "csv", "tsv", "json", "md", "xml", "html", "htm", "ini", "cfg", "log", "yaml"];

pub fn kind_of(path: &Path) -> MediaKind {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        return MediaKind::Image;
    }
    if AUDIO_EXTS.contains(&ext.as_str()) {
        return MediaKind::Audio;
    }
    if TEXT_EXTS.contains(&ext.as_str()) {
        return MediaKind::Text;
    }
    MediaKind::Other
}

/// 浏览器认得的 MIME（把文件内联成 data URL 时用）。
/// 认不出来返回 `application/octet-stream`，调用方据此改用「用系统程序打开」。
pub fn mime_of(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "ogg" | "oga" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "txt" | "ks" | "tjs" | "csv" | "tsv" | "json" | "md" | "xml" | "html" | "htm" | "ini" | "cfg"
        | "log" | "yaml" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// 文本预览的读取上限。脚本文件可能很大，但预览只需要开头一段。
pub const TEXT_PREVIEW_MAX: usize = 256 * 1024;

/// 替换字符占比超过这个值（1/8），就判定「这根本不是文本」。
///
/// 为什么需要这道闸：`.ks` / `.csv` 这些扩展名**不等于**内容是文本 ——
/// 从**加密封包**里取出来的同名文件就是密文（实测高熵，64 字节里 44~52 个互不相同）。
/// 没有这道闸时，`decode_text` 的兜底分支会把密文变成一屏 `U+FFFD` 还给界面，
/// 用户看到的就是「一屏乱码」，比明确说「这个看不了」还糟。
const REPLACEMENT_CHAR_LIMIT: usize = 8;

/// 半角片假名（U+FF61–U+FF9F）占比上限。
///
/// 这是**单独一道闸**，因为光看替换字符挡不住 Shift-JIS 的假阳性：
/// 随机字节里有相当比例（约 0.6 的平方）会落进 Shift-JIS 的**单字节半角片假名区**，
/// 于是密文能被"成功"解码成一屏 `ﾏ0沢Sｴ` 这类东西 —— 零替换字符，却完全是垃圾。
///
/// 真机实测分界很清楚：正常日文脚本（Shift-JIS / UTF-16）半角片假名约 1~4%，
/// 而加密残留高达 12~33%。取 8% 落在两者中间，两侧都有余量。
const HALF_WIDTH_KANA_LIMIT: usize = 12; // 占比超过 1/12 即拒

/// [`decode_text`] 的结果。分「解出来了」与「这不是文本」两种，
/// 让调用方能给出**有出路**的提示，而不是显示一屏替换字符。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedText {
    /// 解出的文本 + 编码名（UTF-8 / Shift-JIS / UTF-16LE…）
    Ok(String, &'static str),
    /// 按已知编码都解不出人话 —— 大概率是二进制或密文（例如加密 xp3 里的残留）
    Binary,
}

/// 解码文本。
///
/// **不能直接 `from_utf8_lossy`**：这条工具链里的脚本大量是 **Shift-JIS**（日文老游戏）
/// 或 **UTF-16**（KiriKiri 的 `.tjs` 常是**无 BOM** 的 UTF-16LE），按 UTF-8 读会得到一屏乱码，
/// 「看文本」这一页就白做了。
///
/// 判定顺序：BOM → 严格 UTF-8 → 无 BOM 的 UTF-16（靠 NUL 分布判）→ Shift-JIS（带质量门槛）
/// → 都不行就是 [`DecodedText::Binary`]。
pub fn decode_text(bytes: &[u8]) -> DecodedText {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return DecodedText::Ok(String::from_utf8_lossy(rest).into_owned(), "UTF-8 (BOM)");
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return DecodedText::Ok(String::from_utf16_lossy(&to_u16(rest, true)), "UTF-16LE");
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return DecodedText::Ok(String::from_utf16_lossy(&to_u16(rest, false)), "UTF-16BE");
    }
    // 无 BOM 的 UTF-16 必须排在**严格 UTF-8 之前**：
    // 纯 ASCII 的 UTF-16LE 是 `x, 0x00` 交替，而 NUL 是合法 UTF-8 字符，
    // 于是它会「成功地」按 UTF-8 解出 `/\0/\0 \0C\0...` 这种带满 NUL 的串 ——
    // 看起来没报错，实际完全没法读。真机 `tail_test` 里 KiriKiri 的 .tjs 正是这种。
    if let Some((le, name)) = sniff_utf16(bytes) {
        return DecodedText::Ok(String::from_utf16_lossy(&to_u16(bytes, le)), name);
    }
    // 严格 UTF-8 能过就是它，没有歧义。
    if let Ok(s) = std::str::from_utf8(bytes) {
        return DecodedText::Ok(s.to_string(), "UTF-8");
    }
    // Shift-JIS 是日文游戏的默认编码。两道闸一起卡：替换字符 + 半角片假名。
    let (cow, _, _) = encoding_rs::SHIFT_JIS.decode(bytes);
    let s = cow.into_owned();
    if acceptable(&s) {
        return DecodedText::Ok(s, "Shift-JIS");
    }
    // 兜底也不能无脑有损 UTF-8：密文在这里会被解成一屏替换字符，
    // 与其给用户看乱码，不如老实说「这不是文本」。
    let lossy = String::from_utf8_lossy(bytes).into_owned();
    if acceptable(&lossy) {
        return DecodedText::Ok(lossy, "未知编码（按 UTF-8 有损显示）");
    }
    DecodedText::Binary
}

/// 判定这算不算「可读文本」。见 [`REPLACEMENT_CHAR_LIMIT`] 与 [`HALF_WIDTH_KANA_LIMIT`]。
fn acceptable(s: &str) -> bool {
    let total = s.chars().count();
    if total == 0 {
        return true; // 空文件按空文本显示，不算「不是文本」
    }
    let bad = s.chars().filter(|c| *c == '\u{FFFD}').count();
    if bad * REPLACEMENT_CHAR_LIMIT > total {
        return false;
    }
    let half = s.chars().filter(|c| ('\u{FF61}'..='\u{FF9F}').contains(c)).count();
    half * HALF_WIDTH_KANA_LIMIT <= total
}

/// 无 BOM 的 UTF-16 嗅探：看 NUL 字节是不是**规律地**落在奇/偶位。
///
/// 只要文件里有非 ASCII 内容（任何中日文脚本都有），UTF-16 的编码字节会是
/// 「非零, 0x00」或「0x00, 非零」交替出现 —— 这是 UTF-16 独有的强特征，
/// Shift-JIS / UTF-8 不会产生大量 NUL。
fn sniff_utf16(b: &[u8]) -> Option<(bool, &'static str)> {
    let n = b.len();
    if n < 8 {
        return None;
    }
    // 只看一个上限，避免对超大文件白扫一遍
    let sample = &b[..n.min(4096)];
    let mut even = 0usize; // 下标为偶数的位置上的 0x00 个数
    let mut odd = 0usize;
    for (i, &x) in sample.iter().enumerate() {
        if x == 0 {
            if i % 2 == 0 {
                even += 1
            } else {
                odd += 1
            }
        }
    }
    let half_slots = sample.len() / 2;
    // 「该位上有一半以上是 NUL」才算规律 —— 阈值放宽到 40% 以兼容纯 ASCII 的 UTF-16
    // （纯 ASCII 的 UTF-16LE 是 `x, 0x00` 交替，奇数位几乎全是 NUL）。
    let le_hit = odd * 10 >= half_slots * 4;
    let be_hit = even * 10 >= half_slots * 4;
    match (le_hit, be_hit) {
        // 只有一边命中才敢下结论；两边都命中说明这不是 UTF-16
        (true, false) => Some((true, "UTF-16LE（无 BOM）")),
        (false, true) => Some((false, "UTF-16BE（无 BOM）")),
        _ => None,
    }
}

fn to_u16(b: &[u8], le: bool) -> Vec<u16> {
    b.as_chunks::<2>()
        .0
        .iter()
        .map(|c| if le { u16::from_le_bytes(*c) } else { u16::from_be_bytes(*c) })
        .collect()
}

/// 媒体列表条目上限：素材目录可能有几万个文件，全量列出会让列表构建与渲染卡顿。
/// 达到上限即提前停止遍历，调用方据「数量 == MAX_MEDIA」判断是否被截断。
pub const MAX_MEDIA: usize = 20_000;

/// 列表里的分组顺序：图片 → 音频 → 文本。
fn kind_rank(k: MediaKind) -> u8 {
    match k {
        MediaKind::Image => 0,
        MediaKind::Audio => 1,
        MediaKind::Text => 2,
        MediaKind::Other => 3,
    }
}

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
    out.sort_by_key(|p| {
        (kind_rank(kind_of(p)), p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_of_classifies_image_audio_text() {
        assert_eq!(kind_of(Path::new("a.PNG")), MediaKind::Image);
        assert_eq!(kind_of(Path::new("cg.tga")), MediaKind::Image);
        assert_eq!(kind_of(Path::new("bgm.ogg")), MediaKind::Audio);
        assert_eq!(kind_of(Path::new("scene.KS")), MediaKind::Text);
        assert_eq!(kind_of(Path::new("t.csv")), MediaKind::Text);
        // KiriKiri 的 .scn 是编译过的，不是文本 —— 当文本显示只会是一屏乱码
        assert_eq!(kind_of(Path::new("x.scn")), MediaKind::Other);
        assert_eq!(kind_of(Path::new("noext")), MediaKind::Other);
    }

    #[test]
    fn kind_id_is_stable_english() {
        // 前端只认这个 id；改中文标签不该影响它
        assert_eq!(MediaKind::Image.id(), "image");
        assert_eq!(MediaKind::Audio.id(), "audio");
        assert_eq!(MediaKind::Text.id(), "text");
        assert_eq!(MediaKind::Other.id(), "other");
    }

    #[test]
    fn mime_covers_only_browser_renderable() {
        assert_eq!(mime_of(Path::new("a.png")), "image/png");
        assert_eq!(mime_of(Path::new("a.ogg")), "audio/ogg");
        assert_eq!(mime_of(Path::new("a.ks")), "text/plain");
        // tga/tif 算「图片」，但浏览器渲染不了 → 必须落 octet-stream，
        // 界面据此降级成「用系统程序打开」，而不是塞给 <img> 一个打不开的东西
        assert_eq!(mime_of(Path::new("cg.tga")), "application/octet-stream");
        assert_eq!(mime_of(Path::new("cg.tif")), "application/octet-stream");
    }

    #[test]
    fn decode_text_handles_bom_utf8_and_sjis() {
        let ok = |b: &[u8]| match decode_text(b) {
            DecodedText::Ok(s, e) => (s, e),
            DecodedText::Binary => panic!("本该解出文本，却被判为二进制"),
        };

        assert_eq!(ok(b"hello").1, "UTF-8");
        assert_eq!(ok("\u{FEFF}hi".as_bytes()).1, "UTF-8 (BOM)");
        assert_eq!(ok(&[0xFF, 0xFE, b'h', 0, b'i', 0]), ("hi".into(), "UTF-16LE"));
        assert_eq!(ok(&[0xFE, 0xFF, 0, b'h', 0, b'i']), ("hi".into(), "UTF-16BE"));

        // 「こんにちは」的 Shift-JIS 字节：不是合法 UTF-8，必须被认出来而不是显示乱码
        let sjis = [0x82, 0xB1, 0x82, 0xF1, 0x82, 0xC9, 0x82, 0xBF, 0x82, 0xCD];
        assert_eq!(ok(&sjis), ("こんにちは".into(), "Shift-JIS"));

        // 同样的字，UTF-16LE 带 BOM
        let mut u16le = vec![0xFF, 0xFE];
        for c in "こんにちは".encode_utf16() {
            u16le.extend_from_slice(&c.to_le_bytes());
        }
        assert_eq!(ok(&u16le), ("こんにちは".into(), "UTF-16LE"));

        // 纯 ASCII 以外的合法 UTF-8 也要走 UTF-8 分支（别误判成 Shift-JIS）
        assert_eq!(ok("中文测试".as_bytes()), ("中文测试".into(), "UTF-8"));
    }

    #[test]
    fn decode_text_calls_binary_binary_instead_of_showing_garbage() {
        // 加密 xp3 里取出的 `.ks` 就是这种高熵字节（真机 komoguri 样本实测 44/64 互不相同）。
        // 以前会走「有损 UTF-8」还给界面一屏 U+FFFD；现在必须判定为「不是文本」。
        let cipher: Vec<u8> = (0u16..512).map(|i| (i.wrapping_mul(97).wrapping_add(13) & 0xFF) as u8).collect();
        assert_eq!(decode_text(&cipher), DecodedText::Binary);

        // 全 0xFF 同理
        assert_eq!(decode_text(&[0xFFu8; 256]), DecodedText::Binary);

        // 空文件不算「不是文本」—— 显示成空文本更合理
        assert!(matches!(decode_text(b""), DecodedText::Ok(_, _)));
    }

    #[test]
    fn decode_text_rejects_sjis_half_width_kana_garbage() {
        // 单独一道闸的意义：随机字节能被 Shift-JIS「成功」解码成一屏半角片假名，
        // 替换字符占比却不高 —— 只靠替换字符闸是漏的。
        // 这批字节在真机上就是加密残留（`komoguri_out` 里的 .ks/.tjs）。
        let mut kana = Vec::new();
        for i in 0..200u32 {
            // 0xA1..0xDF 在 Shift-JIS 里正是单字节半角片假名区
            kana.push((0xA1 + (i * 7) % 0x3F) as u8);
        }
        assert_eq!(decode_text(&kana), DecodedText::Binary, "一屏半角片假名不该当成文本");
    }

    #[test]
    fn decode_text_sniffs_bomless_utf16() {
        // 真机 `verify/tail_test/out/unencrypted/*.tjs` 就是**无 BOM 的 UTF-16LE**。
        // 没有这一步会掉进 Shift-JIS，把 `//` 解成 `ｰtスﾅ` 这类东西。
        let src = "// Config.tjs - KAG\nglobal.config_version = \"EX 3.27\";\n";
        let mut le = Vec::new();
        for c in src.encode_utf16() {
            le.extend_from_slice(&c.to_le_bytes());
        }
        assert_eq!(decode_text(&le), DecodedText::Ok(src.into(), "UTF-16LE（无 BOM）"));

        let mut be = Vec::new();
        for c in src.encode_utf16() {
            be.extend_from_slice(&c.to_be_bytes());
        }
        assert_eq!(decode_text(&be), DecodedText::Ok(src.into(), "UTF-16BE（无 BOM）"));
    }

    #[test]
    fn normal_japanese_scripts_survive_both_gates() {
        // 反例保护：闸门不能把**正常**日文脚本一起挡掉。
        let sjis_script = "*start\n【朔】\n　おはよう。今日もいい天気だね。\n　それじゃ、行こうか。\n";
        let bytes: Vec<u8> = {
            let (cow, _, _) = encoding_rs::SHIFT_JIS.encode(sjis_script);
            cow.into_owned()
        };
        assert_eq!(decode_text(&bytes), DecodedText::Ok(sjis_script.into(), "Shift-JIS"));

        // UTF-8 的中文/日文混排同样不能被误伤
        let mixed = "【朔】おはよう。今日もいい天気だね。混合中文文本。";
        assert_eq!(decode_text(mixed.as_bytes()), DecodedText::Ok(mixed.into(), "UTF-8"));
    }
}
