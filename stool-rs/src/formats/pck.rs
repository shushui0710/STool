//! Godot PCK 封包（format v1 / v2）。

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

pub struct PckEntry {
    pub path: String,
    pub offset: u64,
    pub size: u64,
}

pub fn parse(path: &Path) -> Result<Vec<PckEntry>, String> {
    let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    f.read_to_end(&mut data).map_err(|e| e.to_string())?;
    parse_bytes(&data)
}

pub fn parse_bytes(data: &[u8]) -> Result<Vec<PckEntry>, String> {
    use crate::formats::safe as sf;
    if data.len() < 24 || !data.starts_with(b"GDPC") {
        return Err("不是 PCK 封包".into());
    }
    let fmt_ver = sf::u32_le(data, 4).ok_or("PCK 头部截断")?;
    // 头部布局：magic(4) + fmt_ver(4) + ver_major(4) + ver_minor(4) + 16×u32 保留(64)
    let mut pos = 20usize;
    let file_base: u64 = if fmt_ver == 2 {
        let flags = sf::u32_le(data, pos).ok_or("PCK v2 头部截断")?;
        let base = sf::u64_le(data, pos + 4).ok_or("PCK v2 头部截断")?;
        if flags & 2 != 0 {
            return Err("PCK 目录加密（Godot 加密包暂不支持）".into());
        }
        // flags(4) + file_base(8) + 16×u32 保留(64)
        pos = pos.checked_add(4 + 8 + 64).ok_or("PCK 头部越界")?;
        base
    } else {
        pos = pos.checked_add(64).ok_or("PCK 头部越界")?;
        0
    };
    let count = sf::u32_le(data, pos).ok_or("PCK 头部越界（文件计数缺失）")?;
    pos += 4;

    let mut out = Vec::new();
    for _ in 0..count {
        // 容错：任何一处截断都停止解析已读条目（而不是 panic），已解析部分仍然有效
        let plen = match sf::u32_le(data, pos) {
            Some(v) => v as usize,
            None => break,
        };
        pos += 4;
        let raw = match sf::slice(data, pos, plen) {
            Some(s) => s,
            None => break,
        };
        let path = String::from_utf8_lossy(raw).trim_end_matches('\0').to_string();
        pos += plen;
        let offset = match sf::u64_le(data, pos) {
            Some(v) => v,
            None => break,
        };
        let size = match sf::u64_le(data, pos + 8) {
            Some(v) => v,
            None => break,
        };
        pos += 16 + 16; // offset(8) + size(8) + md5(16)
        if fmt_ver == 2 {
            // v2 每条目附加 4 字节 flags（bit0 = 单文件加密）
            let eflags = match sf::u32_le(data, pos) {
                Some(v) => v,
                None => break,
            };
            pos += 4;
            if eflags & 1 != 0 {
                // 单文件加密：保留条目但标记为不可读（offset 置 0 尺寸 0）
                out.push(PckEntry { path, offset: 0, size: 0 });
                continue;
            }
        }
        out.push(PckEntry { path, offset: file_base.wrapping_add(offset), size });
    }
    Ok(out)
}

// ---------- 流式 API（P2-1）：只读索引 + 按需读条目 ----------

/// 只读索引（不整包读入）。条目截断即停止解析（已解析部分仍有效）。
pub fn parse_index(src: &mut super::source::Source) -> Result<Vec<PckEntry>, String> {
    let len = src.len();
    if len < 24 {
        return Err("不是 PCK 封包".into());
    }
    let head = src.read_exact_at(0, 20)?;
    if !head.starts_with(b"GDPC") {
        return Err("不是 PCK 封包".into());
    }
    let fmt_ver = u32::from_le_bytes(head[4..8].try_into().unwrap());
    // 头部布局：magic(4) + fmt_ver(4) + ver_major(4) + ver_minor(4) + 16×u32 保留(64)
    let mut pos = 20u64;
    let file_base: u64 = if fmt_ver == 2 {
        let h = src.read_exact_at(pos, 12)?;
        let flags = u32::from_le_bytes(h[0..4].try_into().unwrap());
        let base = u64::from_le_bytes(h[4..12].try_into().unwrap());
        if flags & 2 != 0 {
            return Err("PCK 目录加密（Godot 加密包暂不支持）".into());
        }
        pos += 4 + 8 + 64;
        base
    } else {
        pos += 64;
        0
    };
    let count = u32::from_le_bytes(src.read_exact_at(pos, 4)?.try_into().unwrap());
    pos += 4;

    let mut out = Vec::new();
    for _ in 0..count {
        // 容错：任何一处截断都停止解析已读条目（而不是 panic）
        let plen = match src.read_at(pos, 4) {
            Ok(v) if v.len() == 4 => u32::from_le_bytes(v.try_into().unwrap()) as usize,
            _ => break,
        };
        pos += 4;
        let raw = match src.read_at(pos, plen) {
            Ok(v) if v.len() == plen => v,
            _ => break,
        };
        let path = String::from_utf8_lossy(&raw).trim_end_matches('\0').to_string();
        pos += plen as u64;
        let meta = match src.read_at(pos, 32) {
            Ok(v) if v.len() == 32 => v,
            _ => break,
        };
        let offset = u64::from_le_bytes(meta[0..8].try_into().unwrap());
        let size = u64::from_le_bytes(meta[8..16].try_into().unwrap());
        pos += 32; // offset(8) + size(8) + md5(16)
        if fmt_ver == 2 {
            // v2 每条目附加 4 字节 flags（bit0 = 单文件加密）
            let eflags = match src.read_at(pos, 4) {
                Ok(v) if v.len() == 4 => u32::from_le_bytes(v.try_into().unwrap()),
                _ => break,
            };
            pos += 4;
            if eflags & 1 != 0 {
                out.push(PckEntry { path, offset: 0, size: 0 });
                continue;
            }
        }
        out.push(PckEntry { path, offset: file_base.wrapping_add(offset), size });
    }
    Ok(out)
}

/// 按需读取一个条目。
pub fn read_entry(src: &mut super::source::Source, e: &PckEntry) -> Result<Vec<u8>, String> {
    if e.size == 0 {
        return Ok(Vec::new());
    }
    src.read_at(e.offset, e.size as usize)
}

pub fn read_file(data: &[u8], e: &PckEntry) -> Vec<u8> {
    // 用饱和加法 + min 夹取，避免伪造 offset/size 触发整数溢出 panic
    let start = (e.offset as usize).min(data.len());
    let end = start.saturating_add(e.size as usize).min(data.len());
    data[start..end].to_vec()
}

/// 供测试：写出 v1 PCK。
pub fn write_v1(archive: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    use std::io::Write;
    let mut data: Vec<u8> = Vec::new();
    for c in files.values() {
        data.extend_from_slice(c);
    }
    let mut est = 0usize;
    for path in files.keys() {
        let n = path.len() + 1;
        // 4(plen) + 路径(含NUL+补齐) + 16(offset+size) + 16(md5)
        est += 4 + n + (4 - n % 4) % 4 + 16 + 16;
    }
    let base = (20 + 64 + 4 + est) as u64;
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"GDPC");
    out.extend_from_slice(&1u32.to_le_bytes()); // pack format v1
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 64]);
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    let mut off = 0u64;
    for (path, content) in files {
        // Godot 真实格式：path_len 字段 = 路径(含结尾 NUL)长度 + 补齐到 4 字节的 0
        let pb = path.as_bytes();
        let raw_len = pb.len() + 1; // 含结尾 \0
        let pad = (4 - raw_len % 4) % 4;
        out.extend_from_slice(&((raw_len + pad) as u32).to_le_bytes());
        out.extend_from_slice(pb);
        out.push(0u8);
        out.extend(std::iter::repeat_n(0u8, pad));
        out.extend_from_slice(&(base + off).to_le_bytes());
        out.extend_from_slice(&(content.len() as u64).to_le_bytes());
        out.extend_from_slice(&[0u8; 16]); // md5 占位
        off += content.len() as u64;
    }
    out.extend_from_slice(&data);
    let mut f = fs::File::create(archive).map_err(|e| e.to_string())?;
    f.write_all(&out).map_err(|e| e.to_string())?;
    Ok(())
}

/// 流式写出 v1 PCK（name -> 源文件路径），路径需带 `res://` 前缀。
/// 逐文件边读边写，内存占用恒定。返回写入文件数。
pub fn write_v1_paths(archive: &Path, files: &BTreeMap<String, std::path::PathBuf>) -> Result<usize, String> {
    use std::io::Write;
    // 预先收集大小，计算数据区基址
    let mut sizes: Vec<(String, std::path::PathBuf, u64)> = Vec::with_capacity(files.len());
    let mut est = 0usize;
    for (path_str, p) in files {
        let size = fs::metadata(p).map_err(|e| e.to_string())?.len();
        let n = path_str.len() + 1;
        est += 4 + n + (4 - n % 4) % 4 + 16 + 16;
        sizes.push((path_str.clone(), p.clone(), size));
    }
    let base = (20 + 64 + 4 + est) as u64;
    let mut f = fs::File::create(archive).map_err(|e| e.to_string())?;
    let mut out = std::io::BufWriter::new(&mut f);
    out.write_all(b"GDPC").map_err(|e| e.to_string())?;
    out.write_all(&1u32.to_le_bytes()).map_err(|e| e.to_string())?;
    out.write_all(&4u32.to_le_bytes()).map_err(|e| e.to_string())?;
    out.write_all(&0u32.to_le_bytes()).map_err(|e| e.to_string())?;
    out.write_all(&0u32.to_le_bytes()).map_err(|e| e.to_string())?;
    out.write_all(&[0u8; 64]).map_err(|e| e.to_string())?;
    out.write_all(&(sizes.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
    let mut off = 0u64;
    for (path_str, _, size) in &sizes {
        let pb = path_str.as_bytes();
        let raw_len = pb.len() + 1;
        let pad = (4 - raw_len % 4) % 4;
        out.write_all(&((raw_len + pad) as u32).to_le_bytes()).map_err(|e| e.to_string())?;
        out.write_all(pb).map_err(|e| e.to_string())?;
        out.write_all(&[0u8]).map_err(|e| e.to_string())?;
        out.write_all(&vec![0u8; pad]).map_err(|e| e.to_string())?;
        out.write_all(&(base + off).to_le_bytes()).map_err(|e| e.to_string())?;
        out.write_all(&size.to_le_bytes()).map_err(|e| e.to_string())?;
        out.write_all(&[0u8; 16]).map_err(|e| e.to_string())?;
        off += size;
    }
    for (_, path, _) in &sizes {
        let mut src = fs::File::open(path).map_err(|e| e.to_string())?;
        std::io::copy(&mut src, &mut out).map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())?;
    Ok(sizes.len())
}
