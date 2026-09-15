//! 通用内存扫描修改器（Cheat Engine 替代，覆盖所有引擎）。
//!
//! 流程：选择进程 → 首次扫描（在全部可写内存中找值）→ 再次扫描
//! （精确 / 变了 / 没变 / 变大 / 变小 过滤缩小结果）→ 写入新值。
//! 支持 i32 / i64 / f32 / f64 / UTF-8 / UTF-16 字符串。
//!
//! 低层 API（打开进程 / 读写 / 查询区域 / 强制写入）在 [`crate::memapi`]，
//! 与 `features::guard`（反修改保护识别）共用。

use crate::memapi::{self, PageFix, Proc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanType {
    I32,
    I64,
    F32,
    F64,
    Utf8,
    Utf16,
}

impl ScanType {
    pub const ALL: [ScanType; 6] = [
        ScanType::I32,
        ScanType::I64,
        ScanType::F32,
        ScanType::F64,
        ScanType::Utf8,
        ScanType::Utf16,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            ScanType::I32 => "整数 32 位",
            ScanType::I64 => "整数 64 位",
            ScanType::F32 => "小数 32 位",
            ScanType::F64 => "小数 64 位",
            ScanType::Utf8 => "文本 UTF-8",
            ScanType::Utf16 => "文本 UTF-16",
        }
    }
    pub fn value_size(&self) -> usize {
        match self {
            ScanType::I32 | ScanType::F32 => 4,
            ScanType::I64 | ScanType::F64 => 8,
            ScanType::Utf8 | ScanType::Utf16 => 0,
        }
    }
    /// 供 CLI / GUI 解析 `--opt:type=`。
    pub fn parse(s: &str) -> Option<ScanType> {
        match s.trim().to_ascii_lowercase().as_str() {
            "i32" | "int" | "int32" | "整数" | "整数32" => Some(ScanType::I32),
            "i64" | "long" | "int64" | "整数64" => Some(ScanType::I64),
            "f32" | "float" | "小数32" => Some(ScanType::F32),
            "f64" | "double" | "小数64" => Some(ScanType::F64),
            "utf8" | "str" | "string" | "文本" => Some(ScanType::Utf8),
            "utf16" | "wstr" | "文本16" => Some(ScanType::Utf16),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScanValue {
    Int(i64),
    Float(f64),
    Str(String),
}

impl ScanValue {
    pub fn display(&self) -> String {
        match self {
            ScanValue::Int(i) => i.to_string(),
            ScanValue::Float(f) => format!("{f:.4}"),
            ScanValue::Str(s) => {
                // 控制字符替换掉，避免 GUI 显示异常
                s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Exact,
    Changed,
    Unchanged,
    Increased,
    Decreased,
}

impl Filter {
    pub const ALL: [Filter; 5] = [
        Filter::Exact,
        Filter::Changed,
        Filter::Unchanged,
        Filter::Increased,
        Filter::Decreased,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            Filter::Exact => "等于",
            Filter::Changed => "变化了",
            Filter::Unchanged => "没变",
            Filter::Increased => "变大了",
            Filter::Decreased => "变小了",
        }
    }
}

/// 一条扫描命中：内存地址 + 当时读到的值。
#[derive(Debug, Clone)]
pub struct Hit {
    pub addr: usize,
    pub value: ScanValue,
}

/// 单个目标进程的扫描会话（持有进程句柄）。
pub struct Scanner {
    proc: Option<Proc>,
    pub ty: ScanType,
    pub hits: Vec<Hit>,
    pub first_done: bool,
    /// 写入前保存的原始字节（用于撤销）。
    pub saved: Vec<(usize, Vec<u8>)>,
}

const MAX_HITS: usize = 2_000_000;
pub const MAX_REGION: usize = 256 * 1024 * 1024;

impl Scanner {
    /// 打开进程（查询 + 读 + 写内存权限）。失败常见原因：权限不足（试试管理员运行）。
    pub fn open(pid: u32, ty: ScanType) -> Result<Scanner, String> {
        let proc = Proc::open(pid)?;
        Ok(Scanner { proc: Some(proc), ty, hits: Vec::new(), first_done: false, saved: Vec::new() })
    }

    pub fn pid(&self) -> u32 {
        self.proc.as_ref().map(|p| p.pid()).unwrap_or(0)
    }

    fn proc(&self) -> Result<&Proc, String> {
        self.proc.as_ref().ok_or_else(|| "进程句柄已失效，请重新打开".to_string())
    }

    /// 首次扫描全部可写内存。返回命中数量。
    pub fn first_scan(&mut self, input: &str) -> Result<usize, String> {
        let needle = self.parse_value(input)?;
        let bytes = self.pattern_bytes(&needle).ok_or("该类型需要输入有效的值")?;
        let mut hits = Vec::new();
        for (base, buf) in self.writable_regions()? {
            scan_bytes(&buf, base, &bytes, self.ty, &needle, &mut hits);
            if hits.len() >= MAX_HITS {
                break;
            }
        }
        self.hits = hits;
        self.first_done = true;
        Ok(self.hits.len())
    }

    /// 再次扫描：对当前命中地址重新读内存并过滤。返回剩余数量。
    pub fn next_scan(&mut self, input: &str, filter: Filter) -> Result<usize, String> {
        if !self.first_done {
            return Err("请先执行首次扫描".into());
        }
        let new_val = if filter == Filter::Exact {
            Some(self.parse_value(input)?)
        } else {
            None
        };
        let vsz = self.ty.value_size().max(1);
        let h = self.proc()?.raw();
        self.hits.sort_by_key(|h| h.addr);
        let mut kept: Vec<Hit> = Vec::with_capacity(self.hits.len());
        let mut i = 0;
        while i < self.hits.len() {
            let start = self.hits[i].addr;
            // 找一段近似连续区间（相邻间隔 < 1MB 批量读取，减少系统调用）
            let mut j = i;
            while j + 1 < self.hits.len() && self.hits[j + 1].addr - self.hits[j].addr < 1024 * 1024 {
                j += 1;
            }
            let last = self.hits[j].addr;
            let len = (last - start + vsz.max(256)).min(MAX_REGION);
            if let Some(buf) = memapi::read_raw(h, start, len) {
                for hit in &self.hits[i..=j] {
                    let off = hit.addr - start;
                    if off + vsz > buf.len() {
                        continue;
                    }
                    let Some(cur) = read_value_at(&buf[off..], self.ty) else { continue };
                    let keep = match filter {
                        Filter::Exact => Some(&cur) == new_val.as_ref(),
                        Filter::Changed => cur != hit.value,
                        Filter::Unchanged => cur == hit.value,
                        Filter::Increased => value_gt(&cur, &hit.value),
                        Filter::Decreased => value_gt(&hit.value, &cur),
                    };
                    if keep {
                        kept.push(Hit { addr: hit.addr, value: cur });
                    }
                }
            }
            i = j + 1;
        }
        self.hits = kept;
        Ok(self.hits.len())
    }

    /// 把新值写入所有命中地址（或指定单个地址，addr=None 表示全部）。
    /// 写入前记录原始值，可用 undo() 撤销。
    pub fn write(&mut self, input: &str, addr: Option<usize>) -> Result<usize, String> {
        if !self.first_done {
            return Err("请先执行扫描".into());
        }
        let needle = self.parse_value(input)?;
        let bytes = self.pattern_bytes(&needle).ok_or("该类型需要输入有效的值")?;
        let targets: Vec<usize> = match addr {
            Some(a) => vec![a],
            None => self.hits.iter().map(|h| h.addr).collect(),
        };
        let h = self.proc()?.raw();
        let mut n = 0;
        let mut saved = std::mem::take(&mut self.saved);
        for a in targets {
            // 只在首次写入某地址时记录原值，避免重复记录
            if !saved.iter().any(|(sa, _)| *sa == a) {
                if let Some(old) = memapi::read_raw(h, a, bytes.len()) {
                    saved.push((a, old));
                }
            }
            if memapi::write_raw(h, a, &bytes) {
                n += 1;
            }
        }
        self.saved = saved;
        if n == 0 {
            return Err("写入失败（地址可能已失效，请重新扫描）".into());
        }
        Ok(n)
    }

    /// 直接写入（不记录原值），供数值锁定循环高频调用。
    pub fn write_raw(&self, input: &str, addr: Option<usize>) -> Result<usize, String> {
        if !self.first_done {
            return Err("请先执行扫描".into());
        }
        let needle = self.parse_value(input)?;
        let bytes = self.pattern_bytes(&needle).ok_or("该类型需要输入有效的值")?;
        let h = self.proc()?.raw();
        let targets: Vec<usize> = match addr {
            Some(a) => vec![a],
            None => self.hits.iter().map(|h| h.addr).collect(),
        };
        let mut n = 0;
        for a in targets {
            if memapi::write_raw(h, a, &bytes) {
                n += 1;
            }
        }
        if n == 0 {
            return Err("写入失败（地址可能已失效，请重新扫描）".into());
        }
        Ok(n)
    }

    /// 强制写入：页只读 / Guard 导致普通写入失败时，临时解除页保护再写。
    ///
    /// 返回成功写入的地址数 + 首个成功地址用了哪种页修复（用于给用户解释）。
    pub fn force_write(&mut self, input: &str, addr: Option<usize>) -> Result<(usize, PageFix), String> {
        if !self.first_done {
            return Err("请先执行扫描".into());
        }
        let needle = self.parse_value(input)?;
        let bytes = self.pattern_bytes(&needle).ok_or("该类型需要输入有效的值")?;
        let targets: Vec<usize> = match addr {
            Some(a) => vec![a],
            None => self.hits.iter().map(|h| h.addr).collect(),
        };
        let h = self.proc()?.raw();
        let mut n = 0usize;
        let mut fix = PageFix::NotNeeded;
        let mut last_err: Option<String> = None;
        let mut saved = std::mem::take(&mut self.saved);
        for a in targets {
            if !saved.iter().any(|(sa, _)| *sa == a) {
                if let Some(old) = memapi::read_raw(h, a, bytes.len()) {
                    saved.push((a, old));
                }
            }
            match memapi::force_write_raw(h, a, &bytes) {
                Ok(f) => {
                    n += 1;
                    if f != PageFix::NotNeeded {
                        fix = f;
                    }
                }
                Err(e) => last_err = Some(e),
            }
        }
        self.saved = saved;
        match (n, last_err) {
            (0, Some(e)) => Err(e),
            (0, None) => Err("写入失败（地址可能已失效，请重新扫描）".into()),
            _ => Ok((n, fix)),
        }
    }

    /// 撤销写入：把所有记录过的地址恢复为写入前的值。返回恢复的数量。
    pub fn undo(&mut self) -> usize {
        let Some(h) = self.proc.as_ref().map(|p| p.raw()) else { return 0 };
        let mut n = 0;
        for (a, old) in self.saved.drain(..) {
            if memapi::write_raw(h, a, &old) {
                n += 1;
            }
        }
        n
    }

    /// 是否有可撤销的写入记录。
    pub fn has_saved(&self) -> bool {
        !self.saved.is_empty()
    }

    /// 重新读取当前命中的最新值（刷新显示）。
    pub fn refresh(&mut self) {
        let vsz = self.ty.value_size().max(128);
        let Some(h) = self.proc.as_ref().map(|p| p.raw()) else { return };
        for hit in self.hits.iter_mut() {
            if let Some(buf) = memapi::read_raw(h, hit.addr, vsz) {
                if let Some(v) = read_value_at(&buf, self.ty) {
                    hit.value = v;
                }
            }
        }
    }

    pub fn hit_count(&self) -> usize {
        self.hits.len()
    }

    /// 当前命中里，有多少个地址所在页**不能直接写**（只读 / Guard）——
    /// 这类地址用「写入所有命中」会失败，需要「强制写入」。
    pub fn readonly_hit_count(&self) -> usize {
        let Some(h) = self.proc.as_ref().map(|p| p.raw()) else { return 0 };
        self.hits
            .iter()
            .filter(|hit| {
                memapi::region_raw(h, hit.addr)
                    .map(|r| !memapi::is_directly_writable(r.protect))
                    .unwrap_or(false)
            })
            .count()
    }

    fn parse_value(&self, input: &str) -> Result<ScanValue, String> {
        parse_value(self.ty, input)
    }

    fn pattern_bytes(&self, v: &ScanValue) -> Option<Vec<u8>> {
        pattern_bytes_of(self.ty, v)
    }

    /// 枚举全部可写已提交区域（VirtualQueryEx），逐块读出。
    fn writable_regions(&self) -> Result<Vec<(usize, Vec<u8>)>, String> {
        let proc = self.proc()?;
        let mut out = Vec::new();
        for reg in proc.regions() {
            if !reg.is_scannable_writable(MAX_REGION) {
                continue;
            }
            if let Some(buf) = proc.read(reg.base, reg.size) {
                out.push((reg.base, buf));
            }
        }
        Ok(out)
    }
}

/// 把值编码成内存里的字节（供扫描与写入共用）。
pub fn pattern_bytes_of(ty: ScanType, v: &ScanValue) -> Option<Vec<u8>> {
    match (ty, v) {
        (ScanType::I32, ScanValue::Int(i)) => Some((*i as i32).to_le_bytes().to_vec()),
        (ScanType::I64, ScanValue::Int(i)) => Some(i.to_le_bytes().to_vec()),
        (ScanType::F32, ScanValue::Float(f)) => Some((*f as f32).to_le_bytes().to_vec()),
        (ScanType::F64, ScanValue::Float(f)) => Some(f.to_le_bytes().to_vec()),
        (ScanType::Utf8, ScanValue::Str(s)) => Some(s.as_bytes().to_vec()),
        (ScanType::Utf16, ScanValue::Str(s)) => Some(s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()),
        _ => None,
    }
}

/// 按数值类型解析用户输入的字符串（供 CLI / GUI / guard 共用同一套报错文案）。
pub fn parse_value(ty: ScanType, input: &str) -> Result<ScanValue, String> {
    let s = input.trim();
    match ty {
        ScanType::I32 => s.parse::<i32>().map(|v| ScanValue::Int(v as i64)).map_err(|e| format!("需要 32 位整数: {e}")),
        ScanType::I64 => s.parse::<i64>().map(ScanValue::Int).map_err(|e| format!("需要 64 位整数: {e}")),
        ScanType::F32 => s.parse::<f32>().map(|v| ScanValue::Float(v as f64)).map_err(|e| format!("需要小数: {e}")),
        ScanType::F64 => s.parse::<f64>().map(ScanValue::Float).map_err(|e| format!("需要小数: {e}")),
        ScanType::Utf8 | ScanType::Utf16 => Ok(ScanValue::Str(s.to_string())),
    }
}

/// 该类型是否为可做「数值保护探测」的数值类型（文本类型没有 ± 语义，探测不适用）。
pub fn is_numeric(ty: ScanType) -> bool {
    matches!(ty, ScanType::I32 | ScanType::I64 | ScanType::F32 | ScanType::F64)
}

/// 自动构造一个探测值：在 `cur` 基础上往「明显不同」的方向推。
/// 整数：< 1000 时 +1000，否则 +7；小数：×2 + 1。文本类型返回 None。
pub fn auto_probe(ty: ScanType, cur: &ScanValue) -> Option<ScanValue> {
    match (ty, cur) {
        (ScanType::I32 | ScanType::I64, ScanValue::Int(v)) => {
            let d = if v.abs() < 1000 { 1000 } else { 7 };
            Some(ScanValue::Int(v.saturating_add(d)))
        }
        (ScanType::F32 | ScanType::F64, ScanValue::Float(f)) => Some(ScanValue::Float(f * 2.0 + 1.0)),
        _ => None,
    }
}

/// 把值编码成定长 8 字节小端（高位补 0），用于位运算层面的差分判定（XOR / 位移）。
pub fn pattern_u64(ty: ScanType, v: &ScanValue) -> Option<u64> {
    let b = pattern_bytes_of(ty, v)?;
    if b.is_empty() || b.len() > 8 {
        return None;
    }
    let mut buf = [0u8; 8];
    buf[..b.len()].copy_from_slice(&b);
    Some(u64::from_le_bytes(buf))
}

/// 把内存里读到的字节补成定长 8 字节小端（高位补 0），与 [`pattern_u64`] 配对使用。
pub fn bytes_u64(b: &[u8]) -> Option<u64> {
    if b.is_empty() || b.len() > 8 {
        return None;
    }
    let mut buf = [0u8; 8];
    buf[..b.len()].copy_from_slice(b);
    Some(u64::from_le_bytes(buf))
}

fn value_gt(a: &ScanValue, b: &ScanValue) -> bool {
    match (a, b) {
        (ScanValue::Int(x), ScanValue::Int(y)) => x > y,
        (ScanValue::Float(x), ScanValue::Float(y)) => x > y,
        _ => false,
    }
}

/// 在一块内存里找模式字节。命中即记录。
fn scan_bytes(buf: &[u8], base: usize, needle: &[u8], ty: ScanType, val: &ScanValue, out: &mut Vec<Hit>) {
    if needle.is_empty() || needle.len() > buf.len() {
        return;
    }
    let first = needle[0];
    for i in 0..=(buf.len() - needle.len()) {
        if buf[i] == first && &buf[i..i + needle.len()] == needle {
            let v = match ty {
                ScanType::Utf8 => ScanValue::Str(String::from_utf8_lossy(&buf[i..i + needle.len()]).into_owned()),
                ScanType::Utf16 => ScanValue::Str(decode_utf16(&buf[i..i + needle.len()])),
                _ => val.clone(),
            };
            out.push(Hit { addr: base + i, value: v });
            if out.len() >= MAX_HITS {
                return;
            }
        }
    }
}

fn decode_utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

/// 从内存片段读一个值（失败返回 None）。
pub fn read_value_at(buf: &[u8], ty: ScanType) -> Option<ScanValue> {
    match ty {
        ScanType::I32 if buf.len() >= 4 => Some(ScanValue::Int(i32::from_le_bytes(buf[..4].try_into().ok()?) as i64)),
        ScanType::I64 if buf.len() >= 8 => Some(ScanValue::Int(i64::from_le_bytes(buf[..8].try_into().ok()?))),
        ScanType::F32 if buf.len() >= 4 => Some(ScanValue::Float(f32::from_le_bytes(buf[..4].try_into().ok()?) as f64)),
        ScanType::F64 if buf.len() >= 8 => Some(ScanValue::Float(f64::from_le_bytes(buf[..8].try_into().ok()?))),
        ScanType::Utf8 => Some(ScanValue::Str(String::from_utf8_lossy(buf).into_owned())),
        ScanType::Utf16 => Some(ScanValue::Str(decode_utf16(buf))),
        _ => None,
    }
}

/// 列出系统进程 [(pid, 名字)]，按 pid 排序。
pub fn list_processes() -> Vec<(u32, String)> {
    memapi::list_processes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_type_parse_covers_aliases() {
        assert_eq!(ScanType::parse("i32"), Some(ScanType::I32));
        assert_eq!(ScanType::parse("INT"), Some(ScanType::I32));
        assert_eq!(ScanType::parse(" float "), Some(ScanType::F32));
        assert_eq!(ScanType::parse("double"), Some(ScanType::F64));
        assert_eq!(ScanType::parse("utf-16"), None);
        assert_eq!(ScanType::parse("utf16"), Some(ScanType::Utf16));
        assert_eq!(ScanType::parse("nope"), None);
    }

    #[test]
    fn pattern_bytes_roundtrip_for_numeric_types() {
        let b = pattern_bytes_of(ScanType::I32, &ScanValue::Int(100)).unwrap();
        assert_eq!(b, vec![100, 0, 0, 0]);
        assert_eq!(read_value_at(&b, ScanType::I32), Some(ScanValue::Int(100)));

        let b = pattern_bytes_of(ScanType::F32, &ScanValue::Float(1.5)).unwrap();
        assert_eq!(read_value_at(&b, ScanType::F32), Some(ScanValue::Float(1.5)));

        let b = pattern_bytes_of(ScanType::Utf16, &ScanValue::Str("ab".into())).unwrap();
        assert_eq!(b, vec![b'a', 0, b'b', 0]);
    }

    #[test]
    fn scan_bytes_finds_all_occurrences_and_respects_bounds() {
        let buf = vec![0u8, 100, 0, 0, 0, 7, 100, 0, 0, 0, 9];
        let needle = (100i32).to_le_bytes().to_vec();
        let mut out = Vec::new();
        scan_bytes(&buf, 0x1000, &needle, ScanType::I32, &ScanValue::Int(100), &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].addr, 0x1001);
        assert_eq!(out[1].addr, 0x1006);

        // 空模式 / 过长模式不 panic 且不产生命中
        let mut empty = Vec::new();
        scan_bytes(&buf, 0, &[], ScanType::I32, &ScanValue::Int(0), &mut empty);
        assert!(empty.is_empty());
        let mut too_long = Vec::new();
        scan_bytes(&[1u8, 2], 0, &[1, 2, 3], ScanType::I32, &ScanValue::Int(0), &mut too_long);
        assert!(too_long.is_empty());
    }

    #[test]
    fn read_value_at_rejects_short_slices_without_panic() {
        assert_eq!(read_value_at(&[1u8, 2, 3], ScanType::I32), None);
        assert_eq!(read_value_at(&[], ScanType::I64), None);
        assert_eq!(read_value_at(&[1u8], ScanType::Utf8), Some(ScanValue::Str("\u{1}".into())));
    }

    #[test]
    fn utf16_decoding_is_lossy_but_bounded() {
        assert_eq!(decode_utf16(&[b'a', 0, b'b', 0]), "ab");
        // 奇数长度：as_chunks 只取成对的部分，不 panic
        assert_eq!(decode_utf16(&[b'a', 0, b'b']), "a");
        assert_eq!(decode_utf16(&[]), "");
    }
}
