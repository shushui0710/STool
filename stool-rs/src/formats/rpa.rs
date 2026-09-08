//! Ren'Py RPA 封包（RPA-3.0 / 3.2 / 4.0，现代式前缀字节明文透传 + 旧式 int 前缀 XOR 兼容）。

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use flate2::read::ZlibDecoder;

use super::pickle::{self, Value};

pub struct RpaIndex {
    pub version: String,
    /// name -> [(prefix_bytes, data_offset, data_len)]
    pub entries: BTreeMap<String, Vec<(Vec<u8>, u64, u64)>>,
}

pub fn is_rpa(path: &Path) -> bool {
    fs::File::open(path)
        .and_then(|mut f| {
            let mut buf = [0u8; 8];
            f.read_exact(&mut buf)?;
            Ok(buf.starts_with(b"RPA-3.") || buf.starts_with(b"RPA-2.") || buf.starts_with(b"RPA-1."))
        })
        .unwrap_or(false)
}

pub fn read_index(archive: &Path) -> Result<RpaIndex, String> {
    let mut f = fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut header = Vec::new();
    let mut b = [0u8; 1];
    loop {
        f.read_exact(&mut b).map_err(|e| e.to_string())?;
        if b[0] == b'\n' || header.len() > 256 {
            break;
        }
        header.push(b[0]);
    }
    let line = String::from_utf8_lossy(&header);
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() || !parts[0].starts_with("RPA-") {
        return Err("不是 RPA 封包".into());
    }
    let version = parts[0].to_string();
    let (offset, key) = if parts.len() >= 3 {
        (
            u64::from_str_radix(parts[1], 16).map_err(|e| e.to_string())?,
            u32::from_str_radix(parts[2], 16).map_err(|e| e.to_string())?,
        )
    } else if parts.len() == 2 {
        (u64::from_str_radix(parts[1], 16).map_err(|e| e.to_string())?, 0)
    } else {
        return Err("RPA 头部格式错误".into());
    };
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    f.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let z = ZlibDecoder::new(&raw[..]);
    let mut index_bytes = Vec::new();
    z.take(512 * 1024 * 1024)
        .read_to_end(&mut index_bytes)
        .map_err(|e| e.to_string())?;
    let value = pickle::loads(&index_bytes)?;
    let dict = value
        .as_dict()
        .ok_or("RPA 索引不是字典")?
        .clone();

    let mut entries = BTreeMap::new();
    for (k, v) in dict {
        let name = String::from_utf8_lossy(k.as_bytes().unwrap_or(b"")).into_owned();
        let mut chunks = Vec::new();
        if let Some(list) = v.as_list() {
            for chunk in list {
                let parts = chunk.as_list().ok_or("索引块不是元组")?;
                match parts.len() {
                    2 => {
                        let off = parts[0].as_int().unwrap_or(0) as u64 ^ key as u64;
                        let dlen = parts[1].as_int().unwrap_or(0) as u64 ^ key as u64;
                        chunks.push((Vec::new(), off, dlen));
                    }
                    3 => {
                        let off = parts[0].as_int().unwrap_or(0) as u64 ^ key as u64;
                        let dlen = parts[1].as_int().unwrap_or(0) as u64 ^ key as u64;
                        match &parts[2] {
                            Value::Int(prefix) => {
                                // 旧式：前 N 字节逐字节 XOR key 低 8 位
                                let n = (*prefix).max(0) as u64;
                                let data = read_at(&mut f, off, dlen)?;
                                let mut head = data[..(n as usize).min(data.len())].to_vec();
                                for b in head.iter_mut() {
                                    *b ^= (key & 0xFF) as u8;
                                }
                                chunks.push((head, off + n, dlen.saturating_sub(n)));
                            }
                            other => {
                                let bytes = other.as_bytes().unwrap_or(&[]).to_vec();
                                chunks.push((bytes, off, dlen));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        entries.insert(name, chunks);
    }
    Ok(RpaIndex { version, entries })
}

fn read_at(f: &mut fs::File, off: u64, len: u64) -> Result<Vec<u8>, String> {
    f.seek(SeekFrom::Start(off)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; len as usize];
    f.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

/// 读取一个文件的全部数据（前缀字节 + 各数据块）。
pub fn read_file(archive: &Path, chunks: &[(Vec<u8>, u64, u64)]) -> Result<Vec<u8>, String> {
    let mut f = fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (prefix, off, len) in chunks {
        out.extend_from_slice(prefix);
        out.extend_from_slice(&read_at(&mut f, *off, *len)?);
    }
    Ok(out)
}

/// 从内存文件字典写 RPA-3.0 封包（现代式）。数据区紧跟 34 字节头。
pub fn write_archive(archive: &Path, files: &BTreeMap<String, Vec<u8>>, key: u32) -> Result<(), String> {
    const HEADER_LEN: u64 = 8 + 16 + 1 + 8 + 1; // "RPA-3.0 " + offset + " " + key + "\n"
    let mut index: BTreeMap<Vec<u8>, Vec<(u64, u64)>> = BTreeMap::new();
    let mut body: Vec<u8> = Vec::new();
    for (name, data) in files {
        let off = HEADER_LEN + body.len() as u64; // 文件内绝对偏移
        body.extend_from_slice(data);
        index.insert(name.as_bytes().to_vec(), vec![(off ^ key as u64, data.len() as u64 ^ key as u64)]);
    }
    let index_bytes = pickle_dumps(&index);
    let compressed = {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&index_bytes).map_err(|e| e.to_string())?;
        enc.finish().map_err(|e| e.to_string())?
    };
    let index_offset = body.len() as u64 + HEADER_LEN;
    let mut f = fs::File::create(archive).map_err(|e| e.to_string())?;
    write!(f, "RPA-3.0 {index_offset:016x} {key:08x}\n").map_err(|e| e.to_string())?;
    f.write_all(&body).map_err(|e| e.to_string())?;
    f.write_all(&compressed).map_err(|e| e.to_string())?;
    Ok(())
}

/// 把 pickle::Value 序列化回协议 2 字节流（支持索引所需子集）。
/// 偏移/长度用 LONG1 编码（避免 BININT 的 i32 截断与符号扩展）。
fn pickle_dumps(map: &BTreeMap<Vec<u8>, Vec<(u64, u64)>>) -> Vec<u8> {
    let mut out = vec![0x80u8, 2, b'}'];
    for (k, chunks) in map {
        out.push(b'X');
        out.extend_from_slice(&(k.len() as u32).to_le_bytes());
        out.extend_from_slice(k);
        out.push(b']');
        for (a, b) in chunks {
            push_long1(&mut out, *a);
            push_long1(&mut out, *b);
            out.push(0x86); // TUPLE2
            out.push(b'a'); // APPEND
        }
        out.push(b's'); // SETITEM
    }
    out.push(b'.');
    out
}

fn push_long1(out: &mut Vec<u8>, v: u64) {
    out.push(0x8a); // LONG1
    let bytes = v.to_le_bytes();
    let mut n = 8usize;
    while n > 0 && bytes[n - 1] == 0 {
        n -= 1;
    }
    if n == 0 {
        n = 1; // 0 也占 1 字节
    }
    out.push(n as u8);
    out.extend_from_slice(&bytes[..n]);
}
