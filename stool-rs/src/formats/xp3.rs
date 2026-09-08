//! KiriKiri XP3 封包（标准未加密变体：zlib TOC + zlib/明文数据段；
//! 另支持 flag=0x17 的「头部伪装」变体，见 parse_disguised）。

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::io::Seek;
use std::path::Path;

pub const MAGIC: &[u8] = b"XP3\r\n \n\x1a\x8b\x67\x01";

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
    if !data.starts_with(MAGIC) {
        return Err("不是 XP3 封包".into());
    }
    let flag = data[11];
    if flag == 0x17 {
        return parse_disguised(data);
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
        u64::from_le_bytes(data[12..20].try_into().unwrap())
    } else {
        u32::from_le_bytes(data[12..16].try_into().unwrap()) as u64
    };
    if index_pos as usize >= data.len() {
        return Err("XP3 索引偏移越界（可能为加密变体）".into());
    }
    let raw = &data[index_pos as usize..];
    let toc: Vec<u8> = match raw.first() {
        Some(0x01) | Some(0x80) => {
            let mut z = flate2::read::ZlibDecoder::new(&raw[1..]);
            let mut out = Vec::new();
            z.read_to_end(&mut out).map_err(|e| e.to_string())?;
            out
        }
        _ => raw[1..].to_vec(),
    };
    if toc.len() < 4 {
        return Err("XP3 TOC 过短".into());
    }
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
    Ok(files)
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
    let raw = &data[index_pos..];
    if raw.is_empty() {
        return Err("XP3（伪装头）TOC 为空".into());
    }
    let buf: Vec<u8> = match raw[0] {
        0x00 if raw.len() >= 9 => raw[9..].to_vec(),
        0x01 if raw.len() >= 17 => {
            let zlen = u64::from_le_bytes(raw[1..9].try_into().unwrap()) as usize;
            let zend = (17 + zlen).min(raw.len());
            let mut z = flate2::read::ZlibDecoder::new(&raw[17..zend]);
            let mut out = Vec::new();
            z.read_to_end(&mut out).map_err(|e| e.to_string())?;
            if out.starts_with(b"Hxv4") {
                return Err(
                    "Hxv4 保护变体：目录可读但文件名为合成字符且数据段加密，暂不支持提取".into(),
                );
            }
            out
        }
        _ => return Err("XP3（伪装头）TOC 头部不识别".into()),
    };
    // 条目流起点：裸块从 0 开始；zlib 解压后跳过可能存在的私有头（找到首个 File 标签）
    let start = if raw[0] == 0x01 {
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
/// 定位方式：在尾部窗口找最小的 File 标签位置 s，使 u64@(s-8) == 文件大小 - s，
/// 且从 s 起的条目链恰好走到文件尾。
fn parse_tail_toc(data: &[u8]) -> Option<BTreeMap<String, Xp3Entry>> {
    let sz = data.len();
    if sz < 1024 {
        return None;
    }
    let win = (8 * 1024 * 1024usize).min(sz);
    let scan_start = sz - win;
    let mut i = scan_start;
    while i + 4 <= sz {
        let p = match data[i..sz].windows(4).position(|w| w == b"File") {
            Some(off) => i + off,
            None => return None,
        };
        if p >= 8 {
            let total = u64::from_le_bytes(data[p - 8..p].try_into().unwrap()) as usize;
            if total == sz - p {
                if let Ok(files) = walk_variant_entries(&data[p..]) {
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
        let end = (start + seg.archive_size as usize).min(data.len());
        if start > data.len() {
            return Err("数据段越界（可能为加密变体）".into());
        }
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

// ---------- 写入（标准未加密 XP3） ----------

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &c in data {
        a = (a + c as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(tag: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// 写出标准未加密 XP3（逐文件缓冲，zlib 压缩收益为负时按明文存）。
/// 接收 相对路径 -> 源文件路径 映射；返回写入文件数。TOC 用 zlib 压缩。
pub fn write_paths(archive: &Path, files: &BTreeMap<String, std::path::PathBuf>) -> Result<usize, String> {
    use std::io::Write;
    let mut f = fs::File::create(archive).map_err(|e| e.to_string())?;
    let mut out = std::io::BufWriter::new(&mut f);
    out.write_all(MAGIC).map_err(|e| e.to_string())?;
    out.write_all(&[0x00]).map_err(|e| e.to_string())?; // flag：u32 索引偏移
    out.write_all(&0u32.to_le_bytes()).map_err(|e| e.to_string())?; // 占位，稍后回填

    let mut toc: Vec<u8> = Vec::new();
    toc.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (name, path) in files {
        let data = fs::read(path).map_err(|e| format!("{name}: {e}"))?;
        // 数据块：尝试 zlib，压不动就明文
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
        z.write_all(&data).map_err(|e| e.to_string())?;
        let compressed = z.finish().map_err(|e| e.to_string())?;
        let clen = compressed.len();
        let (blob, orig, arch) = if clen < data.len() {
            (compressed, data.len() as u64, clen as u64)
        } else {
            (data.clone(), data.len() as u64, data.len() as u64)
        };
        let seg_off = out.stream_position().map_err(|e| e.to_string())?;
        out.write_all(&blob).map_err(|e| e.to_string())?;

        let nb: Vec<u16> = name.encode_utf16().collect();
        let mut info = Vec::with_capacity(22 + nb.len() * 2);
        info.extend_from_slice(&1u32.to_le_bytes()); // protected
        info.extend_from_slice(&0u64.to_le_bytes()); // time1
        info.extend_from_slice(&0u64.to_le_bytes()); // time2
        info.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        for u in &nb {
            info.extend_from_slice(&u.to_le_bytes());
        }
        let mut segm = Vec::with_capacity(28);
        segm.extend_from_slice(&1u32.to_le_bytes());
        segm.extend_from_slice(&seg_off.to_le_bytes());
        segm.extend_from_slice(&orig.to_le_bytes());
        segm.extend_from_slice(&arch.to_le_bytes());
        let adlr = adler32(&data).to_le_bytes().to_vec();

        let mut entry_body = chunk(b"info", &info);
        entry_body.extend_from_slice(&chunk(b"segm", &segm));
        entry_body.extend_from_slice(&chunk(b"adlr", &adlr));
        // 每个文件条目 = [u32 长度][info+segm+adlr 子块]
        toc.extend_from_slice(&(entry_body.len() as u32).to_le_bytes());
        toc.extend_from_slice(&entry_body);
    }
    let toc_pos = out.stream_position().map_err(|e| e.to_string())?;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    z.write_all(&toc).map_err(|e| e.to_string())?;
    let ztoc = z.finish().map_err(|e| e.to_string())?;
    let toc_raw = if ztoc.len() < toc.len() {
        let mut v = vec![0x01u8];
        v.extend_from_slice(&ztoc);
        v
    } else {
        let mut v = vec![0x00u8];
        v.extend_from_slice(&toc);
        v
    };
    out.write_all(&toc_raw).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    // 回填索引偏移
    let mut f2 = fs::OpenOptions::new().write(true).open(archive).map_err(|e| e.to_string())?;
    f2.seek(std::io::SeekFrom::Start(12)).map_err(|e| e.to_string())?;
    f2.write_all(&(toc_pos as u32).to_le_bytes()).map_err(|e| e.to_string())?;
    Ok(files.len())
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
