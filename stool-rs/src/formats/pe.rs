//! 读 PE 文件里的 **VERSIONINFO（版本资源）** —— 只读文件头 + `.rsrc` 节，绝不整文件进内存。
//!
//! 用途：认出游戏目录里 NW.js 运行时的版本（`nw.dll` / `Game.exe` 的 `FileVersion`）。
//! 为什么需要它：**NW.js ≥ 0.70 起上游坏了 `--remote-debugging-port`**（[nwjs/nw.js#8191]），
//! 「方式一·游戏里改数值」在这些游戏上**任何启动姿势都开不出端口** —— 得先认出这一档，
//! 才能跟用户说人话、并把他引到别的出路（见 `features::runtime`）。
//!
//! [nwjs/nw.js#8191]: https://github.com/nwjs/nw.js/issues/8191
//!
//! 走的是「读 `.rsrc` 节 → 在里面找 UTF-16LE 的键名」这条**简单但够稳**的路：
//! 版本资源就在 `.rsrc` 里，键名（`FileVersion` 等）在整节里基本唯一，
//! 省掉了遍历资源目录三级树那一堆边界判断。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// 头部窗口：足够覆盖 DOS 头 + PE 签名 + COFF + 可选头 + 节表（实际 e_lfanew 通常 0x100 上下）。
const HEADER_WINDOW: usize = 0x1000;
/// `.rsrc` 读取上限 —— 实测真实 `nw.dll` 的 `.rsrc` 约 356 KB；设个上限防畸形文件吃内存。
const MAX_RSRC: u64 = 8 * 1024 * 1024;
/// 节表里最多看多少个节。
const MAX_SECTIONS: usize = 96;

/// 一个 PE 文件的版本资源（字段缺失就是 `None`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionInfo {
    pub file_version: Option<String>,
    pub product_version: Option<String>,
    pub product_name: Option<String>,
}

impl VersionInfo {
    pub fn is_empty(&self) -> bool {
        self.file_version.is_none() && self.product_version.is_none() && self.product_name.is_none()
    }
}

/// 从一个 PE 文件读版本资源；不是 PE / 没有 `.rsrc` / 读不动 → `None`。
pub fn version_info(path: &Path) -> Option<VersionInfo> {
    let mut f = File::open(path).ok()?;
    version_info_from(&mut f)
}

/// 同 [`version_info`]，但读任意可寻址来源（单测用 `Cursor` 喂合成 PE）。
pub fn version_info_from<R: Read + Seek>(r: &mut R) -> Option<VersionInfo> {
    let (off, size) = rsrc_range(r)?;
    let size = size.min(MAX_RSRC);
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    r.seek(SeekFrom::Start(off)).ok()?;
    r.read_exact(&mut buf).ok()?;
    let v = version_info_from_rsrc(&buf);
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// 从 `.rsrc` 节的原始字节里抽三个字段。
///
/// 拆成独立函数是为了**能用真实固件直接测**（`tests/fixtures/pe/*.bin` 是从真实 `nw.dll` 抠出来的）。
pub fn version_info_from_rsrc(rsrc: &[u8]) -> VersionInfo {
    VersionInfo {
        file_version: value_after(rsrc, "FileVersion"),
        product_version: value_after(rsrc, "ProductVersion"),
        product_name: value_after(rsrc, "ProductName"),
    }
}

/// `"0.76.1"` → `(0, 76, 1)`；`"0.48.4"` → `(0, 48, 4)`。
/// 只有一段（`"3"`）按 `(3, 0, 0)`；缺第三段的按 0。任一段不是数字 → `None`。
pub fn parse_triple(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.trim().split('.');
    let a: u32 = it.next()?.trim().parse().ok()?;
    let b: u32 = it.next().unwrap_or("0").trim().parse().ok()?;
    let c: u32 = it.next().unwrap_or("0").trim().parse().ok()?;
    Some((a, b, c))
}

// ---------- PE 头部：找 `.rsrc` 的 (文件偏移, 大小) ----------

fn rsrc_range<R: Read + Seek>(r: &mut R) -> Option<(u64, u64)> {
    let mut head = vec![0u8; HEADER_WINDOW];
    r.seek(SeekFrom::Start(0)).ok()?;
    let n = r.read(&mut head).ok()?;
    head.truncate(n);

    if head.get(0..2)? != b"MZ" {
        return None;
    }
    let e_lfanew = rd_u32(&head, 0x3C)? as usize;
    if e_lfanew < 0x40 || e_lfanew + 24 > head.len() {
        return None;
    }
    if head.get(e_lfanew..e_lfanew + 4)? != b"PE\0\0" {
        return None;
    }
    let coff = e_lfanew + 4;
    let nsec = rd_u16(&head, coff + 2)? as usize;
    let opt_size = rd_u16(&head, coff + 16)? as usize;
    let sec_tab = coff + 20 + opt_size;
    // 节表必须整段落在已读窗口里；落不下就放弃（真实文件不会这样）。
    if sec_tab + nsec.min(MAX_SECTIONS) * 40 > head.len() {
        return None;
    }
    for i in 0..nsec.min(MAX_SECTIONS) {
        let s = sec_tab + i * 40;
        let raw = head.get(s..s + 8)?;
        let name = raw.split(|&c| c == 0).next().unwrap_or(&[]);
        if name == b".rsrc" {
            let size = rd_u32(&head, s + 16)? as u64; // SizeOfRawData
            let ptr = rd_u32(&head, s + 20)? as u64; // PointerToRawData
            return Some((ptr, size));
        }
    }
    None
}

// ---------- VS_VERSIONINFO 里取字符串 ----------

fn is_printable(b: u8) -> bool {
    (0x20..=0x7e).contains(&b)
}

fn find_utf16(hay: &[u8], needle: &str) -> Option<usize> {
    if needle.is_empty() || hay.len() < 2 {
        return None;
    }
    let mut pat = Vec::with_capacity(needle.len() * 2);
    for u in needle.encode_utf16() {
        pat.extend_from_slice(&u.to_le_bytes());
    }
    hay.windows(pat.len()).position(|w| w == pat.as_slice())
}

/// 找到 UTF-16LE 的键名后，跳过它的 NUL 结尾与对齐填充，取出紧随其后的字符串值。
///
/// 布局（VS_VERSIONINFO 的 String 节点）：`wLength, wValueLength, wType, szKey, padding, Value`。
/// 这里不真去解 `wValueLength`，而是按「键后面第一段可打印 UTF-16」来取 —— 够稳且省掉一堆对齐判断。
fn value_after(hay: &[u8], key: &str) -> Option<String> {
    let at = find_utf16(hay, key)?;
    let mut p = at + key.len() * 2;
    let limit = (p + 16).min(hay.len());
    while p + 1 < limit && !(is_printable(hay[p]) && hay[p + 1] == 0) {
        p += 2;
    }
    let mut out = String::new();
    while p + 1 < hay.len() && hay[p + 1] == 0 && is_printable(hay[p]) {
        out.push(hay[p] as char);
        p += 2;
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn rd_u16(b: &[u8], off: usize) -> Option<u16> {
    let s = b.get(off..off + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

fn rd_u32(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    // 真实固件：从真实 nw.dll 的 .rsrc 里抠出的 RT_VERSION 资源（见 tests/fixtures/pe/README 注释）。
    const NWJS_0761: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pe/nwjs_0_76_1_version_resource.bin"));
    const NWJS_0484: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pe/nwjs_0_48_4_version_resource.bin"));

    #[test]
    fn real_fixture_0761_reads_expected_fields() {
        let v = version_info_from_rsrc(NWJS_0761);
        assert_eq!(v.file_version.as_deref(), Some("0.76.1"));
        assert_eq!(v.product_version.as_deref(), Some("0.76.1"));
        assert_eq!(v.product_name.as_deref(), Some("nwjs"));
    }

    #[test]
    fn real_fixture_0484_reads_expected_fields() {
        let v = version_info_from_rsrc(NWJS_0484);
        assert_eq!(v.file_version.as_deref(), Some("0.48.4"));
        assert_eq!(v.product_name.as_deref(), Some("nwjs"));
        // 两款游戏必须能区分开 —— 这正是「要不要给用户换说法」的判据
        assert_ne!(v.file_version, version_info_from_rsrc(NWJS_0761).file_version);
    }

    #[test]
    fn garbage_rsrc_yields_empty() {
        assert!(version_info_from_rsrc(b"").is_empty());
        assert!(version_info_from_rsrc(&[0u8; 512]).is_empty());
        // 有键名但后面没有值 → 该字段仍是 None
        let mut only_key = Vec::new();
        for u in "FileVersion".encode_utf16() {
            only_key.extend_from_slice(&u.to_le_bytes());
        }
        assert!(version_info_from_rsrc(&only_key).file_version.is_none());
    }

    /// 合成一个最小 PE：DOS 头 + PE 签名 + COFF(nsec=1) + 可选头占位 + 一个 `.rsrc` 节。
    /// 供 `features::runtime` 的测试复用（那里要造一个名为 `nw.dll` 的文件走完整路径）。
    pub(crate) fn synthetic_pe(rsrc: &[u8]) -> Vec<u8> {
        let e_lfanew = 0x80usize;
        let opt_size = 0xE0usize;
        let sec_tab = e_lfanew + 4 + 20 + opt_size;
        let rsrc_off = 0x400usize;
        let mut b = vec![0u8; rsrc_off + rsrc.len()];
        b[0..2].copy_from_slice(b"MZ");
        b[0x3C..0x40].copy_from_slice(&(e_lfanew as u32).to_le_bytes());
        b[e_lfanew..e_lfanew + 4].copy_from_slice(b"PE\0\0");
        let coff = e_lfanew + 4;
        b[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // NumberOfSections
        b[coff + 16..coff + 18].copy_from_slice(&(opt_size as u16).to_le_bytes());
        // 可选头 magic = PE32+（0x20B），版本读取并不依赖它，只是让结构看起来正常
        b[coff + 20..coff + 22].copy_from_slice(&0x20Bu16.to_le_bytes());
        // 节表：name=.rsrc, SizeOfRawData @+16, PointerToRawData @+20
        b[sec_tab..sec_tab + 5].copy_from_slice(b".rsrc");
        b[sec_tab + 16..sec_tab + 20].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
        b[sec_tab + 20..sec_tab + 24].copy_from_slice(&(rsrc_off as u32).to_le_bytes());
        b[rsrc_off..].copy_from_slice(rsrc);
        b
    }

    #[test]
    fn walks_pe_and_reads_version_from_rsrc_section() {
        let pe = synthetic_pe(NWJS_0761);
        let v = version_info_from(&mut Cursor::new(pe)).expect("应能读出");
        assert_eq!(v.file_version.as_deref(), Some("0.76.1"));
    }

    #[test]
    fn non_pe_and_missing_rsrc_return_none() {
        assert!(version_info_from(&mut Cursor::new(b"not a pe".to_vec())).is_none());
        // 合法 PE 但没有 .rsrc 节
        let mut pe = synthetic_pe(&[0u8; 16]);
        let sec_tab = 0x80 + 4 + 20 + 0xE0;
        pe[sec_tab..sec_tab + 8].copy_from_slice(b".data\0\0\0");
        assert!(version_info_from(&mut Cursor::new(pe)).is_none());
    }

    #[test]
    fn truncation_never_panics() {
        let pe = synthetic_pe(NWJS_0761);
        for cut in [0usize, 1, 2, 0x3C, 0x40, 0x80, 0x90, 0x200, 0x3FF, 0x401] {
            let _ = version_info_from(&mut Cursor::new(pe[..cut.min(pe.len())].to_vec()));
        }
    }

    #[test]
    fn parse_triple_handles_real_and_odd_forms() {
        assert_eq!(parse_triple("0.76.1"), Some((0, 76, 1)));
        assert_eq!(parse_triple("0.48.4"), Some((0, 48, 4)));
        assert_eq!(parse_triple("1.2.3.4"), Some((1, 2, 3)));
        assert_eq!(parse_triple("3"), Some((3, 0, 0)));
        assert_eq!(parse_triple(" 0.76.1 "), Some((0, 76, 1)));
        assert_eq!(parse_triple("abc"), None);
        assert_eq!(parse_triple("1.x"), None);
        assert_eq!(parse_triple(""), None);
    }
}
