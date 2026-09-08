//! 通用内存扫描修改器（Cheat Engine 替代，覆盖所有引擎）。
//!
//! 流程：选择进程 → 首次扫描（在全部可写内存中找值）→ 再次扫描
//! （精确 / 变了 / 没变 / 变大 / 变小 过滤缩小结果）→ 写入新值。
//! 支持 i32 / i64 / f32 / f64 / UTF-8 / UTF-16 字符串。

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
    handle: isize,
    pub ty: ScanType,
    pub hits: Vec<Hit>,
    pub first_done: bool,
    /// 写入前保存的原始字节（用于撤销）。
    pub saved: Vec<(usize, Vec<u8>)>,
}

const MAX_HITS: usize = 2_000_000;
const MAX_REGION: usize = 256 * 1024 * 1024;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_WRITECOPY: u32 = 0x08;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
const PAGE_GUARD: u32 = 0x100;
const MEM_COMMIT_STATE: u32 = 0x1000;

impl Drop for Scanner {
    fn drop(&mut self) {
        if self.handle != 0 {
            w_close(self.handle);
        }
    }
}

impl Scanner {
    /// 打开进程（查询 + 读 + 写内存权限）。失败常见原因：权限不足（试试管理员运行）。
    pub fn open(pid: u32, ty: ScanType) -> Result<Scanner, String> {
        let handle = w_open_process(0x0438, pid); // VM_OP|VM_READ|VM_WRITE|QUERY
        if handle == 0 {
            return Err("无法打开进程（权限不足或进程已退出）。可尝试以管理员身份运行 STool。".into());
        }
        Ok(Scanner { handle, ty, hits: Vec::new(), first_done: false, saved: Vec::new() })
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
            if let Some(buf) = w_read(self.handle, start, len) {
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
        let mut n = 0;
        for a in targets {
            // 只在首次写入某地址时记录原值，避免重复记录
            if !self.saved.iter().any(|(sa, _)| *sa == a) {
                if let Some(old) = w_read(self.handle, a, bytes.len()) {
                    self.saved.push((a, old));
                }
            }
            if w_write(self.handle, a, &bytes) {
                n += 1;
            }
        }
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
        let targets: Vec<usize> = match addr {
            Some(a) => vec![a],
            None => self.hits.iter().map(|h| h.addr).collect(),
        };
        let mut n = 0;
        for a in targets {
            if w_write(self.handle, a, &bytes) {
                n += 1;
            }
        }
        if n == 0 {
            return Err("写入失败（地址可能已失效，请重新扫描）".into());
        }
        Ok(n)
    }

    /// 撤销写入：把所有记录过的地址恢复为写入前的值。返回恢复的数量。
    pub fn undo(&mut self) -> usize {
        let mut n = 0;
        for (a, old) in self.saved.drain(..) {
            if w_write(self.handle, a, &old) {
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
        for hit in self.hits.iter_mut() {
            if let Some(buf) = w_read(self.handle, hit.addr, vsz) {
                if let Some(v) = read_value_at(&buf, self.ty) {
                    hit.value = v;
                }
            }
        }
    }

    pub fn hit_count(&self) -> usize {
        self.hits.len()
    }

    fn parse_value(&self, input: &str) -> Result<ScanValue, String> {
        let s = input.trim();
        match self.ty {
            ScanType::I32 => s.parse::<i32>().map(|v| ScanValue::Int(v as i64)).map_err(|e| format!("需要 32 位整数: {e}")),
            ScanType::I64 => s.parse::<i64>().map(ScanValue::Int).map_err(|e| format!("需要 64 位整数: {e}")),
            ScanType::F32 => s.parse::<f32>().map(|v| ScanValue::Float(v as f64)).map_err(|e| format!("需要小数: {e}")),
            ScanType::F64 => s.parse::<f64>().map(ScanValue::Float).map_err(|e| format!("需要小数: {e}")),
            ScanType::Utf8 | ScanType::Utf16 => Ok(ScanValue::Str(s.to_string())),
        }
    }

    fn pattern_bytes(&self, v: &ScanValue) -> Option<Vec<u8>> {
        match (self.ty, v) {
            (ScanType::I32, ScanValue::Int(i)) => Some((*i as i32).to_le_bytes().to_vec()),
            (ScanType::I64, ScanValue::Int(i)) => Some(i.to_le_bytes().to_vec()),
            (ScanType::F32, ScanValue::Float(f)) => Some((*f as f32).to_le_bytes().to_vec()),
            (ScanType::F64, ScanValue::Float(f)) => Some(f.to_le_bytes().to_vec()),
            (ScanType::Utf8, ScanValue::Str(s)) => Some(s.as_bytes().to_vec()),
            (ScanType::Utf16, ScanValue::Str(s)) => Some(s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()),
            _ => None,
        }
    }

    /// 枚举全部可写已提交区域（VirtualQueryEx），逐块读出。
    fn writable_regions(&self) -> Result<Vec<(usize, Vec<u8>)>, String> {
        let mut out = Vec::new();
        let mut addr: usize = 0x10000;
        let max_addr = 0x7FFF_FFFF_FFFF;
        while addr < max_addr {
            let Some(mbi) = w_query(self.handle, addr) else { break };
            let region_size = mbi.1.max(1);
            let base = mbi.0;
            let prot = mbi.2;
            let writable = matches!(prot & 0xFF, PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY);
            let committed = mbi.3 == MEM_COMMIT_STATE;
            if committed && writable && prot & PAGE_GUARD == 0 && region_size <= MAX_REGION {
                if let Some(buf) = w_read(self.handle, base, region_size) {
                    out.push((base, buf));
                }
            }
            match base.checked_add(region_size) {
                Some(next) if next > addr => addr = next,
                _ => break,
            }
        }
        Ok(out)
    }
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
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

/// 从内存片段读一个值（失败返回 None）。
fn read_value_at(buf: &[u8], ty: ScanType) -> Option<ScanValue> {
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
    w_enum_processes()
}

// ---------------------------------------------------------------------------
// Windows API 薄封装（非 Windows 返回失败，保证跨平台编译）
// ---------------------------------------------------------------------------

/// (BaseAddress, RegionSize, Protect, State)
type MemInfo = (usize, usize, u32, u32);

#[cfg(windows)]
mod winapi {
    use super::MemInfo;
    use windows_sys::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION};
    use windows_sys::Win32::System::Threading::OpenProcess;

    pub fn open_process(access: u32, pid: u32) -> isize {
        unsafe { OpenProcess(access, 0, pid) as isize }
    }

    pub fn close_handle(h: isize) {
        unsafe { CloseHandle(h as HANDLE) };
    }

    pub fn query_mem(h: isize, addr: usize) -> Option<MemInfo> {
        unsafe {
            let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            let n = VirtualQueryEx(
                h as HANDLE,
                addr as *const core::ffi::c_void,
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            );
            if n == 0 {
                None
            } else {
                Some((mbi.BaseAddress as usize, mbi.RegionSize, mbi.Protect, mbi.State))
            }
        }
    }

    pub fn read_mem(h: isize, addr: usize, size: usize) -> Option<Vec<u8>> {
        unsafe {
            let mut buf = vec![0u8; size];
            let mut read = 0usize;
            let ok = ReadProcessMemory(h as HANDLE, addr as *const core::ffi::c_void, buf.as_mut_ptr() as *mut core::ffi::c_void, size, &mut read);
            if ok != 0 && read > 0 { buf.truncate(read); Some(buf) } else { None }
        }
    }

    pub fn write_mem(h: isize, addr: usize, bytes: &[u8]) -> bool {
        unsafe {
            let mut written = 0usize;
            WriteProcessMemory(h as HANDLE, addr as *const core::ffi::c_void, bytes.as_ptr() as *const core::ffi::c_void, bytes.len(), &mut written) != 0
        }
    }

    pub fn enum_processes() -> Vec<(u32, String)> {
        unsafe {
            let mut out = Vec::new();
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap as isize == -1 {
                return out;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut entry) != 0 {
                loop {
                    let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                    out.push((entry.th32ProcessID, String::from_utf16_lossy(&entry.szExeFile[..len])));
                    if Process32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
            out.sort();
            out
        }
    }
}

#[cfg(windows)]
use winapi as w;

#[cfg(windows)]
fn w_open_process(access: u32, pid: u32) -> isize {
    w::open_process(access, pid)
}
#[cfg(windows)]
fn w_close(h: isize) {
    w::close_handle(h)
}
#[cfg(windows)]
fn w_query(h: isize, addr: usize) -> Option<MemInfo> {
    w::query_mem(h, addr)
}
#[cfg(windows)]
fn w_read(h: isize, addr: usize, size: usize) -> Option<Vec<u8>> {
    w::read_mem(h, addr, size)
}
#[cfg(windows)]
fn w_write(h: isize, addr: usize, bytes: &[u8]) -> bool {
    w::write_mem(h, addr, bytes)
}
#[cfg(windows)]
fn w_enum_processes() -> Vec<(u32, String)> {
    w::enum_processes()
}

#[cfg(not(windows))]
fn w_open_process(_access: u32, _pid: u32) -> isize {
    0
}
#[cfg(not(windows))]
fn w_close(_h: isize) {}
#[cfg(not(windows))]
fn w_query(_h: isize, _addr: usize) -> Option<MemInfo> {
    None
}
#[cfg(not(windows))]
fn w_read(_h: isize, _addr: usize, _size: usize) -> Option<Vec<u8>> {
    None
}
#[cfg(not(windows))]
fn w_write(_h: isize, _addr: usize, _bytes: &[u8]) -> bool {
    false
}
#[cfg(not(windows))]
fn w_enum_processes() -> Vec<(u32, String)> {
    Vec::new()
}
