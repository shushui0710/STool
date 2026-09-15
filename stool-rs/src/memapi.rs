//! 跨进程内存 / 模块访问的 Windows API 薄封装（非 Windows 上是安全桩，保证跨平台编译）。
//!
//! 从 `features/memscan.rs` 抽出来，供内存扫描（`memscan`）与保护诊断（`features::guard`）共用：
//! 打开 / 关闭进程、读写内存、查询区域、强制写入（临时解除页保护）、枚举模块、枚举进程。
//!
//! 约定：所有函数失败都返回 `Option` / `Result`，**不 panic**；句柄用 RAII（`Proc` 在 Drop 里关闭）。

// ---------------------------------------------------------------------------
// 页保护 / 内存类型常量（与 Win32 定义一致，避免依赖具体 windows-sys 版本导出）
// ---------------------------------------------------------------------------

pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READONLY: u32 = 0x02;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_WRITECOPY: u32 = 0x08;
pub const PAGE_EXECUTE: u32 = 0x10;
pub const PAGE_EXECUTE_READ: u32 = 0x20;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
pub const PAGE_GUARD: u32 = 0x100;
pub const PAGE_NOCACHE: u32 = 0x200;
pub const PAGE_WRITECOMBINE: u32 = 0x400;

pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_FREE: u32 = 0x1_0000;
pub const MEM_PRIVATE: u32 = 0x2_0000;
pub const MEM_MAPPED: u32 = 0x4_0000;
pub const MEM_IMAGE: u32 = 0x0100_0000;

/// PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_QUERY_INFORMATION
pub const ACCESS_RW_QUERY: u32 = 0x0438;
/// PROCESS_VM_READ | PROCESS_QUERY_INFORMATION —— 只读诊断用（权限要求更低，成功率高）
pub const ACCESS_QUERY: u32 = 0x0410;

/// 页保护的「基类型」（去掉 PAGE_GUARD / PAGE_NOCACHE 等修饰位）。
pub fn protect_base(protect: u32) -> u32 {
    protect & 0xFF
}

pub fn protect_name(protect: u32) -> &'static str {
    match protect_base(protect) {
        PAGE_NOACCESS => "不可访问",
        PAGE_READONLY => "只读",
        PAGE_READWRITE => "可读写",
        PAGE_WRITECOPY => "写时复制",
        PAGE_EXECUTE => "仅执行",
        PAGE_EXECUTE_READ => "执行+读",
        PAGE_EXECUTE_READWRITE => "执行+读写",
        PAGE_EXECUTE_WRITECOPY => "执行+写时复制",
        _ => "未知",
    }
}

/// 页保护是否允许直接写入（WriteProcessMemory 会成功）。
pub fn is_directly_writable(protect: u32) -> bool {
    matches!(
        protect_base(protect),
        PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
    ) && protect & PAGE_GUARD == 0
}

pub fn is_executable(protect: u32) -> bool {
    matches!(
        protect_base(protect),
        PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
    )
}

pub fn state_name(state: u32) -> &'static str {
    match state {
        MEM_COMMIT => "已提交",
        MEM_RESERVE => "已保留",
        MEM_FREE => "空闲",
        _ => "未知",
    }
}

pub fn kind_name(kind: u32) -> &'static str {
    match kind {
        MEM_IMAGE => "映像(文件映射)",
        MEM_MAPPED => "映射(共享内存)",
        MEM_PRIVATE => "私有",
        _ => "未知",
    }
}

// ---------------------------------------------------------------------------
// 数据结构
// ---------------------------------------------------------------------------

/// (BaseAddress, RegionSize, Protect, State) —— 与 memscan 原有类型保持一致。
pub type MemInfo = (usize, usize, u32, u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub base: usize,
    pub size: usize,
    pub protect: u32,
    pub state: u32,
    pub kind: u32,
}

impl Region {
    pub fn end(&self) -> usize {
        self.base.saturating_add(self.size)
    }
    pub fn protect_name(&self) -> &'static str {
        protect_name(self.protect)
    }
    pub fn kind_name(&self) -> &'static str {
        kind_name(self.kind)
    }
    pub fn is_guard(&self) -> bool {
        self.protect & PAGE_GUARD != 0
    }
    /// 是否适合做「数值扫描」的可写区（与 memscan::writable_regions 口径一致）。
    pub fn is_scannable_writable(&self, max_region: usize) -> bool {
        self.state == MEM_COMMIT
            && is_directly_writable(self.protect)
            && self.size > 0
            && self.size <= max_region
    }
    pub fn is_private_anon(&self) -> bool {
        self.kind == MEM_PRIVATE
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInfo {
    pub base: usize,
    pub size: usize,
    pub name: String,
    pub path: String,
}

/// 强制写入时对页保护做了什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFix {
    /// 页本来就可写，直接写成功。
    NotNeeded,
    /// 临时改成可写、写完已恢复。
    Reprotected { old: u32 },
}

/// 打开进程失败时按 Win32 错误码给「原因 + 修法」。
fn open_error(pid: u32) -> String {
    let code = last_error();
    match code {
        5 => format!("无法打开进程 {pid}：拒绝访问。请以管理员身份运行 STool；若目标是系统保护进程则无法读写。"),
        87 => format!("无法打开进程 {pid}：参数无效（进程可能已退出）。"),
        0 => format!("无法打开进程 {pid}：进程可能已退出，或权限不足（可尝试管理员运行 STool）。"),
        other => format!("无法打开进程 {pid}：Win32 错误 {other}（权限不足或进程已退出；可尝试管理员运行 STool）。"),
    }
}

// ---------------------------------------------------------------------------
// 进程句柄（RAII）
// ---------------------------------------------------------------------------

/// 目标进程句柄包装：`Drop` 时自动 CloseHandle。
pub struct Proc {
    handle: isize,
    pid: u32,
    wow64: bool,
}

impl Drop for Proc {
    fn drop(&mut self) {
        if self.handle != 0 {
            w::close_handle(self.handle);
        }
    }
}

impl Proc {
    /// 打开进程（读 + 写 + 查询）。写入类操作必须用这个。
    pub fn open(pid: u32) -> Result<Proc, String> {
        Self::open_with(pid, ACCESS_RW_QUERY)
    }

    /// 只读打开（权限要求低，诊断类操作优先用它，能少踩「拒绝访问」）。
    pub fn open_query(pid: u32) -> Result<Proc, String> {
        Self::open_with(pid, ACCESS_QUERY)
    }

    pub fn open_with(pid: u32, access: u32) -> Result<Proc, String> {
        if pid == 0 {
            return Err("进程 ID 不能为 0（请先选择或填写目标进程）".into());
        }
        let handle = w::open_process(access, pid);
        if handle == 0 {
            return Err(open_error(pid));
        }
        let wow64 = w::is_wow64(handle);
        Ok(Proc { handle, pid, wow64 })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// 目标是否为 32 位进程（WOW64）。决定代码段指令解码方式（绝对 vs RIP 相对）。
    pub fn is_32bit(&self) -> bool {
        self.wow64
    }

    /// 裸句柄。给「持有 `&self` 的同时还要改 `&mut self.其它字段`」的场景用
    /// （`isize` 是 Copy，借完即走），配合本模块的 `*_raw` 自由函数。
    pub fn raw(&self) -> isize {
        self.handle
    }

    pub fn read(&self, addr: usize, size: usize) -> Option<Vec<u8>> {
        read_raw(self.handle, addr, size)
    }

    /// 从 `addr` 起按需读取，允许跨区域（最多尝试 `tries` 次，每次补齐缺失部分）。
    /// 用途：跨区域边界读一小段（比如 8 字节值刚好压在页尾）。
    pub fn read_spanning(&self, addr: usize, size: usize) -> Option<Vec<u8>> {
        if let Some(b) = self.read(addr, size) {
            if b.len() >= size {
                return Some(b);
            }
        }
        if size <= 1 {
            return self.read(addr, 1);
        }
        let head = size / 2;
        let mut out = Vec::with_capacity(size);
        out.extend(self.read(addr, head)?);
        out.extend(self.read(addr + head, size - head)?);
        (out.len() >= size).then_some(out)
    }

    pub fn write(&self, addr: usize, bytes: &[u8]) -> bool {
        write_raw(self.handle, addr, bytes)
    }

    pub fn query(&self, addr: usize) -> Option<MemInfo> {
        query_raw(self.handle, addr)
    }

    pub fn region(&self, addr: usize) -> Option<Region> {
        region_raw(self.handle, addr)
    }

    /// 枚举全部区域（VirtualQueryEx 逐个步进）。空闲区也返回，由调用方过滤。
    pub fn regions(&self) -> Vec<Region> {
        let mut out = Vec::new();
        let mut addr: usize = 0x10000;
        // 用户态地址上限（x64 下 0x7FFF_FFFF_FFFF 足够覆盖，x86 也安全）
        let max_addr: usize = 0x7FFF_FFFF_FFFF;
        let mut guard = 0usize;
        while addr < max_addr && guard < 2_000_000 {
            guard += 1;
            let Some((base, size, protect, state)) = self.query(addr) else { break };
            let kind = w::query_kind(self.handle, addr).unwrap_or(0);
            let size = size.max(1);
            out.push(Region { base, size, protect, state, kind });
            match base.checked_add(size) {
                Some(next) if next > addr => addr = next,
                _ => break,
            }
        }
        out
    }

    /// 枚举已加载模块（Toolhelp32 快照）。
    pub fn modules(&self) -> Vec<ModuleInfo> {
        w::enum_modules(self.pid)
    }

    /// 强制写入：普通写入失败（页只读 / Guard / 无写权限）时，
    /// 先 `VirtualProtectEx` 临时放开该页，写完后恢复原保护。
    ///
    /// 这是「值写不进去 / 写完被系统拒绝」这类问题的直接解法之一。
    pub fn force_write(&self, addr: usize, bytes: &[u8]) -> Result<PageFix, String> {
        force_write_raw(self.handle, addr, bytes)
    }
}

// ---------------------------------------------------------------------------
// 裸句柄原语
//
// `Proc` 是 RAII 包装；但内存扫描这类「一边遍历 self.hits 一边读内存」的代码
// 会被借用检查器挡住（`&self` 与 `&mut self.hits` 冲突）。所以底层一律提供
// 接受 `isize` 句柄的自由函数（`isize` 是 Copy，借完即走）。
// ---------------------------------------------------------------------------

pub fn read_raw(handle: isize, addr: usize, size: usize) -> Option<Vec<u8>> {
    if size == 0 {
        return None;
    }
    w::read_mem(handle, addr, size)
}

pub fn write_raw(handle: isize, addr: usize, bytes: &[u8]) -> bool {
    !bytes.is_empty() && w::write_mem(handle, addr, bytes)
}

pub fn query_raw(handle: isize, addr: usize) -> Option<MemInfo> {
    w::query_mem(handle, addr)
}

pub fn region_raw(handle: isize, addr: usize) -> Option<Region> {
    let (base, size, protect, state) = query_raw(handle, addr)?;
    let kind = w::query_kind(handle, addr).unwrap_or(0);
    Some(Region { base, size, protect, state, kind })
}

pub fn protect_raw(handle: isize, addr: usize, size: usize, new_protect: u32) -> Option<u32> {
    w::protect_ex(handle, addr, size, new_protect)
}

/// 强制写入的裸句柄版本：正常写入 → 失败则临时放开页保护 → 写完恢复。
pub fn force_write_raw(handle: isize, addr: usize, bytes: &[u8]) -> Result<PageFix, String> {
    if bytes.is_empty() {
        return Err("要写入的字节为空".into());
    }
    if write_raw(handle, addr, bytes) {
        return Ok(PageFix::NotNeeded);
    }
    let Some(reg) = region_raw(handle, addr) else {
        return Err(format!("0x{addr:016X} 不在任何已提交区域里（地址可能已失效，请重新扫描）"));
    };
    if reg.state != MEM_COMMIT {
        return Err(format!(
            "0x{addr:016X} 所在区域状态为「{}」，无法写入（地址已释放？请重新扫描）",
            state_name(reg.state)
        ));
    }
    // Guard 页要先摘掉 Guard 位，否则写入会触发异常。
    let new_protect = if is_executable(reg.protect) { PAGE_EXECUTE_READWRITE } else { PAGE_READWRITE };
    let need = bytes.len().min(reg.end().saturating_sub(addr));
    let Some(old) = protect_raw(handle, addr, need, new_protect) else {
        return Err(format!(
            "0x{addr:016X} 所在页为「{}」（{}），临时解除页保护也失败（Win32 错误 {}）。\
             多数情况是权限不足：请以管理员身份运行 STool；若是内核/驱动保护的页，本工具无法写入。",
            reg.protect_name(),
            reg.kind_name(),
            last_error()
        ));
    };
    let ok = write_raw(handle, addr, bytes);
    // 无论成败都恢复原保护（Guard 页也还原，避免改变目标进程行为）
    protect_raw(handle, addr, need, old);
    if ok {
        Ok(PageFix::Reprotected { old })
    } else {
        Err(format!(
            "0x{addr:016X} 已临时放开页保护，但写入仍失败（Win32 错误 {}）。\
             该地址可能由内核 / 反作弊驱动保护，本工具无法直接写入。",
            last_error()
        ))
    }
}

// ---------------------------------------------------------------------------
// 进程枚举
// ---------------------------------------------------------------------------

/// 列出系统进程 [(pid, 名字)]，按 pid 排序。
pub fn list_processes() -> Vec<(u32, String)> {
    w::enum_processes()
}

/// 按进程名找 pid（不区分大小写，允许带/不带 `.exe`）。返回第一个匹配。
pub fn find_pid(name: &str) -> Option<u32> {
    let want = name.trim().to_ascii_lowercase();
    if want.is_empty() {
        return None;
    }
    let want_exe = if want.ends_with(".exe") { want.clone() } else { format!("{want}.exe") };
    let procs = list_processes();
    // 先精确匹配带 .exe 的名字，再退化成「包含」
    procs
        .iter()
        .find(|(_, n)| {
            let l = n.to_ascii_lowercase();
            l == want || l == want_exe
        })
        .or_else(|| procs.iter().find(|(_, n)| n.to_ascii_lowercase().contains(&want)))
        .map(|(pid, _)| *pid)
}

fn last_error() -> u32 {
    w::last_error()
}

// ---------------------------------------------------------------------------
// Windows 实现
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod winapi {
    use super::{ModuleInfo, MemInfo};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, Process32FirstW, Process32NextW, MODULEENTRY32W,
        PROCESSENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Memory::{VirtualProtectEx, VirtualQueryEx, MEMORY_BASIC_INFORMATION};
    use windows_sys::Win32::System::Threading::{IsWow64Process, OpenProcess};

    pub fn last_error() -> u32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32
    }

    pub fn open_process(access: u32, pid: u32) -> isize {
        unsafe { OpenProcess(access, 0, pid) as isize }
    }

    pub fn close_handle(h: isize) {
        unsafe { CloseHandle(h as HANDLE) };
    }

    pub fn is_wow64(h: isize) -> bool {
        let mut wow: i32 = 0;
        let ok = unsafe { IsWow64Process(h as HANDLE, &mut wow) };
        ok != 0 && wow != 0
    }

    /// 一次 VirtualQueryEx，返回 (BaseAddress, RegionSize, Protect, State)。
    pub fn query_mem(h: isize, addr: usize) -> Option<MemInfo> {
        mbi(h, addr).map(|m| (m.BaseAddress as usize, m.RegionSize, m.Protect, m.State))
    }

    /// 单独取 Type（MEM_IMAGE / MEM_MAPPED / MEM_PRIVATE）——MEMORY_BASIC_INFORMATION 才有。
    pub fn query_kind(h: isize, addr: usize) -> Option<u32> {
        mbi(h, addr).map(|m| m.Type)
    }

    fn mbi(h: isize, addr: usize) -> Option<MEMORY_BASIC_INFORMATION> {
        unsafe {
            let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            let n = VirtualQueryEx(
                h as HANDLE,
                addr as *const core::ffi::c_void,
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            );
            (n != 0).then_some(mbi)
        }
    }

    pub fn read_mem(h: isize, addr: usize, size: usize) -> Option<Vec<u8>> {
        unsafe {
            let mut buf = vec![0u8; size];
            let mut read = 0usize;
            let ok = ReadProcessMemory(
                h as HANDLE,
                addr as *const core::ffi::c_void,
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                size,
                &mut read,
            );
            if ok != 0 && read > 0 {
                buf.truncate(read);
                Some(buf)
            } else {
                None
            }
        }
    }

    pub fn write_mem(h: isize, addr: usize, bytes: &[u8]) -> bool {
        unsafe {
            let mut written = 0usize;
            WriteProcessMemory(
                h as HANDLE,
                addr as *const core::ffi::c_void,
                bytes.as_ptr() as *const core::ffi::c_void,
                bytes.len(),
                &mut written,
            ) != 0
        }
    }

    /// 改页保护，返回旧保护（失败 None）。
    pub fn protect_ex(h: isize, addr: usize, size: usize, new_protect: u32) -> Option<u32> {
        unsafe {
            let mut old: u32 = 0;
            let ok = VirtualProtectEx(
                h as HANDLE,
                addr as *const core::ffi::c_void,
                size,
                new_protect,
                &mut old,
            );
            (ok != 0).then_some(old)
        }
    }

    pub fn enum_processes() -> Vec<(u32, String)> {
        let mut out = Vec::new();
        unsafe {
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
        }
        out.sort();
        out
    }

    pub fn enum_modules(pid: u32) -> Vec<ModuleInfo> {
        let mut out = Vec::new();
        unsafe {
            // 32/64 位进程都要能枚举：两个标志位一起给，快照失败再退化成单标志重试
            let mut snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
            if snap as isize == -1 {
                snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
            }
            if snap as isize == -1 {
                return out;
            }
            let mut entry: MODULEENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
            if Module32FirstW(snap, &mut entry) != 0 {
                loop {
                    out.push(ModuleInfo {
                        base: entry.modBaseAddr as usize,
                        size: entry.modBaseSize as usize,
                        name: wide_to_string(&entry.szModule),
                        path: wide_to_string(&entry.szExePath),
                    });
                    if Module32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        out
    }

    fn wide_to_string(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }
}

// ---------------------------------------------------------------------------
// 非 Windows 桩：保持接口存在，全部返回失败，让跨平台编译通过
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
mod winapi {
    use super::ModuleInfo;
    pub fn last_error() -> u32 {
        0
    }
    pub fn open_process(_access: u32, _pid: u32) -> isize {
        0
    }
    pub fn close_handle(_h: isize) {}
    pub fn is_wow64(_h: isize) -> bool {
        false
    }
    pub fn read_mem(_h: isize, _addr: usize, _size: usize) -> Option<Vec<u8>> {
        None
    }
    pub fn write_mem(_h: isize, _addr: usize, _bytes: &[u8]) -> bool {
        false
    }
    pub fn protect_ex(_h: isize, _addr: usize, _size: usize, _new: u32) -> Option<u32> {
        None
    }
    pub fn enum_processes() -> Vec<(u32, String)> {
        Vec::new()
    }
    pub fn enum_modules(_pid: u32) -> Vec<ModuleInfo> {
        Vec::new()
    }
}

#[cfg(windows)]
use winapi as w;

#[cfg(not(windows))]
use winapi as w;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protect_helpers_agree_with_win32_bits() {
        assert!(is_directly_writable(PAGE_READWRITE));
        assert!(is_directly_writable(PAGE_EXECUTE_READWRITE));
        assert!(is_directly_writable(PAGE_WRITECOPY));
        // 只读 / 仅执行 不可直接写
        assert!(!is_directly_writable(PAGE_READONLY));
        assert!(!is_directly_writable(PAGE_EXECUTE_READ));
        assert!(!is_directly_writable(PAGE_NOACCESS));
        // Guard 位一挂上就必须先解除
        assert!(!is_directly_writable(PAGE_READWRITE | PAGE_GUARD));
        assert!(is_executable(PAGE_EXECUTE_READ));
        assert!(!is_executable(PAGE_READWRITE));
        assert_eq!(protect_name(PAGE_READWRITE | PAGE_GUARD), "可读写");
        assert_eq!(protect_name(PAGE_READONLY), "只读");
        assert_eq!(kind_name(MEM_IMAGE), "映像(文件映射)");
        assert_eq!(state_name(MEM_COMMIT), "已提交");
    }

    #[test]
    fn region_helpers_bound_correctly() {
        let r = Region {
            base: 0x1000,
            size: 0x2000,
            protect: PAGE_READWRITE,
            state: MEM_COMMIT,
            kind: MEM_PRIVATE,
        };
        assert_eq!(r.end(), 0x3000);
        assert!(r.is_scannable_writable(1024 * 1024));
        assert!(!r.is_scannable_writable(0x100)); // 区域比上限大 → 跳过
        assert!(r.is_private_anon());
        // 溢出不应 panic
        let big = Region { base: usize::MAX - 3, size: 16, ..r };
        assert_eq!(big.end(), usize::MAX);
    }

    #[test]
    fn find_pid_tolerates_exe_suffix_and_empty() {
        // 不依赖真实进程列表：空输入必须返回 None 且不 panic
        assert!(find_pid("").is_none());
        assert!(find_pid("   ").is_none());
        // 不存在的名字要么 None，要么（极不可能）某个包含匹配；不断言具体值，只要求不 panic
        let _ = find_pid("__stool_no_such_process_zzz__");
    }

    #[test]
    fn open_rejects_pid_zero() {
        assert!(Proc::open(0).is_err());
        assert!(Proc::open_query(0).is_err());
        let e = Proc::open(0).err().unwrap_or_default();
        assert!(e.contains("不能为 0"), "错误信息应给出原因: {e}");
    }

    #[test]
    fn open_bogus_pid_is_error_not_panic() {
        // 一个几乎不可能存在的 pid：只要求返回 Err，不允许 panic
        assert!(Proc::open(0x7FFF_FFF0).is_err());
    }
}
