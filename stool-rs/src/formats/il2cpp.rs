//! Unity **IL2CPP 版** 的 `global-metadata.dat` 字符串读取（零依赖，只读）。
//!
//! Mono 版有 `*_Data/Managed/Assembly-CSharp.dll`（见 [`super::dotnet`]，
//! 读 `#US` / `#Strings` 堆）；**IL2CPP 版没有这个文件**——C# 已被转成
//! `GameAssembly.dll` 里的机器码，但**字符串字面量与类型/字段名仍完整保留**
//! 在 `*_Data/il2cpp_data/Metadata/global-metadata.dat`。
//!
//! 文件头布局（实测，见下）：
//!
//! ```text
//! @0   u32 magic = 0xFAB11BAF
//! @4   i32 version
//! @8   i32 stringLiteralOffset     / @12 i32 stringLiteralSize
//! @16  i32 stringLiteralDataOffset / @20 i32 stringLiteralDataSize
//! @24  i32 stringOffset            / @28 i32 stringSize
//! ```
//!
//! - `stringLiteral` 区 = `size / 8` 条 `{ i32 len; i32 dataIndex }`；
//! - `stringLiteralData` 区 = 字符串池，第 `i` 条字面量 =
//!   `pool[dataIndex .. dataIndex + len]`——**`len` 是精确字节长度，池内串首尾相接、
//!   没有 NUL 终止符**；
//! - `string` 区 = 类型 / 字段 / 方法名堆（NUL 分隔 UTF-8，开头通常是 `mscorlib`）。
//!
//! **实测依据**（6 个真机样本：v24 ×1、v29 ×1、v31 ×4）：
//! 三区**首尾严格相接**（`lit_off + lit_sz == dat_off`、`dat_off + dat_sz == str_off`），
//! `stringLiteral` 的 `dataIndex` 单调递增且**恰好按 `len` 步进**，
//! 按上式切出的字面量可解 UTF-8 比例 **99.99%**（16662/16663），样本里都是完整串
//! （如 `"    [{0}] {1} --> {2}"`、`"   Offset | Bytes  | Name     Layout: {0}"`），
//! `string` 区首串为 `mscorlib`。
//!
//! > 踩过的坑：一度按"`len` 含结尾 NUL"切 `len-1` 字节，结果所有串都被截掉末字符
//! > （`" - Linux"` → `" - Linu"`）。**没有 NUL 终止符**——真机样本才是判据。
//!
//! **只接受通过校验的输入**：magic、版本区间、三区首尾相接、区界不越文件，
//! 外加**字面量 UTF-8 可解率 ≥90%**（布局解读正确性的强自检）。任一不符即 `Err`，
//! 由调用方退回启发式扫描，绝不拿没把握的偏移硬读、更不产出乱码候选。
//!
//! 同样只读元数据、不反编译、不加载任何代码。

use std::path::Path;

use super::dotnet;
use super::safe;
use super::source::Source;

/// 元数据根签名。
const MAGIC: u32 = 0xFAB1_1BAF;
/// 支持的版本区间（区间外布局可能不同，宁可退回启发式）。
const MIN_VERSION: i32 = 24;
const MAX_VERSION: i32 = 31;
/// 单文件上限。
const MAX_METADATA: u64 = 512 * 1024 * 1024;

/// 从 `global-metadata.dat` 里读出的字符串。
#[derive(Debug, Default, Clone)]
pub struct Il2CppStrings {
    /// 字符串字面量（C# 里 `"xxx"` 的那部分）。
    pub literals: Vec<String>,
    /// 类型 / 字段 / 方法名。
    pub names: Vec<String>,
    /// 实际版本号（诊断用）。
    pub version: i32,
}

impl Il2CppStrings {
    /// 名字里"像全 CG 画廊"的标识（与 .NET 路径共用同一套判据）。
    pub fn gallery_hits(&self) -> Vec<String> {
        dotnet::gallery_hits_of(&self.names)
    }

    /// 是否识别到画廊相关类型。
    pub fn has_gallery_type(&self) -> bool {
        !self.gallery_hits().is_empty()
    }
}

/// 打开文件并读元数据字符串。
pub fn read_strings(path: &Path) -> Result<Il2CppStrings, String> {
    let mut src = Source::open(path, MAX_METADATA)?;
    let len = src.len();
    if len < 64 {
        return Err("文件过小，不是 IL2CPP 元数据".into());
    }
    let blob = src.read_exact_at(0, len as usize)?;
    parse_metadata(&blob)
}

/// 解析 `global-metadata.dat` 的字符串区。
pub fn parse_metadata(blob: &[u8]) -> Result<Il2CppStrings, String> {
    let file_len = blob.len();
    let magic = safe::u32_le(blob, 0).ok_or("文件过小")?;
    if magic != MAGIC {
        return Err(format!("不是 IL2CPP 元数据（magic 0x{magic:08X}）"));
    }
    let version = safe::i32_le(blob, 4).ok_or("文件过小")?;
    if !(MIN_VERSION..=MAX_VERSION).contains(&version) {
        return Err(format!(
            "IL2CPP 元数据版本 {version} 不在支持的 {MIN_VERSION}..={MAX_VERSION} 内"
        ));
    }

    let lit_off = safe::i32_le(blob, 8).ok_or("头部截断")?;
    let lit_sz = safe::i32_le(blob, 12).ok_or("头部截断")?;
    let dat_off = safe::i32_le(blob, 16).ok_or("头部截断")?;
    let dat_sz = safe::i32_le(blob, 20).ok_or("头部截断")?;
    let str_off = safe::i32_le(blob, 24).ok_or("头部截断")?;
    let str_sz = safe::i32_le(blob, 28).ok_or("头部截断")?;

    // ── 校验：全部非负、不越文件、三区首尾相接 ──
    // 首尾相接是"布局解读正确"的强信号（实测 6/6 样本成立）；不成立就绝不硬读。
    let bad = |what: &str| Err(format!("IL2CPP 元数据 {what} 校验不通过（布局与预期不符）"));
    if lit_off < 0 || lit_sz <= 0 || dat_off < 0 || dat_sz <= 0 || str_off < 0 || str_sz <= 0 {
        return bad("区偏移/大小");
    }
    let (lit_off, lit_sz, dat_off, dat_sz) = (lit_off as usize, lit_sz as usize, dat_off as usize, dat_sz as usize);
    let (str_off, str_sz) = (str_off as usize, str_sz as usize);
    if lit_sz % 8 != 0 {
        return bad("stringLiteral 区大小");
    }
    if lit_off + lit_sz != dat_off || dat_off + dat_sz != str_off {
        return bad("区连续性");
    }
    if str_off + str_sz > file_len {
        return bad("区界越界");
    }

    let Some(lit_heap) = safe::slice(blob, lit_off, lit_sz) else {
        return bad("stringLiteral 区");
    };
    let Some(pool) = safe::slice(blob, dat_off, dat_sz) else {
        return bad("stringLiteralData 区");
    };
    let Some(name_heap) = safe::slice(blob, str_off, str_sz) else {
        return bad("string 区");
    };

    let mut out = Il2CppStrings {
        version,
        literals: Vec::new(),
        names: dotnet::parse_strings_heap(name_heap),
    };
    let (literals, valid, total) = parse_literals(lit_heap, pool);
    // 布局自检：解读正确时字面量应几乎全部可解 UTF-8（实测 99.99%）。
    // 比例过低说明区指针或切片方式不对 —— 拒绝，由调用方退回启发式，
    // 绝不产出一堆乱码候选（这个自检正是"len 是否含 NUL"那类坑的拦网）。
    if total > 0 && valid * 10 < total * 9 {
        return Err(format!(
            "IL2CPP 元数据字面量区校验不通过（{valid}/{total} 可解 UTF-8，布局与预期不符）"
        ));
    }
    out.literals = literals;
    if out.literals.is_empty() && out.names.is_empty() {
        return Err("IL2CPP 元数据里没读到任何字符串".into());
    }
    Ok(out)
}

/// `stringLiteral` 区：`{ i32 len; i32 dataIndex }`，指向 `pool` 里的 UTF-8 串。
///
/// 返回 `(去重后的字面量, 可解 UTF-8 的条数, 参与统计的条数)`。
fn parse_literals(lit_heap: &[u8], pool: &[u8]) -> (Vec<String>, usize, usize) {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let (mut valid, mut total) = (0usize, 0usize);
    let n = lit_heap.len() / 8;
    let mut cur = safe::Cursor::new(lit_heap);
    for _ in 0..n {
        let (Some(len), Some(data_index)) = (cur.i32_le(), cur.i32_le()) else {
            break;
        };
        if len <= 0 || data_index < 0 {
            continue;
        }
        let (len, data_index) = (len as usize, data_index as usize);
        // 注意：`total` 在**切片前**累加 —— 越界/解码失败也要计入分母，
        // 否则"指针整体错位"这类问题会被漏掉（比值虚高），自检就白做了。
        total += 1;
        let Some(raw) = safe::slice(pool, data_index, len) else {
            continue;
        };
        let Ok(s) = std::str::from_utf8(raw) else {
            continue;
        };
        valid += 1;
        if s.is_empty() {
            continue;
        }
        if seen.insert(s.to_string()) {
            out.push(s.to_string());
        }
    }
    (out, valid, total)
}

/// **仅供测试**：造一个结构合法的 IL2CPP 元数据（三区首尾相接）。
#[cfg(test)]
pub(crate) mod tests_support {
    use super::MAGIC;

    /// `literals` 进字面量池，`names` 进名字堆。
    ///
    /// 与真实布局一致：池内字面量**首尾相接、无 NUL 终止符**，`len` = 精确字节长度。
    pub fn build_metadata(version: i32, literals: &[&str], names: &[&str]) -> Vec<u8> {
        // 字符串池 + (len, dataIndex) 表
        let mut pool: Vec<u8> = Vec::new();
        let mut table: Vec<u8> = Vec::new();
        for s in literals {
            let bytes = s.as_bytes();
            table.extend_from_slice(&(bytes.len() as i32).to_le_bytes());
            table.extend_from_slice(&(pool.len() as i32).to_le_bytes());
            pool.extend_from_slice(bytes);
        }

        let mut name_heap: Vec<u8> = vec![0]; // 首串为空（保留）
        for n in names {
            name_heap.extend_from_slice(n.as_bytes());
            name_heap.push(0);
        }

        const HDR: usize = 256;
        let lit_off = HDR;
        let lit_sz = table.len();
        let dat_off = lit_off + lit_sz;
        let dat_sz = pool.len();
        let str_off = dat_off + dat_sz;
        let str_sz = name_heap.len();

        let mut blob = vec![0u8; str_off + str_sz];
        blob[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        blob[4..8].copy_from_slice(&version.to_le_bytes());
        for (i, v) in [lit_off, lit_sz, dat_off, dat_sz, str_off, str_sz].iter().enumerate() {
            let at = 8 + i * 4;
            blob[at..at + 4].copy_from_slice(&(*v as i32).to_le_bytes());
        }
        blob[lit_off..lit_off + lit_sz].copy_from_slice(&table);
        blob[dat_off..dat_off + dat_sz].copy_from_slice(&pool);
        blob[str_off..str_off + str_sz].copy_from_slice(&name_heap);
        blob
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::build_metadata;
    use super::*;

    #[test]
    fn reads_literals_and_names() {
        let blob = build_metadata(
            29,
            &["cg_flag_01", "cg_flag_02", "not a key/with/slash"],
            &["mscorlib", "GalleryManager", "_wholeNameList", "CgColor"],
        );
        let got = parse_metadata(&blob).unwrap();
        assert_eq!(got.version, 29);
        assert_eq!(got.literals, vec!["cg_flag_01", "cg_flag_02", "not a key/with/slash"]);
        assert!(got.names.iter().any(|n| n == "mscorlib"));
        let hits = got.gallery_hits();
        assert!(hits.iter().any(|h| h == "GalleryManager"), "{hits:?}");
        assert!(hits.iter().any(|h| h == "_wholeNameList"), "{hits:?}");
        assert!(!hits.iter().any(|h| h == "CgColor"), "CgColor 不该误报: {hits:?}");
        assert!(got.has_gallery_type());
    }

    #[test]
    fn literals_are_back_to_back_without_nul() {
        // 关键回归：`len` 是精确字节长度、池内无 NUL 终止符。
        // 若误按"len 含 NUL"切 len-1 字节，这里会得到 "a"/"ab"/"abc" 被截尾的结果。
        let blob = build_metadata(31, &["a", "ab", "abc", " - Linux"], &[]);
        let got = parse_metadata(&blob).unwrap();
        assert_eq!(got.literals, vec!["a", "ab", "abc", " - Linux"]);
    }

    /// 布局自检：字面量指针/长度与池错位 → 必须拒绝（不允许产出乱码候选）。
    #[test]
    fn rejects_misaligned_pool() {
        let good = build_metadata(29, &["cg_flag_01", "cg_flag_02"], &["GalleryManager"]);
        let lit_off = safe::i32_le(&good, 8).unwrap() as usize;
        let lit_sz = safe::i32_le(&good, 12).unwrap() as usize;
        let pool_sz = safe::i32_le(&good, 20).unwrap();

        // 单条小错位不至于让比例跌破 90%，但绝不能 panic
        let mut b = good.clone();
        let len0 = safe::i32_le(&b, lit_off).unwrap();
        b[lit_off..lit_off + 4].copy_from_slice(&(len0 + 1).to_le_bytes());
        let _ = parse_metadata(&b);

        // 极端错位：把所有长度改成池大小 → 越界/错位，比例崩掉 → 自检必须拦下
        let mut b = good.clone();
        for i in 0..(lit_sz / 8) {
            let at = lit_off + i * 8;
            b[at..at + 4].copy_from_slice(&pool_sz.to_le_bytes());
        }
        assert!(parse_metadata(&b).is_err(), "极端错位必须被自检拦下");
    }

    #[test]
    fn rejects_bad_magic_version_and_layout() {
        // 用 2 条字面量（stringLiteral 区 = 16 字节），便于"减 8"制造连续性问题
        let good = build_metadata(29, &["xx", "yy"], &["z"]);

        // magic 不符
        let mut b = good.clone();
        b[0..4].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        assert!(parse_metadata(&b).unwrap_err().contains("magic"));

        // 版本超区间
        let mut b = good.clone();
        b[4..8].copy_from_slice(&16i32.to_le_bytes());
        assert!(parse_metadata(&b).unwrap_err().contains("版本"));

        // 破坏 8 字节对齐（stringLiteralSize - 4）
        let mut b = good.clone();
        let sz = safe::i32_le(&b, 12).unwrap();
        b[12..16].copy_from_slice(&(sz - 4).to_le_bytes());
        assert!(parse_metadata(&b).unwrap_err().contains("stringLiteral 区大小"));

        // 破坏区连续性（stringLiteralSize - 8，仍满足 8 字节对齐）
        let mut b = good.clone();
        b[12..16].copy_from_slice(&(sz - 8).to_le_bytes());
        assert!(parse_metadata(&b).unwrap_err().contains("连续性"));

        // 区界越界（把 stringSize 改得超出文件）
        let mut b = good.clone();
        let sz = safe::i32_le(&b, 28).unwrap();
        b[28..32].copy_from_slice(&(sz + 4096).to_le_bytes());
        assert!(parse_metadata(&b).unwrap_err().contains("越界"));

        // 空文件 / 超短文件
        assert!(parse_metadata(&[]).is_err());

        // 逐长度截断：任何长度都不许 panic
        for cut in 0..good.len() {
            let _ = parse_metadata(&good[..cut]);
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

        let base = build_metadata(
            29,
            &["cg_flag_01", "CG01", "回想シーン", "very very long literal value here"],
            &["mscorlib", "GalleryManager", "_wholeNameList"],
        );

        // 逐字节翻 1 bit
        for i in 0..base.len() {
            let mut m = base.clone();
            m[i] ^= 1 << (i % 8);
            let _ = parse_metadata(&m);
        }
        // 随机改写若干字节（含把区指针改成极端值）
        let mut r = R(0x1234_5678_9abc_def1);
        for _ in 0..500 {
            let mut m = base.clone();
            for _ in 0..6 {
                let pos = (r.next() as usize) % m.len();
                m[pos] = (r.next() & 0xFF) as u8;
            }
            let _ = parse_metadata(&m);
        }
    }
}
