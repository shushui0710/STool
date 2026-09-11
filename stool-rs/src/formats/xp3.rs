//! KiriKiri XP3 封包。
//!
//! 两代头部（读侧都支持，写侧可指定）：
//! - **V1**（KiriKiri 2.28 及以前 / GARbro v1）：`magic(11) + u64 索引偏移`，数据区起点 19；
//! - **V2**（KiriKiri 2.30+ / 吉里吉里Z，GARbro 默认，实测所有真机样本都是这代）：
//!   `magic(11) + u64 0x17 + u32 次版本 + u8 0x80 + u64 保留 + u64 索引偏移`，
//!   数据区起点 40。
//!
//! 索引区（两代相同，GARbro 布局）：`[u8 压缩标记][u64 压缩后长][u64 原始长][zlib 索引]`
//! （未压缩时为 `[u8 0][u64 索引长][索引]`），索引体是 `File` 条目流，
//! 每项 = `[u32 "File"][u64 内容长]` + `info`/`segm`/`adlr` 子块（子块长度字段为 u64）。
//!
//! 另有几种曾被误当成“伪装头”的变体（Hxv4 加密、目录追加在文件尾等），
//! 由 fallback 分支尽力识别。

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::io::Seek;
use std::path::Path;

use super::source::Source;

pub const MAGIC: &[u8] = b"XP3\r\n \n\x1a\x8b\x67\x01";

/// 索引区单次读取上限：XP3 的索引恒在文件尾部，正常只有几百 KB ~ 几 MB。
const TOC_CAP: usize = 256 * 1024 * 1024;
/// 「目录在文件尾」变体扫描的尾部窗口大小。
const TAIL_WINDOW: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Segment {
    pub offset: u64,
    pub orig_size: u64,
    pub archive_size: u64,
}

#[derive(Debug, Clone)]
pub struct Xp3Entry {
    pub name: String,
    pub segments: Vec<Segment>,
    pub protected: u32,
}

pub fn parse(path: &Path) -> Result<BTreeMap<String, Xp3Entry>, String> {
    let data = fs::read(path).map_err(|e| e.to_string())?;
    parse_bytes(&data)
}

pub fn parse_bytes(data: &[u8]) -> Result<BTreeMap<String, Xp3Entry>, String> {
    use crate::formats::safe as sf;
    if !data.starts_with(MAGIC) {
        return Err("不是 XP3 封包".into());
    }
    // MAGIC 仅 11 字节，flag 在第 12 字节——必须单独判边界，否则 11 字节伪文件会 panic
    let flag = sf::u8_at(data, 11).ok_or("XP3 头部不完整（缺少 flag 字节）")?;
    if flag == 0x17 {
        return parse_disguised(data);
    }
    // V1（KiriKiri 2.28- / GARbro v1）：magic 之后直接是 u64 索引偏移。
    // 先于「未知 flag」分支尝试，命中即用；否则再走尾目录启发式。
    if let Some(files) = try_legacy_toc(data) {
        return Ok(files);
    }
    if flag != 0x00 && flag != 0x80 {
        // 未知 flag：常见于网盘发布游戏的「头部伪装」翻译补丁
        // （目录被追加在文件尾，或目录整体加密）
        if let Some(files) = parse_tail_toc(data) {
            return Ok(files);
        }
        return Err("XP3 索引无法识别（翻译补丁/保护变体：目录可能被加密）".into());
    }
    let index_pos: u64 = if flag == 0x80 {
        sf::u64_le(data, 12).ok_or("XP3 头部截断（索引偏移缺失）")?
    } else {
        sf::u32_le(data, 12).ok_or("XP3 头部截断（索引偏移缺失）")? as u64
    };
    if index_pos as usize >= data.len() {
        return Err("XP3 索引偏移越界（可能为加密变体）".into());
    }
    let toc = decode_toc(&data[index_pos as usize..])?;
    Ok(walk_std_toc(&toc))
}

/// 解开索引区首字节标记：0x01/0x80 = zlib，其它按裸块。
fn decode_toc(raw: &[u8]) -> Result<Vec<u8>, String> {
    let toc: Vec<u8> = match raw.first() {
        Some(0x01) | Some(0x80) => {
            let mut z = flate2::read::ZlibDecoder::new(&raw[1..]);
            let mut out = Vec::new();
            z.read_to_end(&mut out).map_err(|e| e.to_string())?;
            out
        }
        _ => raw.get(1..).unwrap_or(&[]).to_vec(),
    };
    if toc.len() < 4 {
        return Err("XP3 TOC 过短".into());
    }
    Ok(toc)
}

/// 标准 XP3 索引体：[u32 条目数] 后跟 N ×（[u32 块长][info/segm/adlr 子块]）。
fn walk_std_toc(toc: &[u8]) -> BTreeMap<String, Xp3Entry> {
    let count = u32::from_le_bytes(toc[0..4].try_into().unwrap());
    let mut p = 4usize;
    let mut files = BTreeMap::new();
    for _ in 0..count {
        if p + 4 > toc.len() {
            break;
        }
        let chunk_size = u32::from_le_bytes(toc[p..p + 4].try_into().unwrap()) as usize;
        p += 4;
        if p + chunk_size > toc.len() {
            break;
        }
        let chunk = &toc[p..p + chunk_size];
        p += chunk_size;
        let mut name = String::new();
        let mut segments = Vec::new();
        let mut protected = 0u32;
        let mut q = 0usize;
        while q + 8 <= chunk.len() {
            let tag = &chunk[q..q + 4];
            let size = u32::from_le_bytes(chunk[q + 4..q + 8].try_into().unwrap()) as usize;
            let body = &chunk[(q + 8).min(chunk.len())..(q + 8 + size).min(chunk.len())];
            match tag {
                b"info" if body.len() >= 22 => {
                    protected = u32::from_le_bytes(body[0..4].try_into().unwrap());
                    let nlen = u16::from_le_bytes(body[20..22].try_into().unwrap()) as usize;
                    let nb = &body[22..(22 + nlen * 2).min(body.len())];
                    name = String::from_utf16_lossy(
                        &nb.chunks(2).map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]))
                            .collect::<Vec<u16>>(),
                    );
                }
                b"segm" if body.len() >= 4 => {
                    // 标准 XP3 segm：u32 计数（恒为 1）+ 每段 24 字节（offset/orig/archive）
                    let n = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
                    for i in 0..n {
                        let off = 4 + i * 24;
                        if off + 24 <= body.len() {
                            let (aoff, osize, asize) = three_u64(&body[off..off + 24]);
                            segments.push(Segment { offset: aoff, orig_size: osize, archive_size: asize });
                        }
                    }
                }
                _ => {}
            }
            q += 8 + size;
        }
        files.insert(name, Xp3Entry { name: String::new(), segments, protected });
    }
    files
}

fn three_u64(b: &[u8]) -> (u64, u64, u64) {
    (
        u64::from_le_bytes(b[0..8].try_into().unwrap()),
        u64::from_le_bytes(b[8..16].try_into().unwrap()),
        u64::from_le_bytes(b[16..24].try_into().unwrap()),
    )
}

/// 「头部伪装」变体（flag 字节 0x17，常见于带保护补丁的 Kirikiri 游戏）：
///   - 头部固定 40 字节，真实索引位置为偏移 32 处的 u64；
///   - TOC 首字节为格式标记：0x00 = 裸块，0x01 = zlib 压缩；
///     - 0x00：[0x00][u64 条目区总长] + 条目流；
///     - 0x01：[0x01][u64 压缩长][u64 解压长][zlib 流]（OPPAI 系游戏；解压后若
///       以 "Hxv4" 开头则文件名被合成字符混淆且数据段加密，属保护类，明确报错）；
///   - 每条目 = [b"File"][u64 内容长] + info/segm/adlr 子块，
///     子块 = [tag 4B][u64 长度][数据]（长度字段为 u64，非标准的 u32）；
///   - info 数据 = [u32 protect][u64 原始大小][u64 归档大小][u16 名长][名字 UTF-16LE]；
///   - segm 数据 = N × [u32 压缩 id][u64 偏移][u64 原始大小][u64 归档大小]（无计数前缀）。
fn parse_disguised(data: &[u8]) -> Result<BTreeMap<String, Xp3Entry>, String> {
    if data.len() < 40 {
        return Err("XP3（伪装头）头部不完整".into());
    }
    let index_pos = u64::from_le_bytes(data[32..40].try_into().unwrap()) as usize;
    if index_pos >= data.len() {
        return Err("XP3（伪装头）索引偏移越界".into());
    }
    walk_disguised_toc(&data[index_pos..])
}

/// 解开 GARbro 布局的索引区（V1/V2 通用）：
/// `[u8 0x01][u64 压缩后长][u64 原始长][zlib 数据]` 或 `[u8 0x00][u64 索引长][索引]`。
/// 不识别（标记非 0/1、长度越界等）返回 None。
fn decode_garbro_toc(raw: &[u8]) -> Option<Vec<u8>> {
    match raw.first()? {
        0x01 if raw.len() >= 17 => {
            let packed = u64::from_le_bytes(raw[1..9].try_into().ok()?) as usize;
            let end = (17usize).saturating_add(packed).min(raw.len());
            let mut z = flate2::read::ZlibDecoder::new(&raw[17..end]);
            let mut out = Vec::new();
            z.read_to_end(&mut out).ok()?;
            Some(out)
        }
        0x00 if raw.len() >= 9 => {
            let size = u64::from_le_bytes(raw[1..9].try_into().ok()?) as usize;
            // 声明长度异常（0 或超出剩余）时退回「到文件尾」，兼容写出方不填长度的变体
            let avail = raw.len() - 9;
            let end = if size == 0 || size > avail { raw.len() } else { 9 + size };
            Some(raw[9..end].to_vec())
        }
        _ => None,
    }
}

/// V1 旧版头：整包 `data` 的偏移 11 处是 u64 索引偏移（**绝对**文件位置）。
/// 偏移合法且指向一段能解开的 `File` 条目流时返回索引，否则 None（留给其它分支）。
fn try_legacy_toc(data: &[u8]) -> Option<BTreeMap<String, Xp3Entry>> {
    if data.len() < 19 {
        return None;
    }
    let pos = u64::from_le_bytes(data[11..19].try_into().ok()?) as usize;
    if pos >= data.len() {
        return None;
    }
    let buf = decode_garbro_toc(&data[pos..])?;
    if !buf.starts_with(b"File") {
        return None;
    }
    walk_variant_entries(&buf).ok().filter(|m| !m.is_empty())
}

/// 解开伪装头变体的索引区（[标记][长度…] + `File` 条目流）。
fn walk_disguised_toc(raw: &[u8]) -> Result<BTreeMap<String, Xp3Entry>, String> {
    if raw.is_empty() {
        return Err("XP3（伪装头）TOC 为空".into());
    }
    let compressed = raw[0] == 0x01;
    let buf = decode_garbro_toc(raw).ok_or("XP3（伪装头）TOC 头部不识别")?;
    if buf.starts_with(b"Hxv4") {
        return Err("Hxv4 保护变体：目录可读但文件名为合成字符且数据段加密，暂不支持提取".into());
    }
    // 条目流起点：裸块从 0 开始；zlib 解压后跳过可能存在的私有头（找到首个 File 标签）
    let start = if compressed {
        match buf.windows(4).position(|w| w == b"File") {
            Some(p) => p,
            None => return Err("XP3（伪装头）TOC 中未找到条目".into()),
        }
    } else {
        0
    };
    walk_variant_entries(&buf[start..])
}

/// 遍历伪装系变体的条目流：[File][u64 长度][info/segm/adlr 子块（u64 长度字段）]。
fn walk_variant_entries(buf: &[u8]) -> Result<BTreeMap<String, Xp3Entry>, String> {
    let mut files = BTreeMap::new();
    let mut p = 0usize;
    while p + 12 <= buf.len() {
        if &buf[p..p + 4] != b"File" {
            return Err(format!("XP3（伪装头）TOC 条目标签异常 @ +{p}"));
        }
        let content = u64::from_le_bytes(buf[p + 4..p + 12].try_into().unwrap()) as usize;
        p += 12;
        let cend = (p + content).min(buf.len());
        let mut name = String::new();
        let mut segments = Vec::new();
        let mut protected = 0u32;
        let mut q = p;
        while q + 12 <= cend {
            let tag = &buf[q..q + 4];
            let len = u64::from_le_bytes(buf[q + 4..q + 12].try_into().unwrap()) as usize;
            q += 12;
            let body = &buf[q..(q + len).min(cend)];
            q += len;
            match tag {
                b"info" if body.len() >= 22 => {
                    protected = u32::from_le_bytes(body[0..4].try_into().unwrap());
                    let nlen = u16::from_le_bytes(body[20..22].try_into().unwrap()) as usize;
                    let nb = &body[22..(22 + nlen * 2).min(body.len())];
                    name = String::from_utf16_lossy(
                        &nb.chunks(2).map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]))
                            .collect::<Vec<u16>>(),
                    );
                }
                b"segm" if body.len() >= 28 => {
                    for i in 0..body.len() / 28 {
                        let off = i * 28;
                        // body 起点 = q - len；body[off..off+4] = cid，off+4..off+28 = 偏移/原始/归档
                        let (aoff, osize, asize) = three_u64(&buf[q - len + off + 4..][..24]);
                        // cid: 0=明文，1=zlib（读侧按压缩比自适应），2=明文；统一交给 read_file
                        segments.push(Segment { offset: aoff, orig_size: osize, archive_size: asize });
                    }
                }
                _ => {}
            }
        }
        if !name.is_empty() {
            files.insert(name.clone(), Xp3Entry { name, segments, protected });
        }
        p = cend;
    }
    Ok(files)
}

/// 「目录在文件尾」变体（常见于翻译补丁，flag 字节为 0xaf/0xb5/0xc9 等任意值）：
///   - 头部 [magic][flag][u32 数据区长][u16 0] + zlib 免责声明等伪装内容；
///   - 条目区追加在文件最末：[u64 条目区总长][条目流]（与伪装头变体同格式），
///     u64 恰等于“文件大小 − 条目区起点”。
///
/// 定位方式：在尾部窗口找最小的 File 标签位置 s，使 u64@(s-8) == 文件大小 - s，
/// 且从 s 起的条目链恰好走到文件尾。
fn parse_tail_toc(data: &[u8]) -> Option<BTreeMap<String, Xp3Entry>> {
    let sz = data.len() as u64;
    if sz < 1024 {
        return None;
    }
    let win = TAIL_WINDOW.min(sz) as usize;
    let base = sz - win as u64;
    find_tail_toc(&data[base as usize..], base, sz)
}

/// 在尾部窗口里找「条目区起点」：最小的 `File` 位置 p，使
/// u64@(p-8) == 文件总长 − p（绝对值），且从 p 起的条目链恰好走到文件尾。
///
/// `buf` 是 [base, 文件尾) 的窗口切片，因此 p 的绝对位置 = base + p_rel。
fn find_tail_toc(buf: &[u8], base: u64, total_len: u64) -> Option<BTreeMap<String, Xp3Entry>> {
    let mut i = 0usize;
    while i + 4 <= buf.len() {
        let p = {
            let off = buf[i..].windows(4).position(|w| w == b"File")?;
            i + off
        };
        // p >= 8 才能读到前面的 u64 总长，且保证 base+p >= 8 时该 u64 落在窗口内
        if p >= 8 {
            let abs = base + p as u64;
            let total = u64::from_le_bytes(buf[p - 8..p].try_into().unwrap());
            if total == total_len - abs {
                if let Ok(files) = walk_variant_entries(&buf[p..]) {
                    if !files.is_empty() {
                        return Some(files);
                    }
                }
            }
        }
        i = p + 4;
    }
    None
}

/// 读取一个文件的数据（zlib 压缩段自动解压）。
pub fn read_file(data: &[u8], entry: &Xp3Entry) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for seg in &entry.segments {
        if seg.archive_size == 0 {
            continue;
        }
        let start = seg.offset as usize;
        if start > data.len() {
            return Err("数据段越界（可能为加密变体）".into());
        }
        // 饱和夹取，避免伪造 archive_size 触发整数溢出 panic
        let end = start.saturating_add(seg.archive_size as usize).min(data.len());
        let blob = &data[start..end];
        // 压缩判定：压缩大小与原大小不同则尝试 zlib（小文件压完可能更大），
        // 解压失败或大小不符则按明文回退
        let decoded = if seg.archive_size != seg.orig_size {
            let mut z = flate2::read::ZlibDecoder::new(blob);
            let mut d = Vec::new();
            match z.read_to_end(&mut d) {
                Ok(_) if d.len() as u64 == seg.orig_size => Some(d),
                _ => None,
            }
        } else {
            None
        };
        match decoded {
            Some(d) => out.extend_from_slice(&d),
            None => out.extend_from_slice(blob),
        }
    }
    Ok(out)
}

// ---------- 流式 API（P2-1）：只读索引 + 按需读条目 ----------

/// 只读索引（不整包读入内存），覆盖标准 / 伪装头 / 目录在文件尾三种变体。
pub fn parse_index(src: &mut Source) -> Result<BTreeMap<String, Xp3Entry>, String> {
    let len = src.len();
    if len < 12 {
        return Err("XP3 头部不完整".into());
    }
    let head = src.read_exact_at(0, 12)?;
    if !head.starts_with(MAGIC) {
        return Err("不是 XP3 封包".into());
    }
    let flag = head[11];
    if flag == 0x17 {
        let index_pos = u64::from_le_bytes(src.read_exact_at(32, 8)?.try_into().unwrap());
        if index_pos >= len {
            return Err("XP3（伪装头）索引偏移越界".into());
        }
        let raw = src.read_tail(index_pos, TOC_CAP)?;
        return walk_disguised_toc(&raw);
    }
    // V1（KiriKiri 2.28- / GARbro v1）：magic 之后直接是 u64 索引偏移
    if let Some(files) = try_legacy_toc_src(src)? {
        return Ok(files);
    }
    if flag != 0x00 && flag != 0x80 {
        if let Some(files) = parse_tail_toc_src(src)? {
            return Ok(files);
        }
        return Err("XP3 索引无法识别（翻译补丁/保护变体：目录可能被加密）".into());
    }
    let index_pos = if flag == 0x80 {
        u64::from_le_bytes(src.read_exact_at(12, 8)?.try_into().unwrap())
    } else {
        u32::from_le_bytes(src.read_exact_at(12, 4)?.try_into().unwrap()) as u64
    };
    if index_pos >= len {
        return Err("XP3 索引偏移越界（可能为加密变体）".into());
    }
    let raw = src.read_tail(index_pos, TOC_CAP)?;
    let toc = decode_toc(&raw)?;
    Ok(walk_std_toc(&toc))
}

/// 流式版的 V1 头尝试（语义同 `try_legacy_toc`）。
fn try_legacy_toc_src(src: &mut Source) -> Result<Option<BTreeMap<String, Xp3Entry>>, String> {
    let len = src.len();
    if len < 27 {
        return Ok(None);
    }
    let pos = u64::from_le_bytes(src.read_exact_at(11, 8)?.try_into().unwrap());
    if pos >= len {
        return Ok(None);
    }
    let raw = src.read_tail(pos, TOC_CAP)?;
    let Some(buf) = decode_garbro_toc(&raw) else { return Ok(None) };
    if !buf.starts_with(b"File") {
        return Ok(None);
    }
    match walk_variant_entries(&buf) {
        Ok(m) if !m.is_empty() => Ok(Some(m)),
        _ => Ok(None),
    }
}

fn parse_tail_toc_src(src: &mut Source) -> Result<Option<BTreeMap<String, Xp3Entry>>, String> {
    let sz = src.len();
    if sz < 1024 {
        return Ok(None);
    }
    let win = TAIL_WINDOW.min(sz) as usize;
    let base = sz - win as u64;
    let buf = src.read_at(base, win)?;
    Ok(find_tail_toc(&buf, base, sz))
}

/// 按需读取一个条目（zlib 压缩段自动解压），不整包读入。
pub fn read_entry(src: &mut Source, entry: &Xp3Entry) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for seg in &entry.segments {
        if seg.archive_size == 0 {
            continue;
        }
        let blob = src.read_at(seg.offset, seg.archive_size as usize)?;
        let decoded = if seg.archive_size != seg.orig_size {
            let mut z = flate2::read::ZlibDecoder::new(&blob[..]);
            let mut d = Vec::new();
            match z.read_to_end(&mut d) {
                Ok(_) if d.len() as u64 == seg.orig_size => Some(d),
                _ => None,
            }
        } else {
            None
        };
        match decoded {
            Some(d) => out.extend_from_slice(&d),
            None => out.extend_from_slice(&blob),
        }
    }
    Ok(out)
}

// ---------- 写入（未加密 XP3，GARbro / KiriKiri 可直接加载的布局） ----------

/// Adler-32（XP3 `adlr` 子块的值；实测未加密条目就是明文内容的 Adler-32）。
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &c in data {
        a = (a + c as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// XP3 头部变体（写侧选择）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Xp3Version {
    /// 旧版（KiriKiri 2.28 及以前 / GARbro v1）：magic 之后直接是 u64 索引偏移，数据区起点 19。
    V1,
    /// 现代版（KiriKiri 2.30+ / 吉里吉里Z，GARbro 默认）：带 `0x17` 扩展头，数据区起点 40。
    V2,
}

impl Xp3Version {
    /// 数据区起点（= 头部长度）。
    pub fn header_len(self) -> u64 {
        match self {
            Xp3Version::V1 => 19,
            Xp3Version::V2 => 40,
        }
    }
}

/// 从文件头判断头部变体（非 XP3 / 过短返回 None）。
pub fn version_of_bytes(head: &[u8]) -> Option<Xp3Version> {
    if head.len() < 19 || !head.starts_with(MAGIC) {
        return None;
    }
    Some(if head[11] == 0x17 { Xp3Version::V2 } else { Xp3Version::V1 })
}

/// 判断某封包用的是哪代头（只读前 19 字节）。
pub fn version_of(path: &Path) -> Option<Xp3Version> {
    use std::io::Read as _;
    let mut f = fs::File::open(path).ok()?;
    let mut head = [0u8; 19];
    f.read_exact(&mut head).ok()?;
    version_of_bytes(&head)
}

/// 索引里有多少条目的内容被引擎加密（KiriKiri 各家 Cx/Hx 方案，`info.flags` 的 bit31）。
///
/// STool 不内置这些方案（按游戏定制、需各自密钥），读出的是随机字节——
/// 调用方必须先看这个数，别把乱码当解包结果写盘。
pub fn encrypted_count(index: &BTreeMap<String, Xp3Entry>) -> usize {
    index.values().filter(|e| e.protected & 0x8000_0000 != 0).count()
}

/// 写出标准未加密 XP3（默认现代头 V2，逐文件缓冲，zlib 收益为正才压）。
/// 接收 相对路径 -> 源文件路径 映射；返回写入文件数。
pub fn write_paths(archive: &Path, files: &BTreeMap<String, std::path::PathBuf>) -> Result<usize, String> {
    write_paths_v(archive, files, Xp3Version::V2)
}

/// 按指定头部变体写出未加密 XP3，返回写入文件数。
///
/// 布局与 GARbro 一致（KiriKiri / 吉里吉里Z 可直接加载）：
/// - **头**：11 字节 magic；V2 再跟 `u64 0x17` + `u32 次版本 1` + `u8 0x80` + `u64 0`；
///   随后是待回填的 `u64 索引偏移`（V1 紧跟 magic）；数据区起点 V2=40 / V1=19。
/// - **数据区**：逐文件顺序写入（zlib 压缩收益为正才压，否则明文）。
/// - **索引区**：`[u8 0x01][u64 压缩后长][u64 原始长][zlib 索引]`，压不动时退化为
///   `[u8 0x00][u64 索引长][索引]`；索引体为 `File` 条目流，每项
///   `[u32 "File"][u64 内容长]` + `info`/`segm`/`adlr`（子块长度字段均为 u64）。
///
/// 两个易错点（都踩过）：`info.flags` **必须写 0**（bit31 表示“内容经引擎加密”，
/// 乱置会让引擎按密文解，读出乱码）；`segm` 的第一段字段是
/// `[u32 是否压缩][u64 段偏移][u64 原始大小][u64 归档大小]`，顺序不能换。
pub fn write_paths_v(archive: &Path, files: &BTreeMap<String, std::path::PathBuf>, version: Xp3Version) -> Result<usize, String> {
    use std::io::Write;
    let mut f = fs::File::create(archive).map_err(|e| e.to_string())?;
    let mut out = std::io::BufWriter::new(&mut f);
    out.write_all(MAGIC).map_err(|e| e.to_string())?;
    if version == Xp3Version::V2 {
        out.write_all(&0x17u64.to_le_bytes()).map_err(|e| e.to_string())?;
        out.write_all(&1u32.to_le_bytes()).map_err(|e| e.to_string())?; // 次版本号
        out.write_all(&[0x80]).map_err(|e| e.to_string())?;
        out.write_all(&0u64.to_le_bytes()).map_err(|e| e.to_string())?; // 保留字段
    }
    let index_slot = out.stream_position().map_err(|e| e.to_string())?;
    out.write_all(&0u64.to_le_bytes()).map_err(|e| e.to_string())?; // 索引偏移，末尾回填

    let mut toc: Vec<u8> = Vec::new();
    for (name, path) in files {
        let data = fs::read(path).map_err(|e| format!("{name}: {e}"))?;
        let seg_off = out.stream_position().map_err(|e| e.to_string())?;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
        z.write_all(&data).map_err(|e| e.to_string())?;
        let compressed = z.finish().map_err(|e| e.to_string())?;
        let (cid, blob): (u32, &[u8]) = if compressed.len() < data.len() { (1, &compressed) } else { (0, &data) };
        out.write_all(blob).map_err(|e| e.to_string())?;
        toc.extend_from_slice(&file_chunk(&name.replace('\\', "/"), seg_off, data.len(), blob.len(), cid, adler32(&data)));
    }

    let index_pos = out.stream_position().map_err(|e| e.to_string())?;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(9));
    z.write_all(&toc).map_err(|e| e.to_string())?;
    let ztoc = z.finish().map_err(|e| e.to_string())?;
    if ztoc.len() < toc.len() {
        out.write_all(&[0x01]).map_err(|e| e.to_string())?;
        out.write_all(&(ztoc.len() as u64).to_le_bytes()).map_err(|e| e.to_string())?;
        out.write_all(&(toc.len() as u64).to_le_bytes()).map_err(|e| e.to_string())?;
        out.write_all(&ztoc).map_err(|e| e.to_string())?;
    } else {
        out.write_all(&[0x00]).map_err(|e| e.to_string())?;
        out.write_all(&(toc.len() as u64).to_le_bytes()).map_err(|e| e.to_string())?;
        out.write_all(&toc).map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())?;
    drop(out);

    // 回填索引偏移（只改 8 字节，不重写整个文件）
    let mut f2 = fs::OpenOptions::new().write(true).open(archive).map_err(|e| e.to_string())?;
    f2.seek(std::io::SeekFrom::Start(index_slot)).map_err(|e| e.to_string())?;
    f2.write_all(&index_pos.to_le_bytes()).map_err(|e| e.to_string())?;
    Ok(files.len())
}

/// 组装一个 `File` 条目：`[u32 "File"][u64 内容长]` + `info` / `segm` / `adlr` 子块。
fn file_chunk(name: &str, offset: u64, orig: usize, packed: usize, cid: u32, adler: u32) -> Vec<u8> {
    let nb: Vec<u16> = name.encode_utf16().collect();

    let mut info = Vec::with_capacity(22 + nb.len() * 2);
    info.extend_from_slice(&0u32.to_le_bytes()); // flags：0 = 明文（bit31 = 引擎加密）
    info.extend_from_slice(&(orig as u64).to_le_bytes());
    info.extend_from_slice(&(packed as u64).to_le_bytes());
    info.extend_from_slice(&(nb.len() as u16).to_le_bytes());
    for u in &nb {
        info.extend_from_slice(&u.to_le_bytes());
    }

    let mut segm = Vec::with_capacity(28);
    segm.extend_from_slice(&cid.to_le_bytes());
    segm.extend_from_slice(&offset.to_le_bytes());
    segm.extend_from_slice(&(orig as u64).to_le_bytes());
    segm.extend_from_slice(&(packed as u64).to_le_bytes());

    let adlr = adler.to_le_bytes();
    let mut body = Vec::with_capacity(128 + nb.len() * 2);
    for (tag, b) in [
        (&b"info"[..], &info[..]),
        (&b"segm"[..], &segm[..]),
        (&b"adlr"[..], &adlr[..]),
    ] {
        body.extend_from_slice(tag);
        body.extend_from_slice(&(b.len() as u64).to_le_bytes());
        body.extend_from_slice(b);
    }

    let mut out = Vec::with_capacity(12 + body.len());
    out.extend_from_slice(b"File");
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// 内存版便捷封装（测试/小封包用）。
pub fn write(archive: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<usize, String> {
    let dir = archive.parent().unwrap_or_else(|| Path::new(".")).join(".xp3_tmp");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut paths = BTreeMap::new();
    for (name, data) in files {
        let p = dir.join(format!("{:016x}.bin", fxhash_str(name)));
        fs::write(&p, data).map_err(|e| e.to_string())?;
        paths.insert(name.clone(), p);
    }
    let r = write_paths(archive, &paths);
    let _ = fs::remove_dir_all(&dir);
    r
}

fn fxhash_str(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_xp3_{}_{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample() -> BTreeMap<String, Vec<u8>> {
        let mut m = BTreeMap::new();
        m.insert("scenario/first.ks".to_string(), "こんにちは。".repeat(200).into_bytes());
        // 不可压（随机化固定序列），应走明文存
        let noisy: Vec<u8> = (0..3000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        m.insert("image/bg01.bin".to_string(), noisy);
        m.insert("empty.txt".to_string(), Vec::new());
        m
    }

    #[test]
    fn v2_header_is_kirikiri_compatible() {
        let d = tmp("v2hdr");
        let arc = d.join("data.xp3");
        write(&arc, &sample()).unwrap();
        let data = fs::read(&arc).unwrap();
        assert!(data.starts_with(MAGIC));
        assert_eq!(data[11], 0x17, "V2 头必须带 0x17 标识（KiriKiri 2.30+/吉里吉里Z）");
        assert_eq!(&data[11..19], &0x17u64.to_le_bytes());
        assert_eq!(&data[19..23], &1u32.to_le_bytes());
        assert_eq!(data[23], 0x80);
        // 数据区起点必须正好是 40（索引偏移字段之后），且索引偏移指向文件内合法位置
        assert_eq!(first_seg_offset(&data), 40);
        let index_pos = u64::from_le_bytes(data[32..40].try_into().unwrap()) as usize;
        assert!(index_pos > 40 && index_pos < data.len(), "索引偏移 {index_pos} 越界");
        assert_eq!(version_of(&arc), Some(Xp3Version::V2));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn v1_header_is_legacy_layout() {
        let d = tmp("v1hdr");
        let arc = d.join("data.xp3");
        let mut paths = BTreeMap::new();
        for (k, v) in sample() {
            let p = d.join(format!("{}.bin", k.replace('/', "_")));
            fs::write(&p, v).unwrap();
            paths.insert(k, p);
        }
        write_paths_v(&arc, &paths, Xp3Version::V1).unwrap();
        let data = fs::read(&arc).unwrap();
        assert!(data.starts_with(MAGIC));
        assert_ne!(data[11], 0x17, "V1 没有 0x17 扩展头");
        // 数据区起点 = 19（magic + u64 索引偏移）
        assert_eq!(first_seg_offset(&data), 19);
        let index_pos = u64::from_le_bytes(data[11..19].try_into().unwrap()) as usize;
        assert!(index_pos > 19 && index_pos < data.len(), "索引偏移 {index_pos} 越界");
        assert_eq!(version_of(&arc), Some(Xp3Version::V1));
        let _ = fs::remove_dir_all(&d);
    }

    /// 取索引里第一个条目的段偏移（= 数据区起点，用来校验头部长度）。
    fn first_seg_offset(data: &[u8]) -> u64 {
        let index = parse_bytes(data).unwrap();
        let (_, e) = index.iter().next().expect("至少一个条目");
        e.segments.first().expect("至少一段").offset
    }

    #[test]
    fn roundtrip_both_versions_lossless_and_plain() {
        for ver in [Xp3Version::V2, Xp3Version::V1] {
            let d = tmp(&format!("rt{ver:?}"));
            let arc = d.join("data.xp3");
            let mut paths = BTreeMap::new();
            for (k, v) in sample() {
                let p = d.join(format!("{}.bin", k.replace('/', "_")));
                fs::write(&p, v).unwrap();
                paths.insert(k, p);
            }
            assert_eq!(write_paths_v(&arc, &paths, ver).unwrap(), 3);
            let index = parse(&arc).unwrap();
            assert_eq!(index.len(), 3);
            // 写出的条目必须是「明文」，否则引擎会按密文解出乱码
            assert_eq!(encrypted_count(&index), 0, "写出的条目不该带加密标志");
            for (name, want) in sample() {
                let e = index.get(&name).unwrap_or_else(|| panic!("{name} 缺失"));
                assert_eq!(read_file(&fs::read(&arc).unwrap(), e).unwrap(), want, "{name} 内容不符");
            }
            let _ = fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn stream_read_matches_memory_read() {
        let d = tmp("stream");
        let arc = d.join("data.xp3");
        write(&arc, &sample()).unwrap();
        let mut src = Source::open(&arc, u64::MAX).unwrap();
        let index = parse_index(&mut src).unwrap();
        assert_eq!(index.len(), 3);
        for (name, e) in &index {
            let got = read_entry(&mut src, e).unwrap();
            assert_eq!(got, sample()[name], "{name} 流式读取不一致");
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn adler_matches_known_value() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E60398);
        assert_eq!(adler32(b""), 1);
    }

    /// 逐字段核对索引区布局（不依赖外部工具），钉死 GARbro/KiriKiri 规范。
    /// 这是「游戏能不能加载」的关键：头部位置、子块长度字段宽度（u64 不是 u32）、
    /// `info.flags` 必须为 0、`segm` 字段顺序。
    #[test]
    fn toc_layout_matches_garbro_spec() {
        let d = tmp("layout");
        let arc = d.join("data.xp3");
        let mut files = BTreeMap::new();
        let payload: Vec<u8> = (0..64u32).map(|i| (i.wrapping_mul(2654435761) >> 11) as u8).collect();
        files.insert("dir/a.txt".to_string(), payload.clone());
        write(&arc, &files).unwrap();

        let data = fs::read(&arc).unwrap();
        let index_pos = u64::from_le_bytes(data[32..40].try_into().unwrap()) as usize;
        let toc_raw = &data[index_pos..];
        let compressed = toc_raw[0] == 0x01;
        let toc = decode_garbro_toc(toc_raw).expect("TOC 应可解开");
        if compressed {
            assert!(toc_raw.len() >= 17);
            let packed = u64::from_le_bytes(toc_raw[1..9].try_into().unwrap()) as usize;
            assert_eq!(u64::from_le_bytes(toc_raw[9..17].try_into().unwrap()) as usize, toc.len());
            assert_eq!(packed, toc_raw.len() - 17, "压缩后长度字段应等于 TOC 实际字节数");
        }

        let mut p = 0usize;
        assert_eq!(&toc[p..p + 4], b"File", "条目流必须以 File 标签开始");
        p += 4;
        let content_len = u64::from_le_bytes(toc[p..p + 8].try_into().unwrap()) as usize;
        p += 8;
        let entry_end = p + content_len;

        assert_eq!(&toc[p..p + 4], b"info");
        let info_len = u64::from_le_bytes(toc[p + 4..p + 12].try_into().unwrap()) as usize;
        p += 12;
        let info = &toc[p..p + info_len];
        assert_eq!(u32::from_le_bytes(info[0..4].try_into().unwrap()), 0, "info.flags 必须是 0（明文）");
        assert_eq!(u64::from_le_bytes(info[4..12].try_into().unwrap()) as usize, payload.len());
        assert_eq!(u64::from_le_bytes(info[12..20].try_into().unwrap()) as usize, payload.len());
        let nlen = u16::from_le_bytes(info[20..22].try_into().unwrap()) as usize;
        let name: String = info[22..22 + nlen * 2]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]) as u32)
            .map(|u| char::from_u32(u).unwrap_or('?'))
            .collect();
        assert_eq!(name, "dir/a.txt");
        p += info_len;

        assert_eq!(&toc[p..p + 4], b"segm");
        let segm_len = u64::from_le_bytes(toc[p + 4..p + 12].try_into().unwrap()) as usize;
        assert_eq!(segm_len, 28, "segm 子块长度字段必须是 u64 且值为 28");
        p += 12;
        let segm = &toc[p..p + segm_len];
        // [u32 是否压缩][u64 段偏移][u64 原始大小][u64 归档大小]
        assert_eq!(u32::from_le_bytes(segm[0..4].try_into().unwrap()), 0, "不可压内容应标为未压缩");
        assert_eq!(u64::from_le_bytes(segm[4..12].try_into().unwrap()), Xp3Version::V2.header_len());
        assert_eq!(u64::from_le_bytes(segm[12..20].try_into().unwrap()) as usize, payload.len());
        assert_eq!(u64::from_le_bytes(segm[20..28].try_into().unwrap()) as usize, payload.len());
        p += segm_len;

        assert_eq!(&toc[p..p + 4], b"adlr");
        assert_eq!(u64::from_le_bytes(toc[p + 4..p + 12].try_into().unwrap()), 4);
        let adler = u32::from_le_bytes(toc[p + 12..p + 16].try_into().unwrap());
        assert_eq!(adler, adler32(&payload));
        p += 4 + 12;
        assert_eq!(p, entry_end, "File 内容长度字段应与三个子块总长一致");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn version_of_rejects_non_xp3() {
        assert_eq!(version_of_bytes(b"not an xp3 at all, but 19 bytes"), None);
        assert_eq!(version_of_bytes(&[]), None);
    }
}
