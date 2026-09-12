//! 最小 .NET / PE 元数据读取（零依赖，只读不执行）。
//!
//! 用途：Unity **Mono 版**游戏的 `*_Data/Managed/Assembly-CSharp.dll` 是标准
//! .NET 程序集，里面的字符串字面量就是 `PlayerPrefs.SetInt("键", 1)` 里那个"键"。
//! 读出来即可把「全 CG 解锁」的候选键名从**启发式场景扫描**升级为**精确命中**，
//! 不必依赖 ilspycmd / dnSpy，也不必联网或安装 .NET 运行时。
//!
//! 只做"读元数据字符串"，不做反编译、不加载程序集、不执行任何代码：
//! - `#US` 堆：所有用户字符串字面量（UTF-16LE，长度前缀为 ECMA-335 压缩整数）；
//! - `#Strings` 堆：类型名 / 字段名 / 方法名等（UTF-8，NUL 分隔）。
//!
//! 解析路径：DOS 头 → PE 头 → 可选头 → 节表（RVA→文件偏移）→ CLI 头（数据目录 14）
//! → 元数据根（签名 `BSJB`）→ 流头（`#Strings` / `#US`）。
//!
//! 所有越界一律返回 `Err`，绝不 panic（与 `formats::safe` 同约定）。

use super::safe::{self, Cursor};
use super::source::Source;

/// 元数据根签名 `BSJB`。
const BSJB: u32 = 0x424A_5342;
/// 单文件上限（Assembly-CSharp.dll 通常几 MB，几十 MB 已属异常）。
const MAX_ASSEMBLY: u64 = 512 * 1024 * 1024;

/// 从程序集里读出的字符串。
#[derive(Debug, Default, Clone)]
pub struct DotnetStrings {
    /// `#US` 堆里的用户字符串字面量（去重后保序）。
    pub user_strings: Vec<String>,
    /// `#Strings` 堆里的名字（类型/字段/方法）。
    pub names: Vec<String>,
}

impl DotnetStrings {
    /// 只在名字里找"像全 CG 画廊"的标识（类名 / 字段名）。
    pub fn gallery_hits(&self) -> Vec<String> {
        gallery_hits_of(&self.names)
    }

    /// 名字里是否出现过"画廊"相关的类型（用于给用户一句"找到了什么"）。
    pub fn has_gallery_type(&self) -> bool {
        !gallery_hits_of(&self.names).is_empty()
    }
}

/// 从一组"名字"里挑出像画廊的标识，去重后按字典序返回。
///
/// 供 `.NET 程序集`（[`DotnetStrings`]）与 `IL2CPP 元数据`（`formats::il2cpp`）共用。
pub(crate) fn gallery_hits_of(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        if is_gallery_like(n) && !out.iter().any(|x| x == n) {
            out.push(n.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 单个名字是否"像全 CG 画廊"的标识。
///
/// 判据保守，避免把 `CgColor`、`Camera` 这类常见名当命中。
pub(crate) fn is_gallery_like(name: &str) -> bool {
    let n = name.trim_start_matches('_');
    if n.len() < 3 {
        return false;
    }
    let low = n.to_ascii_lowercase();
    for kw in [
        "gallery",
        "album",
        "recollection",
        "omake",
        "artbook",
        "cglist",
        "kaisou",
        "回想",
        "auto_cg",
        "autocg",
        "allcg",
        "cgflag",
        "cg_flag",
        // Unity 画廊组件里最常见的字段名（`_wholeNameList` / `wholeNameList`）
        "wholename",
    ] {
        if low.contains(kw) {
            return true;
        }
    }
    // `cg` 开头的复合名：只认 `cg_` / `cg0`…`cg9`（如 cg_flag、CG01、cg1_2）。
    // 刻意**不认**「第 3 个字符是大写字母」——那会把 NVIDIA Cg 运行时的
    // `CgColor` / `CgProgram` / `CgShader` 误判成画廊字段。
    let bytes = low.as_bytes();
    if bytes.starts_with(b"cg") && bytes.len() > 2 {
        return bytes[2] == b'_' || bytes[2].is_ascii_digit();
    }
    false
}

/// 打开文件并读元数据字符串。
pub fn read_strings(path: &std::path::Path) -> Result<DotnetStrings, String> {
    let mut src = Source::open(path, MAX_ASSEMBLY)?;
    let len = src.len();
    if len < 0x40 {
        return Err("文件过小，不是 .NET 程序集".into());
    }
    let blob = src.read_exact_at(0, len as usize)?;
    parse_metadata(&blob)
}

/// 解析 PE / CLI 元数据，返回字符串堆内容。
pub fn parse_metadata(blob: &[u8]) -> Result<DotnetStrings, String> {
    // ── 1. DOS 头 → PE 偏移 ──
    if safe::slice(blob, 0, 2) != Some(b"MZ") {
        return Err("不是 PE 文件（缺少 MZ）".into());
    }
    let pe_off = safe::u32_le(blob, 0x3C).ok_or("PE 头偏移越界")? as usize;
    if safe::slice(blob, pe_off, 4) != Some(b"PE\0\0") {
        return Err("PE 签名不匹配".into());
    }
    let coff = pe_off + 4;
    let num_sections = safe::u16_le(blob, coff + 2).ok_or("COFF 头截断")? as usize;
    let size_opt = safe::u16_le(blob, coff + 16).ok_or("COFF 头截断")? as usize;
    if num_sections == 0 || num_sections > 96 {
        return Err(format!("节表数量异常（{num_sections}）"));
    }
    let opt = coff + 20;
    let magic = safe::u16_le(blob, opt).ok_or("可选头截断")?;
    // 0x10b = PE32，0x20b = PE32+
    let (dir_off, num_dirs_off) = match magic {
        0x10b => (opt + 96, opt + 92),
        0x20b => (opt + 112, opt + 108),
        m => return Err(format!("未知的可选头 Magic（0x{m:04x}）")),
    };
    let num_dirs = safe::u32_le(blob, num_dirs_off).ok_or("可选头截断")? as usize;
    if num_dirs <= 14 {
        return Err("没有 CLI 数据目录（不是 .NET 程序集）".into());
    }
    // 数据目录 14 = CLI 头
    let cli_rva = safe::u32_le(blob, dir_off + 14 * 8).ok_or("CLI 目录越界")? as usize;

    // ── 2. 节表：RVA → 文件偏移 ──
    let secs = opt + size_opt;
    let mut sections: Vec<(usize, usize, usize, usize)> = Vec::with_capacity(num_sections);
    for i in 0..num_sections {
        let s = secs + i * 40;
        let va = safe::u32_le(blob, s + 12).ok_or("节表截断")? as usize;
        let vsize = safe::u32_le(blob, s + 8).ok_or("节表截断")? as usize;
        let raw_size = safe::u32_le(blob, s + 16).ok_or("节表截断")? as usize;
        let raw_ptr = safe::u32_le(blob, s + 20).ok_or("节表截断")? as usize;
        sections.push((va, vsize.max(raw_size), raw_size, raw_ptr));
    }
    let rva2off = |rva: usize| -> Option<usize> {
        for &(va, vsize, raw_size, raw_ptr) in &sections {
            if rva >= va && rva < va + vsize.max(1) {
                let delta = rva - va;
                if delta < raw_size {
                    return Some(raw_ptr + delta);
                }
            }
        }
        None
    };

    // ── 3. CLI 头 → 元数据根 ──
    let cli = rva2off(cli_rva).ok_or("CLI 头的 RVA 不在任何节内")?;
    let md_rva = safe::u32_le(blob, cli + 8).ok_or("CLI 头截断")? as usize;
    let md_off = rva2off(md_rva).ok_or("元数据根的 RVA 不在任何节内")?;

    // ── 4. 元数据根 + 流头 ──
    if safe::u32_le(blob, md_off) != Some(BSJB) {
        return Err("元数据签名不是 BSJB".into());
    }
    let ver_len = safe::u32_le(blob, md_off + 12).ok_or("元数据根截断")? as usize;
    // 版本字符串按 4 字节对齐
    let after_ver = md_off + 16 + ver_len.div_ceil(4) * 4;
    // 版本串之后依次是 Flags(2) + Streams(2)；流数量在 Flags **之后**，
    // 若直接读 after_ver 会拿到 Flags（恒 0）→ 误判"流数量异常（0）"。
    let streams = safe::u16_le(blob, after_ver + 2).ok_or("元数据根截断")? as usize;
    if streams == 0 || streams > 64 {
        return Err(format!("元数据流数量异常（{streams}）"));
    }
    let mut cur = after_ver + 4; // Flags(2) + Streams(2)
    let mut us: Option<(usize, usize)> = None;
    let mut strings: Option<(usize, usize)> = None;
    for _ in 0..streams {
        let off = safe::u32_le(blob, cur).ok_or("流头截断")? as usize;
        let size = safe::u32_le(blob, cur + 4).ok_or("流头截断")? as usize;
        let name_at = cur + 8;
        let raw = safe::slice(blob, name_at, 32).ok_or("流头名字越界")?;
        let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        let name = String::from_utf8_lossy(&raw[..end]).into_owned();
        match name.as_str() {
            "#US" => us = Some((off, size)),
            "#Strings" => strings = Some((off, size)),
            _ => {}
        }
        // 名字 NUL 结尾并按 4 字节对齐
        cur = name_at + (end + 1).div_ceil(4) * 4;
    }

    let mut out = DotnetStrings::default();
    // 流头里的 Offset 是**相对元数据根**的，必须加上 md_off 才是文件偏移。
    if let Some((off, size)) = us {
        let heap = safe::slice(blob, md_off + off, size).ok_or("#US 堆越界")?;
        out.user_strings = parse_user_strings(heap);
    }
    if let Some((off, size)) = strings {
        let heap = safe::slice(blob, md_off + off, size).ok_or("#Strings 堆越界")?;
        out.names = parse_strings_heap(heap);
    }
    if out.user_strings.is_empty() && out.names.is_empty() {
        return Err("未读到任何字符串堆（可能被裁剪或加密）".into());
    }
    Ok(out)
}

/// `#US` 堆：`[压缩长度][UTF-16LE]…`。长度含一个尾随字节，故实际字符字节数 = len-1。
fn parse_user_strings(heap: &[u8]) -> Vec<String> {
    let mut cur = Cursor::new(heap);
    // 第 0 项是空串（保留项），跳过
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
    while cur.remaining() > 0 {
        let Some(n) = read_compressed(&mut cur) else { break };
        if n == 0 {
            continue;
        }
        let body = n - 1; // 去掉尾随的"是否含非 ASCII"标志字节
        let Some(raw) = cur.take(body) else { break };
        // UTF-16LE → String（奇数长度或非法码位时退回宽松解码）
        let units: Vec<u16> =
            raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        let s = String::from_utf16_lossy(&units);
        if s.is_empty() {
            continue;
        }
        if seen.insert(s.as_bytes().to_vec()) {
            out.push(s);
        }
    }
    out
}

/// `#Strings` 堆 / `global-metadata.dat` 的 string 区：NUL 分隔的 UTF-8。
pub(crate) fn parse_strings_heap(heap: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;
    for i in 0..heap.len() {
        if heap[i] == 0 {
            if i > start {
                if let Ok(s) = std::str::from_utf8(&heap[start..i]) {
                    out.push(s.to_string());
                }
            }
            start = i + 1;
        }
    }
    out
}

/// ECMA-335 压缩无符号整数（1 / 2 / 4 字节）。
fn read_compressed(cur: &mut Cursor<'_>) -> Option<usize> {
    let b0 = cur.u8()?;
    if b0 & 0x80 == 0 {
        return Some(b0 as usize);
    }
    if b0 & 0xC0 == 0x80 {
        let b1 = cur.u8()?;
        return Some((((b0 & 0x3F) as usize) << 8) | b1 as usize);
    }
    if b0 & 0xE0 == 0xC0 {
        let rest = cur.take(3)?;
        return Some((((b0 & 0x1F) as usize) << 24) | ((rest[0] as usize) << 16) | ((rest[1] as usize) << 8) | rest[2] as usize);
    }
    None
}

/// **仅供测试**：造一个最小但结构完整的 PE32 + CLI 元数据程序集。
///
/// 供本模块与 `features::gallery` 的测试复用（真实 `Assembly-CSharp.dll`
/// 不便入库，用合成样本验证"程序集 → 候选键名"这条链路）。
#[cfg(test)]
pub(crate) mod tests_support {
    use super::BSJB;

    /// 参数：`strings` 进 `#Strings` 堆（类型/字段名），`user` 进 `#US` 堆（字面量）。
    ///
    /// 节表只有一节，RVA 基址 0x2000，文件偏移 0x200。
    pub fn minimal_pe(strings: &[&str], user: &[&str]) -> Vec<u8> {
        let mut blob = vec![0u8; 0x200];

        // ── #Strings 堆 ──
        let mut s_heap: Vec<u8> = vec![0];
        for s in strings {
            s_heap.extend_from_slice(s.as_bytes());
            s_heap.push(0);
        }

        // ── #US 堆 ──
        let mut u_heap: Vec<u8> = vec![0];
        for s in user {
            let mut raw: Vec<u8> = Vec::new();
            for u in s.encode_utf16() {
                raw.extend_from_slice(&u.to_le_bytes());
            }
            let body = raw.len() + 1;
            // 压缩整数编码
            if body < 0x80 {
                u_heap.push(body as u8);
            } else {
                u_heap.push(0x80 | ((body >> 8) as u8));
                u_heap.push((body & 0xFF) as u8);
            }
            u_heap.extend_from_slice(&raw);
            u_heap.push(0); // 尾随标志字节
        }

        // ── 元数据根 ──
        let ver = b"v4.0.30319\0";
        let mut md: Vec<u8> = Vec::new();
        md.extend_from_slice(&BSJB.to_le_bytes());
        md.extend_from_slice(&1u16.to_le_bytes()); // Major
        md.extend_from_slice(&1u16.to_le_bytes()); // Minor
        md.extend_from_slice(&0u32.to_le_bytes()); // Reserved
        md.extend_from_slice(&(ver.len() as u32).to_le_bytes());
        md.extend_from_slice(ver);
        while !md.len().is_multiple_of(4) {
            md.push(0);
        }
        let streams = 2u16;
        md.extend_from_slice(&0u16.to_le_bytes()); // Flags
        md.extend_from_slice(&streams.to_le_bytes());

        // 流头区长度：两个 (8 + 对齐名字)
        let hdr_strings_name = b"#Strings\0";
        let hdr_us_name = b"#US\0";
        let hdr_lens = 8 + hdr_strings_name.len().div_ceil(4) * 4 + 8 + hdr_us_name.len().div_ceil(4) * 4;
        let strings_off = md.len() + hdr_lens;
        let us_off = strings_off + s_heap.len();

        md.extend_from_slice(&(strings_off as u32).to_le_bytes());
        md.extend_from_slice(&(s_heap.len() as u32).to_le_bytes());
        md.extend_from_slice(hdr_strings_name);
        while !md.len().is_multiple_of(4) {
            md.push(0);
        }
        md.extend_from_slice(&(us_off as u32).to_le_bytes());
        md.extend_from_slice(&(u_heap.len() as u32).to_le_bytes());
        md.extend_from_slice(hdr_us_name);
        while !md.len().is_multiple_of(4) {
            md.push(0);
        }
        md.extend_from_slice(&s_heap);
        md.extend_from_slice(&u_heap);

        // ── 节内容（RVA 基址 0x2000 → 文件偏移 0x200）──
        const RVA_BASE: u32 = 0x2000;
        const RAW_BASE: usize = 0x200;
        let mut section: Vec<u8> = Vec::new();
        // CLI 头（偏移 0，RVA 0x2000）
        let cli_off = 0usize;
        let md_rva_placeholder_at = cli_off + 8;
        section.extend_from_slice(&72u32.to_le_bytes()); // cb
        section.extend_from_slice(&2u16.to_le_bytes()); // MajorRuntimeVersion
        section.extend_from_slice(&5u16.to_le_bytes()); // Minor
        section.extend_from_slice(&0u32.to_le_bytes()); // MetaData RVA（稍后回填）
        section.extend_from_slice(&(md.len() as u32).to_le_bytes());
        section.resize(72, 0);
        // 元数据根紧接 CLI 头，按 4 对齐
        while !section.len().is_multiple_of(4) {
            section.push(0);
        }
        let md_off_in_sec = section.len();
        section.extend_from_slice(&md);
        let md_rva = RVA_BASE + md_off_in_sec as u32;
        section[md_rva_placeholder_at..md_rva_placeholder_at + 4]
            .copy_from_slice(&md_rva.to_le_bytes());

        // ── PE 头 ──
        let pe_off = 0x80usize;
        let blob_len = RAW_BASE + section.len();
        blob.resize(blob_len, 0);
        blob[0..2].copy_from_slice(b"MZ");
        blob[0x3C..0x40].copy_from_slice(&(pe_off as u32).to_le_bytes());
        blob[pe_off..pe_off + 4].copy_from_slice(b"PE\0\0");
        let coff = pe_off + 4;
        blob[coff..coff + 2].copy_from_slice(&0x14cu16.to_le_bytes()); // Machine i386
        blob[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // NumberOfSections
        let size_opt = 0xE0usize;
        blob[coff + 16..coff + 18].copy_from_slice(&(size_opt as u16).to_le_bytes());
        let opt = coff + 20;
        blob[opt..opt + 2].copy_from_slice(&0x10bu16.to_le_bytes()); // PE32
        let num_dirs_off = opt + 92;
        blob[num_dirs_off..num_dirs_off + 4].copy_from_slice(&16u32.to_le_bytes());
        // 数据目录 14 = CLI 头
        let dir_off = opt + 96;
        let cli_dir = dir_off + 14 * 8;
        blob[cli_dir..cli_dir + 4].copy_from_slice(&RVA_BASE.to_le_bytes());
        blob[cli_dir + 4..cli_dir + 8].copy_from_slice(&72u32.to_le_bytes());
        // 节表（紧跟可选头）
        let sec = opt + size_opt;
        blob[sec..sec + 8].copy_from_slice(b".text\0\0\0");
        blob[sec + 8..sec + 12].copy_from_slice(&(section.len() as u32).to_le_bytes()); // VirtualSize
        blob[sec + 12..sec + 16].copy_from_slice(&RVA_BASE.to_le_bytes()); // VirtualAddress
        blob[sec + 16..sec + 20].copy_from_slice(&(section.len() as u32).to_le_bytes()); // SizeOfRawData
        blob[sec + 20..sec + 24].copy_from_slice(&(RAW_BASE as u32).to_le_bytes()); // PointerToRawData
        blob[RAW_BASE..RAW_BASE + section.len()].copy_from_slice(&section);
        blob
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::minimal_pe as build_minimal_pe;
    use super::*;

    #[test]
    fn reads_both_string_heaps() {
        let blob = build_minimal_pe(
            &["GalleryManager", "_wholeNameList", "CgColor", "PlayerPrefs", "Unlock"],
            &["CG_UnlockAll", "gallery_flag", "some_other_value"],
        );
        let got = parse_metadata(&blob).unwrap();
        assert!(got.names.contains(&"GalleryManager".to_string()));
        assert!(got.names.contains(&"_wholeNameList".to_string()));
        // #US 是 UTF-16LE，必须解对
        assert_eq!(got.user_strings, vec!["CG_UnlockAll", "gallery_flag", "some_other_value"]);
    }

    #[test]
    fn gallery_hits_are_conservative() {
        let blob = build_minimal_pe(
            &[
                "GalleryManager",
                "_wholeNameList",
                "CgColor",
                "CgProgram",
                "CgShader",
                "Camera",
                "cg_flag",
                "albumView",
                "CG01",
                "Name",
            ],
            &[],
        );
        let got = parse_metadata(&blob).unwrap();
        let hits = got.gallery_hits();
        for want in ["GalleryManager", "_wholeNameList", "cg_flag", "albumView", "CG01"] {
            assert!(hits.iter().any(|h| h == want), "应命中 {want}: {hits:?}");
        }
        // NVIDIA Cg 运行时类型 / 常见短名不该误报
        for bad in ["CgColor", "CgProgram", "CgShader", "Camera", "Name"] {
            assert!(!hits.iter().any(|h| h == bad), "不该误报 {bad}: {hits:?}");
        }
        assert!(got.has_gallery_type());
    }

    #[test]
    fn rejects_non_dotnet_and_truncated() {
        // 不是 PE
        assert!(parse_metadata(b"not a pe file at all....").is_err());
        // MZ 但没有 PE 签名
        let mut b = vec![0u8; 0x100];
        b[0..2].copy_from_slice(b"MZ");
        b[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        assert!(parse_metadata(&b).is_err());
        // 截断的合法 PE：逐长度切，任何长度都不许 panic
        let full = build_minimal_pe(&["GalleryManager"], &["k"]);
        for cut in 0..full.len() {
            let _ = parse_metadata(&full[..cut]);
        }
    }

    /// 变异模糊：对合法样本逐字节翻转 + 随机改写，绝不 panic。
    #[test]
    fn mutations_never_panic() {
        struct R(u64);
        impl R {
            fn next(&mut self) -> u64 {
                let mut x = self.0;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                self.0 = x;
                x
            }
        }

        let base = build_minimal_pe(
            &["GalleryManager", "_wholeNameList", "CgColor", "Camera"],
            &["cg_flag_01", "CG01", "回想シーン"],
        );
        // 逐字节翻 1 bit
        for i in 0..base.len() {
            let mut m = base.clone();
            m[i] ^= 1 << (i % 8);
            let _ = parse_metadata(&m);
        }
        // 随机改写若干字节（含 PE 头 / CLI 头 / 流头的关键字段）
        let mut r = R(0x2468_ace0_1357_9bdf);
        for _ in 0..500 {
            let mut m = base.clone();
            for _ in 0..6 {
                let pos = (r.next() as usize) % m.len();
                m[pos] = (r.next() & 0xFF) as u8;
            }
            let _ = parse_metadata(&m);
        }
    }

    #[test]
    fn compressed_int_roundtrip() {        let cases: Vec<(Vec<u8>, usize)> = vec![
            (vec![0x03], 3),
            (vec![0x7F], 0x7F),
            (vec![0x80, 0x80], 0x80),
            (vec![0xAE, 0x57], 0x2E57),
            (vec![0xBF, 0xFF], 0x3FFF),
            (vec![0xC0, 0x00, 0x40, 0x00], 0x4000),
            (vec![0xDF, 0xFF, 0xFF, 0xFF], 0x1FFF_FFFF),
        ];
        for (bytes, want) in cases {
            let mut cur = Cursor::new(&bytes);
            assert_eq!(read_compressed(&mut cur), Some(want), "{bytes:02x?}");
        }
        // 非法首字节（0xE0 及以上）→ None
        let mut cur = Cursor::new(&[0xF0u8, 0, 0, 0]);
        assert_eq!(read_compressed(&mut cur), None);
    }
}
