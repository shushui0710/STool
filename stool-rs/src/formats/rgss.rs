//! RPG Maker XP/VX/Ace 的 RGSSAD 封包。
//!
//! 算法对照 GARbro (Experimental/RPGMaker/ArcRGSS.cs) 与 RPGMakerDecrypter：
//! - v1 (.rgssad/.rgss2a)：条目头与数据交错存放；密钥流从 0xDEADCAFE 起按
//!   `k = k*7+3` 推进，但**只覆盖元数据**（名字长度/名字/数据长度），不穿过数据区；
//!   每个文件记录其数据区起始密钥，数据按 4 字节块解密（块首推进一次，
//!   块内第 i 字节用 `(key >> (8*(i%4))) & 0xFF`）。
//! - v3 (.rgss3a)：8 字节头后是 4 字节元数据密钥种子，`key = seed*9+3`；
//!   所有条目头**连续**存放（offset/size/file_key/name_len 各 XOR key，offset==0 结束）；
//!   名字用 `key` 循环 4 字节异或；数据用 entry 的 file_key 按块解密（同 v1 方式）。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const BASE_KEY: u32 = 0xDEADCAFE;

struct KeyStream(u32);
impl KeyStream {
    fn new() -> Self {
        KeyStream(BASE_KEY)
    }
    /// 返回当前密钥并推进（k = k*7+3）
    fn next(&mut self) -> u32 {
        let k = self.0;
        self.0 = self.0.wrapping_mul(7).wrapping_add(3);
        k
    }
    fn current(&self) -> u32 {
        self.0
    }
}

/// 块解密/加密通用：首块用 key 本身，之后每块推进一次，
/// 块内第 i 字节用 `(key >> (8*(i%4))) & 0xFF`。
fn xor_block_stream(data: &[u8], start: usize, mut key: u32) -> Vec<u8> {
    let mut out = vec![0u8; data.len().saturating_sub(start)];
    for (i, o) in out.iter_mut().enumerate() {
        if i > 0 && i % 4 == 0 {
            key = key.wrapping_mul(7).wrapping_add(3);
        }
        *o = data[start + i] ^ ((key >> ((i % 4) * 8)) & 0xFF) as u8;
    }
    out
}

/// v1 (.rgssad/.rgss2a)：name -> (offset, size, data_key)
pub fn parse_v1(path: &Path) -> Result<BTreeMap<String, (u64, u32, u32)>, String> {
    let data = fs::read(path).map_err(|e| e.to_string())?;
    if !data.starts_with(b"RGSSAD\x00") {
        return Err("不是 RGSSAD 封包".into());
    }
    let mut pos = 8usize;
    let mut ks = KeyStream::new();
    let mut out = BTreeMap::new();
    while pos + 4 <= data.len() {
        let name_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) ^ ks.next();
        pos += 4;
        if name_len == 0 || name_len > 512 || pos + name_len as usize + 4 > data.len() {
            break;
        }
        let mut name = Vec::with_capacity(name_len as usize);
        for i in 0..name_len as usize {
            name.push(data[pos + i] ^ (ks.next() & 0xFF) as u8);
        }
        pos += name_len as usize;
        let size = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) ^ ks.next();
        pos += 4;
        let data_key = ks.current(); // 数据区起始密钥（元数据流不穿过数据区）
        out.insert(
            String::from_utf8_lossy(&name).into_owned(),
            (pos as u64, size, data_key),
        );
        pos = pos.saturating_add(size as usize);
    }
    Ok(out)
}

/// v1 数据解密。
pub fn extract_v1_file(data: &[u8], offset: u64, size: u32, data_key: u32) -> Vec<u8> {
    // 饱和加法 + min 夹取，避免伪造 offset/size 触发整数溢出 panic
    let start = (offset as usize).min(data.len());
    let end = start.saturating_add(size as usize).min(data.len());
    xor_block_stream(&data[..end], start, data_key)
}

pub struct V3Entry {
    pub name: String,
    pub offset: u64,
    pub size: u64,
    pub filekey: u32,
}

/// v3 (.rgss3a)。
pub fn parse_v3(path: &Path) -> Result<Vec<V3Entry>, String> {
    let data = fs::read(path).map_err(|e| e.to_string())?;
    if !data.starts_with(b"RGSSAD\x00\x03") {
        return Err("不是 RGSS3A 封包".into());
    }
    if data.len() < 12 {
        return Err("RGSS3A 头部过短".into());
    }
    let seed = u32::from_le_bytes(data[8..12].try_into().unwrap());
    let key = seed.wrapping_mul(9).wrapping_add(3);
    let mut pos = 12usize;
    let mut out = Vec::new();
    while pos + 16 <= data.len() {
        let r = |p: &mut usize| -> u32 {
            let v = u32::from_le_bytes(data[*p..*p + 4].try_into().unwrap()) ^ key;
            *p += 4;
            v
        };
        let offset = r(&mut pos) as u64;
        if offset == 0 {
            break;
        }
        let size = r(&mut pos) as u64;
        let filekey = r(&mut pos);
        let name_len = r(&mut pos);
        if name_len == 0 || name_len > 512 || pos + name_len as usize > data.len() {
            break;
        }
        let mut name = Vec::with_capacity(name_len as usize);
        for i in 0..name_len as usize {
            name.push(data[pos + i] ^ ((key >> ((i % 4) * 8)) & 0xFF) as u8);
        }
        pos += name_len as usize;
        out.push(V3Entry {
            name: String::from_utf8_lossy(&name).into_owned(),
            offset,
            size,
            filekey,
        });
    }
    Ok(out)
}

/// v3 数据解密。
pub fn extract_v3_file(data: &[u8], offset: u64, size: u64, filekey: u32) -> Vec<u8> {
    // 饱和加法 + min 夹取，避免伪造 offset/size 触发整数溢出 panic
    let start = (offset as usize).min(data.len());
    let end = start.saturating_add(size as usize).min(data.len());
    xor_block_stream(&data[..end], start, filekey)
}

/// 供测试/封包使用：v1 加密写出（与真实格式一致）。
pub fn write_v1(archive: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    use std::io::Write;
    let mut out: Vec<u8> = b"RGSSAD\x00\x01".to_vec();
    let mut ks = KeyStream::new();
    for (name, data) in files {
        let nb = name.as_bytes();
        out.extend_from_slice(&(nb.len() as u32 ^ ks.next()).to_le_bytes());
        for b in nb {
            out.push(b ^ (ks.next() & 0xFF) as u8);
        }
        out.extend_from_slice(&((data.len() as u32) ^ ks.next()).to_le_bytes());
        // 数据加密：用当前流状态作为本文件数据密钥，块内推进但不影响元数据流
        let data_key = ks.current();
        let mut k = data_key;
        for (i, b) in data.iter().enumerate() {
            if i > 0 && i % 4 == 0 {
                k = k.wrapping_mul(7).wrapping_add(3);
            }
            out.push(b ^ ((k >> ((i % 4) * 8)) & 0xFF) as u8);
        }
    }
    let mut f = fs::File::create(archive).map_err(|e| e.to_string())?;
    f.write_all(&out).map_err(|e| e.to_string())?;
    Ok(())
}

/// 供测试/封包使用：v3 加密写出（条目头连续 + 数据区，与真实格式一致）。
pub fn write_v3(archive: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    use std::io::Write;
    // 元数据密钥：任选种子，key = seed*9+3
    let seed: u32 = 0x1234_5678;
    let key = seed.wrapping_mul(9).wrapping_add(3);
    // 计算头区总长：12(头+种子) + Σ(4×4 字段 + 名字) + 4(offset=0 终止符)
    let mut header_len = 16usize;
    for name in files.keys() {
        header_len += 16 + name.len();
    }
    let mut out: Vec<u8> = b"RGSSAD\x00\x03".to_vec();
    out.extend_from_slice(&seed.to_le_bytes());
    let mut data_off = header_len as u64;
    for (name, size) in files.iter().map(|(n, d)| (n, d.len() as u64)) {
        let nb = name.as_bytes();
        let filekey = (size as u32).wrapping_mul(9).wrapping_add(3);
        out.extend_from_slice(&((data_off as u32) ^ key).to_le_bytes());
        out.extend_from_slice(&((size as u32) ^ key).to_le_bytes());
        out.extend_from_slice(&(filekey ^ key).to_le_bytes());
        out.extend_from_slice(&((nb.len() as u32) ^ key).to_le_bytes());
        for (i, b) in nb.iter().enumerate() {
            out.push(b ^ ((key >> ((i % 4) * 8)) & 0xFF) as u8);
        }
        data_off += size;
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // 终止条目 offset=0
    for (name, data) in files {
        let filekey = (data.len() as u32).wrapping_mul(9).wrapping_add(3);
        out.extend_from_slice(&xor_block_stream(data, 0, filekey));
        let _ = name;
    }
    let mut f = fs::File::create(archive).map_err(|e| e.to_string())?;
    f.write_all(&out).map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- 流式写出（路径版）：避免大封包整体载入内存 ----------

// ---------- 流式 API（P2-1）：只读索引 + 按需读条目 ----------

/// 只读 v1 索引（不整包读入）。v1 的条目头与数据**交错**存放，因此按
/// 「读元数据 → 跳过数据区」的方式顺序推进。
pub fn parse_index_v1(src: &mut super::source::Source) -> Result<BTreeMap<String, (u64, u32, u32)>, String> {
    let len = src.len();
    if len < 8 {
        return Err("不是 RGSSAD 封包".into());
    }
    if !src.read_exact_at(0, 8)?.starts_with(b"RGSSAD\x00") {
        return Err("不是 RGSSAD 封包".into());
    }
    let mut pos = 8u64;
    let mut ks = KeyStream::new();
    let mut out = BTreeMap::new();
    while pos + 4 <= len {
        let name_len = match src.read_at(pos, 4) {
            Ok(v) if v.len() == 4 => u32::from_le_bytes(v.try_into().unwrap()) ^ ks.next(),
            _ => break,
        };
        pos += 4;
        if name_len == 0 || name_len > 512 || pos + name_len as u64 + 4 > len {
            break;
        }
        let nb = match src.read_at(pos, name_len as usize) {
            Ok(v) if v.len() == name_len as usize => v,
            _ => break,
        };
        let mut name = Vec::with_capacity(name_len as usize);
        for b in nb {
            name.push(b ^ (ks.next() & 0xFF) as u8);
        }
        pos += name_len as u64;
        let size = match src.read_at(pos, 4) {
            Ok(v) if v.len() == 4 => u32::from_le_bytes(v.try_into().unwrap()) ^ ks.next(),
            _ => break,
        };
        pos += 4;
        let data_key = ks.current(); // 数据区起始密钥（元数据流不穿过数据区）
        out.insert(String::from_utf8_lossy(&name).into_owned(), (pos, size, data_key));
        pos = pos.saturating_add(size as u64);
    }
    Ok(out)
}

/// 只读 v3 索引（不整包读入）：头表连续存放，按需逐条读。
pub fn parse_index_v3(src: &mut super::source::Source) -> Result<Vec<V3Entry>, String> {
    let len = src.len();
    if len < 12 {
        return Err("不是 RGSS3A 封包".into());
    }
    let head = src.read_exact_at(0, 12)?;
    if !head.starts_with(b"RGSSAD\x00\x03") {
        return Err("不是 RGSS3A 封包".into());
    }
    let seed = u32::from_le_bytes(head[8..12].try_into().unwrap());
    let key = seed.wrapping_mul(9).wrapping_add(3);
    let mut pos = 12u64;
    let mut out = Vec::new();
    while pos + 16 <= len {
        let rec = match src.read_at(pos, 16) {
            Ok(v) if v.len() == 16 => v,
            _ => break,
        };
        let offset = (u32::from_le_bytes(rec[0..4].try_into().unwrap()) ^ key) as u64;
        if offset == 0 {
            break;
        }
        let size = (u32::from_le_bytes(rec[4..8].try_into().unwrap()) ^ key) as u64;
        let filekey = u32::from_le_bytes(rec[8..12].try_into().unwrap()) ^ key;
        let name_len = u32::from_le_bytes(rec[12..16].try_into().unwrap()) ^ key;
        pos += 16;
        if name_len == 0 || name_len > 512 || pos + name_len as u64 > len {
            break;
        }
        let nb = match src.read_at(pos, name_len as usize) {
            Ok(v) if v.len() == name_len as usize => v,
            _ => break,
        };
        let mut name = Vec::with_capacity(name_len as usize);
        for (i, b) in nb.iter().enumerate() {
            name.push(b ^ ((key >> ((i % 4) * 8)) & 0xFF) as u8);
        }
        pos += name_len as u64;
        out.push(V3Entry {
            name: String::from_utf8_lossy(&name).into_owned(),
            offset,
            size,
            filekey,
        });
    }
    Ok(out)
}

/// 按需读取 v1 条目数据（读多少解密多少）。
pub fn read_entry_v1(
    src: &mut super::source::Source,
    offset: u64,
    size: u32,
    data_key: u32,
) -> Result<Vec<u8>, String> {
    let buf = src.read_at(offset, size as usize)?;
    Ok(xor_block_stream(&buf, 0, data_key))
}

/// 按需读取 v3 条目数据。
pub fn read_entry_v3(
    src: &mut super::source::Source,
    offset: u64,
    size: u64,
    filekey: u32,
) -> Result<Vec<u8>, String> {
    let buf = src.read_at(offset, size as usize)?;
    Ok(xor_block_stream(&buf, 0, filekey))
}

/// 数据块 XOR 加密写入器：密钥推进规则与 xor_block_stream 一致（块首推进）。
struct XorBlockWriter<W: std::io::Write> {
    inner: W,
    key: u32,
    i: usize,
}

impl<W: std::io::Write> XorBlockWriter<W> {
    fn new(inner: W, key: u32) -> Self {
        XorBlockWriter { inner, key, i: 0 }
    }
}

impl<W: std::io::Write> std::io::Write for XorBlockWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut enc = Vec::with_capacity(buf.len());
        for &b in buf {
            if self.i > 0 && self.i.is_multiple_of(4) {
                self.key = self.key.wrapping_mul(7).wrapping_add(3);
            }
            enc.push(b ^ ((self.key >> ((self.i % 4) * 8)) & 0xFF) as u8);
            self.i += 1;
        }
        self.inner.write_all(&enc)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// RGSSAD 名字里的路径分隔符统一为反斜杠（与原版工具一致）。
fn rgss_name(rel: &str) -> String {
    rel.replace('/', "\\")
}

/// 流式 v1 写出：name -> 源文件路径。逐文件边读边加密，内存占用恒定。
pub fn write_v1_paths(archive: &Path, files: &BTreeMap<String, std::path::PathBuf>) -> Result<u64, String> {
    use std::io::Write;
    let mut f = fs::File::create(archive).map_err(|e| e.to_string())?;
    let mut out = std::io::BufWriter::new(&mut f);
    out.write_all(b"RGSSAD\x00\x01").map_err(|e| e.to_string())?;
    let mut ks = KeyStream::new();
    for (name, path) in files {
        let nb = rgss_name(name).as_bytes().to_vec();
        out.write_all(&(nb.len() as u32 ^ ks.next()).to_le_bytes()).map_err(|e| e.to_string())?;
        for b in &nb {
            out.write_all(&[b ^ (ks.next() & 0xFF) as u8]).map_err(|e| e.to_string())?;
        }
        let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
        if size > u32::MAX as u64 {
            return Err(format!("文件过大（{}），RGSSAD v1 不支持", size));
        }
        out.write_all(&((size as u32) ^ ks.next()).to_le_bytes()).map_err(|e| e.to_string())?;
        let data_key = ks.current();
        let src = fs::File::open(path).map_err(|e| e.to_string())?;
        let mut xw = XorBlockWriter::new(&mut out, data_key);
        std::io::copy(&mut &src, &mut xw).map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())?;
    Ok(0)
}

/// 流式 v3 写出：name -> 源文件路径。
pub fn write_v3_paths(archive: &Path, files: &BTreeMap<String, std::path::PathBuf>) -> Result<u64, String> {
    use std::io::Write;
    let seed: u32 = 0x1234_5678;
    let key = seed.wrapping_mul(9).wrapping_add(3);
    let mut header_len = 16usize;
    for name in files.keys() {
        header_len += 16 + rgss_name(name).len();
    }
    // 先收集大小
    let mut sizes: Vec<(String, std::path::PathBuf, u64)> = Vec::with_capacity(files.len());
    for (name, path) in files {
        let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
        if size > u32::MAX as u64 {
            return Err(format!("文件过大（{}），RGSSAD v3 偏移为 u32，不支持", size));
        }
        sizes.push((name.clone(), path.clone(), size));
    }
    let mut f = fs::File::create(archive).map_err(|e| e.to_string())?;
    let mut out = std::io::BufWriter::new(&mut f);
    out.write_all(b"RGSSAD\x00\x03").map_err(|e| e.to_string())?;
    out.write_all(&seed.to_le_bytes()).map_err(|e| e.to_string())?;
    let mut off = header_len as u64;
    for (name, _, size) in &sizes {
        let nb = rgss_name(name).into_bytes();
        let filekey = (*size as u32).wrapping_mul(9).wrapping_add(3);
        out.write_all(&((off as u32) ^ key).to_le_bytes()).map_err(|e| e.to_string())?;
        out.write_all(&((*size as u32) ^ key).to_le_bytes()).map_err(|e| e.to_string())?;
        out.write_all(&(filekey ^ key).to_le_bytes()).map_err(|e| e.to_string())?;
        out.write_all(&((nb.len() as u32) ^ key).to_le_bytes()).map_err(|e| e.to_string())?;
        for (i, b) in nb.iter().enumerate() {
            out.write_all(&[b ^ ((key >> ((i % 4) * 8)) & 0xFF) as u8]).map_err(|e| e.to_string())?;
        }
        off += size;
    }
    out.write_all(&0u32.to_le_bytes()).map_err(|e| e.to_string())?;
    for (_, path, size) in &sizes {
        let filekey = (*size as u32).wrapping_mul(9).wrapping_add(3);
        let src = fs::File::open(path).map_err(|e| e.to_string())?;
        let mut xw = XorBlockWriter::new(&mut out, filekey);
        std::io::copy(&mut &src, &mut xw).map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())?;
    Ok(0)
}
