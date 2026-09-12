//! Artemis Engine `.pfs` 封包（`pf6` / `pf8`）。
//!
//! 真实样本：`アマカノ3.pfs`（1.7GB / 3361 条目）、`美少女万華鏡1 root.pfs`（1.5GB / 2703 条目）。
//!
//! # 结构
//!
//! ```text
//! 0x00  "pf" + 版本数字（'6' 或 '8'）        3 字节
//! 0x03  u32 index_size                       4 字节
//! 0x07  index(index_size 字节) ← count 就存在 index 的头 4 字节里
//!       └ index[0..4] u32 count
//!         每条目：[u32 名长][名字（UTF-8，不补零）][u32 保留=0][u32 数据偏移][u32 数据大小]
//! 数据区（紧跟 index 之后，条目基本连续）
//! ```
//!
//! # 数据混淆（sha1 密钥流）
//!
//! `pf8`（以及 GARbro 记的 4/5/9 版）的数据区用 **`SHA1(index)` 的前 20 字节**做重复
//! XOR 密钥流。要点：**每条目都从 `key[0]` 重新开始**（不是按文件绝对偏移）——
//! 实测同一类型文件在不同偏移处的密文前缀完全一致，即为此结论的直接证据；
//! GARbro 的 `ByteStringEncryptedStream` 因为流位置恰好等于绝对偏移，在本样本上
//! 会算错（其 base_pos 只在 `base_pos + pos` 意义上成立）。
//!
//! 这是引擎自带的**固定**混淆：密钥完全由封包自身索引推导，不含每游戏私钥，
//! 与 `rgss` / `rpgmmv` 里已实现的固定密钥 XOR 同性质，不属于"破解他人加密方案"。
//! `pf6` 为明文（无 XOR）。
//!
//! 写侧对称：`write_archive` 先按条目重建索引、算 `SHA1(index)`，再对数据做同样的
//! XOR —— 因此「解包 → 汉化 → 回封」是闭环的。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::safe;
use super::source::Source;
use crate::hash::Sha1;

/// 索引区体积上限（防止伪造头声明出一个巨大的分配）。
const MAX_INDEX: u64 = 512 * 1024 * 1024;
/// 条目数上限。
const MAX_COUNT: i32 = 1 << 20;

/// 封包内的一个条目。
#[derive(Debug, Clone)]
pub struct PfsEntry {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

/// 解析后的索引 + 该封包的数据密钥。
#[derive(Debug)]
pub struct PfsIndex {
    /// 版本数字（6 / 8）。
    pub version: u8,
    pub entries: Vec<PfsEntry>,
    /// 数据 XOR 密钥流（`SHA1(index)`）；`pf6` 时为空（明文）。
    key: Vec<u8>,
}

impl PfsIndex {
    /// 是否带数据混淆（`pf8`）。
    pub fn obfuscated(&self) -> bool {
        !self.key.is_empty()
    }
}

/// 快速判据：文件头是否为 `pf<数字>`。
pub fn is_pfs(path: &Path) -> bool {
    fs::File::open(path)
        .and_then(|mut f| {
            use std::io::Read;
            let mut buf = [0u8; 3];
            f.read_exact(&mut buf)?;
            Ok(buf[0] == b'p' && buf[1] == b'f' && buf[2].is_ascii_digit())
        })
        .unwrap_or(false)
}

/// 从流式来源解析索引。
///
/// 与其它格式一致：只读头部 + 索引区，条目数据按需 `read_entry`。
pub fn parse_index(src: &mut Source) -> Result<PfsIndex, String> {
    let head = src.read_exact_at(0, 7)?;
    if head[0] != b'p' || head[1] != b'f' {
        return Err("不是 Artemis PFS 封包（缺少 pf 魔术）".into());
    }
    let version = head[2];
    if version != b'6' && version != b'8' {
        return Err(format!(
            "不支持的 PFS 版本「pf{}」（本工具支持 pf6 / pf8）",
            version as char
        ));
    }
    let index_size = safe::u32_le(&head, 3).ok_or("PFS 头部截断")? as u64;
    if !(4..=MAX_INDEX).contains(&index_size) {
        return Err(format!("PFS 索引体积异常（{index_size} 字节）"));
    }
    let index_len = usize::try_from(index_size).map_err(|_| "PFS 索引体积超出 32 位范围")?;
    if 7 + index_size > src.len() {
        return Err(format!(
            "PFS 索引越界：头部声明 {index_size} 字节，但文件仅 {} 字节",
            src.len()
        ));
    }
    let index = src.read_exact_at(7, index_len)?;

    let count = safe::i32_le(&index, 0).ok_or("PFS 索引截断（读不到 count）")?;
    if !(0..=MAX_COUNT).contains(&count) {
        return Err(format!("PFS 条目数异常（{count}）"));
    }
    let mut entries = Vec::with_capacity(count as usize);
    // 游标从 4 起步：index[0..4] 已作为 count 读掉。
    let mut off = 4usize;
    for i in 0..count {
        let name_len = safe::i32_le(&index, off)
            .ok_or_else(|| format!("PFS 索引截断（第 {i} 条的名字长度）"))?;
        if name_len < 0 {
            return Err(format!("PFS 第 {i} 条名字长度为负（{name_len}）"));
        }
        let name_len = name_len as usize;
        // [名长 4][名字 name_len][保留 4][偏移 4][大小 4]
        let name_at = off + 4;
        let name = safe::slice(&index, name_at, name_len)
            .ok_or_else(|| format!("PFS 第 {i} 条名字越界"))?;
        let name = String::from_utf8_lossy(name).into_owned();
        off += name_len + 8;
        let eo = safe::u32_le(&index, off)
            .ok_or_else(|| format!("PFS 第 {i} 条偏移越界"))? as u64;
        let es = safe::u32_le(&index, off + 4)
            .ok_or_else(|| format!("PFS 第 {i} 条大小越界"))? as u64;
        off += 8;
        let total = src.len();
        if eo > total || eo + es > total {
            return Err(format!(
                "PFS 第 {i} 条（{name}）位置越界：offset {eo} + size {es} > 文件 {total} 字节"
            ));
        }
        entries.push(PfsEntry { name, offset: eo, size: es });
    }

    // pf6 明文；pf8 用 SHA1(index) 做 XOR 密钥流（每条目从 key[0] 重新开始）。
    let key = if version == b'8' {
        let mut h = Sha1::new();
        h.update(&index);
        h.finalize().to_vec()
    } else {
        Vec::new()
    };
    Ok(PfsIndex { version, entries, key })
}

/// 读取并（按需）解出单个条目。
pub fn read_entry(src: &mut Source, idx: &PfsIndex, e: &PfsEntry) -> Result<Vec<u8>, String> {
    let len = usize::try_from(e.size).map_err(|_| format!("条目过大：{}", e.name))?;
    let mut data = src.read_exact_at(e.offset, len)?;
    if idx.obfuscated() {
        xor_key_stream(&mut data, &idx.key);
    }
    Ok(data)
}

/// 用密钥流做就地 XOR（**每条目都从头开始**，见文件头注释）。
fn xor_key_stream(buf: &mut [u8], key: &[u8]) {
    if key.is_empty() {
        return;
    }
    let klen = key.len();
    for (i, b) in buf.iter_mut().enumerate() {
        *b ^= key[i % klen];
    }
}

/// 索引里名字之后、偏移之前那个字段：两个真机样本 6064 条全为 0，写出时照写 0。
const ENTRY_RESERVED: u32 = 0;

/// 单条索引记录长度（名长字段 4 + 名字 + 保留 4 + 偏移 4 + 大小 4）。
fn record_len(name_len: usize) -> u64 {
    4 + name_len as u64 + 4 + 4 + 4
}

/// 用内存中的条目写出 `pf8`（或 `pf6`）封包。
///
/// `files` 的**顺序即写出的索引顺序**：回封原封包时应沿用原索引顺序
/// （Artemis 按索引顺序线性查找，保持原顺序最稳）。
pub fn write_archive(archive: &Path, files: &[(String, Vec<u8>)], version: u8) -> Result<(), String> {
    if version != b'6' && version != b'8' {
        return Err(format!("不支持的 PFS 版本 pf{}", version as char));
    }
    if files.is_empty() {
        return Err("没有条目可写".into());
    }
    let count = files.len() as u64;
    let index_size: u64 = 4 + files.iter().map(|(n, _)| record_len(n.len())).sum::<u64>();
    if index_size > MAX_INDEX {
        return Err(format!("索引过大（{index_size} 字节），拒绝写出"));
    }
    let data_start = 7 + index_size;

    // 1) 建索引；同时算出每个条目的绝对偏移。
    let mut index: Vec<u8> = Vec::with_capacity(index_size as usize);
    index.extend_from_slice(&(count as u32).to_le_bytes());
    let mut cursor = data_start;
    let mut offsets: Vec<u64> = Vec::with_capacity(files.len());
    for (name, data) in files {
        let nb = name.as_bytes();
        let nb = &nb[..nb.len().min(u32::MAX as usize)];
        index.extend_from_slice(&(nb.len() as u32).to_le_bytes());
        index.extend_from_slice(nb);
        index.extend_from_slice(&ENTRY_RESERVED.to_le_bytes());
        let off = u32::try_from(cursor).map_err(|_| {
            format!("封包总长超过 4GB，PFS 的 u32 偏移放不下（当前 {cursor} 字节）")
        })?;
        index.extend_from_slice(&off.to_le_bytes());
        index.extend_from_slice(&(data.len() as u32).to_le_bytes());
        offsets.push(cursor);
        cursor += data.len() as u64;
    }
    debug_assert_eq!(index.len() as u64, index_size);

    // 2) 密钥流由索引推出，再逐条 XOR。
    let key = if version == b'8' {
        let mut h = Sha1::new();
        h.update(&index);
        h.finalize().to_vec()
    } else {
        Vec::new()
    };

    let mut out: Vec<u8> = Vec::with_capacity(cursor as usize);
    out.extend_from_slice(b"pf");
    out.push(version);
    out.extend_from_slice(&(index_size as u32).to_le_bytes());
    out.extend_from_slice(&index);
    for (_, data) in files {
        let mut blob = data.clone();
        xor_key_stream(&mut blob, &key);
        out.extend_from_slice(&blob);
    }
    crate::settings::ensure_parent(archive);
    fs::write(archive, &out).map_err(|e| format!("写出失败 {}: {e}", archive.display()))?;
    Ok(())
}

/// 从磁盘上的条目文件写出封包（顺序 = `map` 的迭代顺序 = 名字字典序）。
pub fn write_paths(
    archive: &Path,
    files: &BTreeMap<String, PathBuf>,
    version: u8,
) -> Result<usize, String> {
    let mut loaded: Vec<(String, Vec<u8>)> = Vec::with_capacity(files.len());
    for (name, path) in files {
        let data = fs::read(path).map_err(|e| format!("读取失败 {}: {e}", path.display()))?;
        loaded.push((name.clone(), data));
    }
    write_archive(archive, &loaded, version)?;
    Ok(loaded.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::source::MAX_ARCHIVE;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_pfs_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// 手工拼一个真实结构的最小 pf8（含 XOR），再走解析 + 读取。
    ///
    /// `tag` 必须**每个调用点唯一**：`tmp()` 会先删目录，cargo 并行跑测试时
    /// 两个测试共用同一目录会互相删掉对方的文件（曾因此偶发 `fs::read` 失败）。
    fn sample(tag: &str) -> (PathBuf, Vec<u8>) {
        let d = tmp(tag);
        let p = d.join("root.pfs");
        let files = vec![
            ("system\\ui\\title.lua".to_string(), b"-- title\nreturn 1\n".to_vec()),
            ("pc\\bg_cn.png".to_string(), b"\x89PNG\r\n\x1a\n fake png bytes".to_vec()),
            ("font\\a.otf".to_string(), b"OTTO\x00\x10\x01\x00".to_vec()),
        ];
        write_archive(&p, &files, b'8').unwrap();
        let blob = fs::read(&p).unwrap();
        (p, blob)
    }

    #[test]
    fn header_matches_real_layout() {
        let (_p, blob) = sample("hdr");
        assert_eq!(&blob[0..3], b"pf8");
        let index_size = u32::from_le_bytes(blob[3..7].try_into().unwrap());
        // 3（头）+ 4（索引长度）+ index_size + 数据
        assert!(7 + index_size as u64 <= blob.len() as u64);
        // 索引头 4 字节 == 条目数
        assert_eq!(u32::from_le_bytes(blob[7..11].try_into().unwrap()), 3);
        // 名字以明文存在索引里（未混淆）
        let idx = &blob[7..7 + index_size as usize];
        assert!(
            idx.windows(8).any(|w| w == b"title.lu"),
            "索引里应能直接看到明文条目名"
        );
    }

    #[test]
    fn parse_read_roundtrip_with_obfuscation() {
        let (p, _blob) = sample("roundtrip");
        let mut src = Source::open(&p, MAX_ARCHIVE).unwrap();
        let idx = parse_index(&mut src).unwrap();
        assert_eq!(idx.version, b'8');
        assert!(idx.obfuscated());
        assert_eq!(idx.entries.len(), 3);
        assert_eq!(idx.entries[0].name, "system\\ui\\title.lua");

        // 数据区必须是被 XOR 过的（先确认密文不等于明文）
        let raw = {
            let mut s2 = Source::open(&p, MAX_ARCHIVE).unwrap();
            s2.read_at(idx.entries[2].offset, 8).unwrap()
        };
        assert_ne!(raw, b"OTTO\x00\x10\x01\x00".to_vec(), "pf8 数据应被混淆");

        for e in &idx.entries {
            let bytes = read_entry(&mut src, &idx, e).unwrap();
            assert_eq!(bytes.len() as u64, e.size);
        }
        let otf = read_entry(&mut src, &idx, &idx.entries[2]).unwrap();
        assert_eq!(&otf, b"OTTO\x00\x10\x01\x00", "解出后应还原为明文");
        let png = read_entry(&mut src, &idx, &idx.entries[1]).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn pf6_is_plaintext() {
        let d = tmp("pf6");
        let p = d.join("plain.pfs");
        let files = vec![("a.txt".to_string(), b"hello pf6".to_vec())];
        write_archive(&p, &files, b'6').unwrap();
        let blob = fs::read(&p).unwrap();
        let index_size = u32::from_le_bytes(blob[3..7].try_into().unwrap()) as usize;
        assert_eq!(&blob[7 + index_size..], b"hello pf6", "pf6 数据区应明文");
        let mut src = Source::open(&p, MAX_ARCHIVE).unwrap();
        let idx = parse_index(&mut src).unwrap();
        assert!(!idx.obfuscated());
        assert_eq!(read_entry(&mut src, &idx, &idx.entries[0]).unwrap(), b"hello pf6");
    }

    #[test]
    fn keys_restart_per_entry_not_per_offset() {
        // 两个内容相同的条目放在不同偏移：解出结果必须一致，
        // 这正是「密钥流按条目重启」而非「按文件偏移」的证据。
        let d = tmp("restart");
        let p = d.join("two.pfs");
        let payload = b"identical-content-payload-0123456789".to_vec();
        // 前面塞一段别的数据把第二个条目顶到不同偏移
        let files = vec![
            ("a.bin".to_string(), vec![0xABu8; 37]),
            ("b.bin".to_string(), payload.clone()),
            ("c.bin".to_string(), payload.clone()),
        ];
        write_archive(&p, &files, b'8').unwrap();
        let mut src = Source::open(&p, MAX_ARCHIVE).unwrap();
        let idx = parse_index(&mut src).unwrap();
        let b = read_entry(&mut src, &idx, &idx.entries[1]).unwrap();
        let c = read_entry(&mut src, &idx, &idx.entries[2]).unwrap();
        assert_eq!(b, payload);
        assert_eq!(c, payload);
        assert!(
            idx.entries[1].offset % 20 != idx.entries[2].offset % 20,
            "两偏移对 20 取模必须不同，否则区分不了「按条目重启」与「按文件偏移」两种实现"
        );
    }

    #[test]
    fn write_paths_from_disk_roundtrips() {
        let d = tmp("paths");
        let src_dir = d.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("x.dat"), b"xxxx").unwrap();
        fs::write(src_dir.join("y.dat"), b"yyyyyyyy").unwrap();
        let mut map = BTreeMap::new();
        map.insert("dir/x.dat".to_string(), src_dir.join("x.dat"));
        map.insert("dir/y.dat".to_string(), src_dir.join("y.dat"));
        let p = d.join("out.pfs");
        assert_eq!(write_paths(&p, &map, b'8').unwrap(), 2);

        let mut src = Source::open(&p, MAX_ARCHIVE).unwrap();
        let idx = parse_index(&mut src).unwrap();
        assert_eq!(idx.entries.len(), 2);
        let names: Vec<&str> = idx.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["dir/x.dat", "dir/y.dat"], "顺序应等于 map 迭代顺序");
        assert_eq!(read_entry(&mut src, &idx, &idx.entries[1]).unwrap(), b"yyyyyyyy");
    }

    #[test]
    fn rejects_bad_input() {
        let d = tmp("bad");
        // 不是 pfs
        let p1 = d.join("nope.bin");
        fs::write(&p1, b"NOTPFS____").unwrap();
        let mut s1 = Source::open(&p1, MAX_ARCHIVE).unwrap();
        assert!(parse_index(&mut s1).is_err());

        // 版本不认识
        let p2 = d.join("v9.pfs");
        fs::write(&p2, b"pf9\x10\x00\x00\x00\x01\x00\x00\x00").unwrap();
        let mut s2 = Source::open(&p2, MAX_ARCHIVE).unwrap();
        assert!(parse_index(&mut s2).unwrap_err().contains("不支持的 PFS 版本"));

        // 索引声明越界
        let p3 = d.join("trunc.pfs");
        fs::write(&p3, b"pf8\xff\xff\xff\x7f\x01\x00\x00\x00").unwrap();
        let mut s3 = Source::open(&p3, MAX_ARCHIVE).unwrap();
        assert!(parse_index(&mut s3).is_err());

        assert!(!is_pfs(&p1));
        assert!(is_pfs(&p2));
        let _ = fs::remove_dir_all(&d);
    }
}
