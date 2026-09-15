//! 反修改保护识别与对抗（针对「CE 改完立刻被还原 / 改了没用」）。
//!
//! ## 要解决的问题
//! 单机游戏常把关键数值（金币、好感度、CG 解锁标记）做了「反修改」处理：写入后立刻被
//! 校验线程/驱动还原、同一份数据存多份互相覆盖、或页面被置为只读。表现为
//! **改了没反应 / 改完过一瞬又变回去**，而普通内存扫描器只会说「写入成功」。
//!
//! ## 本模块做什么
//! 1. **写入-存活探测**：反复写入探测值，测量每次写入「存活」多久 → 判定
//!    `立即回滚 / 周期性回滚 / 延迟回滚 / 稳定`，并给出可用的写入周期。
//! 2. **写入被改写判定**：用两次不同写入做差分，判断写入值与读回值之间是否存在
//!    XOR / 偏移 / 常量关系 → 说明该地址是「派生/缓存」，写它没用。
//! 3. **同值镜像 + 数据源定位**：找出所有持有同值的地址，再对**可写且不可执行**页上的副本
//!    逐个写探测值反测 —— 写哪个副本会让目标地址跟着变，哪个就是真正被游戏采纳的「源」。
//! 4. **页保护**：只读 / Guard / 私有 vs 映像 —— 这类地址普通写入会被系统拒绝，
//!    需要 [`crate::memapi::force_write_raw`] 临时解除保护。
//! 5. **保护机制线索**：已加载模块里的加壳 / DRM / 在线反作弊，以及主程序导入的
//!    反调试 API（PE 导入表）。
//! 6. **还原点定位**：在可执行区搜 `mov [disp32], imm32` 形式的「立即数写入点」，
//!    给出**可能是重置指令**的地址 —— 这是代码补丁的落点。
//! 7. **应对方案**：按上面的事实生成带可行性评级的方案清单（强制写入 / 自适应锁值 /
//!    改数据源 / 代码补丁 / 替代路线 / 在线反作弊不建议改）。
//!
//! ## 边界（重要）
//! 本模块只做**本地单机游戏**的数值保护识别与对抗；识别到 `EasyAntiCheat` /
//! `BattlEye` / `Vanguard` / `ACE` 等**在线反作弊**时只给风险提示，**不提供绕过**。

use crate::features::memscan::{self, ScanType, ScanValue};
use crate::memapi::{self, PageFix, Proc, Region};
use std::cmp::Reverse;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ProbeOpts {
    /// 探测值。None = 用 [`memscan::auto_probe`] 自动构造。
    pub probe: Option<String>,
    /// 单次写入的观察窗口（毫秒）。写入后超过这个时间还没被改回，就认为「存活」。
    pub window_ms: u64,
    /// 最多重复写入探测多少次（用于估计回滚周期）。
    pub rounds: u32,
    /// 是否做同值镜像扫描（较慢：要遍历可写内存）。
    pub mirror: bool,
    /// 是否做代码段「立即数写入点」搜索（较慢）。
    pub code: bool,
    /// 最多记录多少个镜像地址。
    pub max_mirrors: usize,
}

impl Default for ProbeOpts {
    fn default() -> Self {
        ProbeOpts { probe: None, window_ms: 400, rounds: 5, mirror: true, code: true, max_mirrors: 64 }
    }
}

/// 镜像扫描最多读多少字节（防止在几十 GB 的进程上卡死）。
const MIRROR_MAX_BYTES: usize = 512 * 1024 * 1024;
/// 单个区域超过这个大小就不做镜像扫描。
const MIRROR_MAX_REGION: usize = 64 * 1024 * 1024;
/// 代码扫描最多读多少字节 / 单个区域上限。
const CODE_MAX_BYTES: usize = 192 * 1024 * 1024;
const CODE_MAX_REGION: usize = 64 * 1024 * 1024;
/// 单次探测的采样上限（防止把 CPU 跑满）。
const MAX_SAMPLES: u64 = 400_000;
/// 同值副本「写谁才是源」的反测次数上限（只数**可写**的副本；每轮 3 次跨进程调用）。
const MAX_SOURCE_TRIES: usize = 24;

// ---------------------------------------------------------------------------
// 判定类型
// ---------------------------------------------------------------------------

/// 写入后值「存活」情况的判定。
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// 写入后一直没被改回去 → 内存层面没有保护。
    Stable,
    /// 写入后极快被改回（< 1ms）→ 同线程写完即校验、内核回调、或高频轮询线程。
    /// 单线程锁值**赢不了**（写入间隙比这还长）。
    Instant { after_us: u64 },
    /// 每次写入都在相近的时间内被改回 → 有个周期性校验/重写。
    Periodic { period_ms: f64, rounds: u32 },
    /// 写入后过了较长时间才被改回，且时间不规律（按帧 / 按秒 / 事件触发）。
    Delayed { after_ms: f64, rounds: u32 },
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Stable => "稳定（写入后被保持）",
            Verdict::Instant { .. } => "立即回滚（极快被还原）",
            Verdict::Periodic { .. } => "周期性回滚",
            Verdict::Delayed { .. } => "非固定间隔回滚",
        }
    }
    /// 带实测数值的描述。
    pub fn measure(&self) -> String {
        match self {
            Verdict::Stable => "写入后一直被保持到观察窗口结束".into(),
            Verdict::Instant { after_us } => format!("写入后约 {after_us} 微秒即被还原"),
            Verdict::Periodic { period_ms, rounds } => format!("每 {period_ms:.1} 毫秒被还原一次（{rounds} 轮实测）"),
            Verdict::Delayed { after_ms, rounds } => {
                format!("约 {after_ms:.1} 毫秒后被还原（{rounds} 轮实测，间隔不固定）")
            }
        }
    }
    /// 这个判定下「值是否会被还原」。
    pub fn is_reverted(&self) -> bool {
        !matches!(self, Verdict::Stable)
    }
}

/// 写入值与读回值之间的关系（两次写入做差分）。
#[derive(Debug, Clone, PartialEq)]
pub enum Rewrite {
    /// 读回值 = 写入值 XOR key → 该地址存的是派生/编码值，或写入被按位篡改。
    Xor { key: u64 },
    /// 读回值 = 写入值 + delta。
    Shift { delta: i64 },
    /// 写什么都读回同一个值 → 影子副本，真实数据在别处。
    Constant { fixed: ScanValue },
    /// 有稳定的改写关系但不符合上面几种（可能多次变换/取反）。
    Opaque,
}

impl Rewrite {
    pub fn describe(&self) -> String {
        match self {
            Rewrite::Xor { key } => format!("读回值 = 写入值 XOR 0x{key:X}（该地址存的是编码/派生值）"),
            Rewrite::Shift { delta } => format!("读回值 = 写入值 {:+}（该地址存的是带偏移的派生值）", delta),
            Rewrite::Constant { fixed } => format!("写什么都读回 {}（影子副本：真实值在别处）", fixed.display()),
            Rewrite::Opaque => "写入值与读回值之间有不规则的改写关系（可能被多重复算）".to_string(),
        }
    }
}

/// 代码段里一条「立即数写入」指令的候选。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImmWrite {
    /// 指令起始地址（含 REX 前缀）。
    pub off: usize,
    /// 指令长度。
    pub len: usize,
    /// 写入宽度（4 或 8 字节）。
    pub size: u16,
    /// 立即数。
    pub imm: u64,
    /// 写入目标地址。仅 `[disp32]` 形式可解析（x64 为 RIP 相对、x86-32 为绝对）。
    pub target: Option<usize>,
}

/// 一条命中（已换算成绝对地址）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeHit {
    pub addr: usize,
    pub len: usize,
    pub size: u16,
    pub imm: u64,
    pub target: Option<usize>,
}

impl CodeHit {
    pub fn describe(&self) -> String {
        match self.target {
            Some(t) => format!("写入 0x{t:016X}，宽度 {} 字节，立即数 {}", self.size, self.imm),
            None => format!("写入常量 {}（目标由寄存器算出，无法直接定位），宽度 {} 字节", self.imm, self.size),
        }
    }
}

/// 已知保护机制的分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// 在线反作弊（多人/联网游戏）——只提示风险，不提供绕过。
    OnlineAntiCheat,
    /// 本地 DRM（单机正版校验）。
    LocalDrm,
    /// 加壳 / 保护壳（Themida、VMProtect 等）。
    Packer,
}

impl Family {
    pub fn label(&self) -> &'static str {
        match self {
            Family::OnlineAntiCheat => "在线反作弊",
            Family::LocalDrm => "本地 DRM",
            Family::Packer => "加壳/保护壳",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleHit {
    /// 实际命中的模块文件名。
    pub module: String,
    /// 识别出的保护机制名。
    pub name: String,
    pub family: Family,
    pub advice: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportHit {
    pub module: String,
    pub func: String,
    pub why: &'static str,
}

/// 方案可行性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Feasibility {
    /// 明确不建议（在线反作弊 / 注定无效）。
    NotAdvised,
    Low,
    Medium,
    High,
}

impl Feasibility {
    pub fn label(&self) -> &'static str {
        match self {
            Feasibility::NotAdvised => "不建议",
            Feasibility::Low => "可行性低",
            Feasibility::Medium => "可行性中",
            Feasibility::High => "可行性高",
        }
    }
}

/// 可由工具直接执行的动作。
#[derive(Debug, Clone, PartialEq)]
pub enum AutoAction {
    /// 强制写入（临时解除页保护）。
    ForceWrite,
    /// 按给定周期锁值。
    Freeze { period_ms: u64 },
    /// 改写入某个「源」地址。
    WriteSource { addr: usize },
}

/// 一条应对方案。
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub title: String,
    pub feasibility: Feasibility,
    pub detail: String,
    pub steps: Vec<String>,
    pub auto: Option<AutoAction>,
}

// ---------------------------------------------------------------------------
// 纯逻辑：判定
// ---------------------------------------------------------------------------

/// 由「每次写入的存活时间（微秒）」判定回滚行为。
///
/// * `survivals_us`：每轮的存活时间；`0` 表示这一轮写入后直到窗口结束都没被改回。
/// * 全为 0 / 空 → [`Verdict::Stable`]。
pub fn classify_revert(survivals_us: &[u64]) -> Verdict {
    let times: Vec<u64> = survivals_us.iter().copied().filter(|t| *t > 0).collect();
    if times.is_empty() {
        return Verdict::Stable;
    }
    // 多轮存活时间高度一致 → 周期性校验
    if times.len() >= 3 {
        let mut sorted = times.clone();
        sorted.sort_unstable();
        let median = sorted[sorted.len() / 2];
        let lo = (median as f64 * 0.5) as u64;
        let hi = (median as f64 * 1.6) as u64 + 1;
        if median > 0 && times.iter().all(|t| *t >= lo && *t <= hi) {
            return Verdict::Periodic { period_ms: median as f64 / 1000.0, rounds: times.len() as u32 };
        }
    }
    let min = *times.iter().min().unwrap_or(&0);
    // 「立即回滚」要求每一轮都撑不过 1 毫秒，且没有任何一轮活满窗口 ——
    // 只要有一轮活满窗口，就说明还原不是必经路径，只是偶发。
    let survived_full_window = survivals_us.contains(&0);
    if !survived_full_window && times.iter().all(|t| *t < 1000) {
        Verdict::Instant { after_us: min }
    } else {
        Verdict::Delayed { after_ms: min as f64 / 1000.0, rounds: times.len() as u32 }
    }
}

/// 由两次「写入 → 立刻读回」的原始 8 字节视图判定改写关系。
///
/// 返回 `None` 表示读回值等于写入值（没有被改写）。
pub fn classify_rewrite(w1: u64, r1: u64, w2: u64, r2: u64) -> Option<Rewrite> {
    if r1 == w1 && r2 == w2 {
        return None;
    }
    if r1 == r2 {
        // 定长 8 字节视图下与具体数值类型无关，调用方会把 fixed 换成对应类型的展示值
        return Some(Rewrite::Constant { fixed: ScanValue::Int(r1 as i64) });
    }
    if (r1 ^ r2) == (w1 ^ w2) && r1 != w1 {
        return Some(Rewrite::Xor { key: r1 ^ w1 });
    }
    let (dw, dr) = (w1 as i64 - w2 as i64, r1 as i64 - r2 as i64);
    if dw == dr && r1 != w1 {
        return Some(Rewrite::Shift { delta: r1 as i64 - w1 as i64 });
    }
    Some(Rewrite::Opaque)
}

/// 建议的锁值周期（毫秒）。`None` = 锁值注定无效，别浪费 CPU。
///
/// 判据：**回滚间隔 ≥ 1 毫秒**才可能赢。轮询式锁值最快约每 1 毫秒写一次，
/// 间隔比这更短时（含「立即回滚」）再锁也只会输。
pub fn recommended_period_ms(v: &Verdict) -> Option<u64> {
    let interval = match v {
        Verdict::Stable | Verdict::Instant { .. } => return None,
        Verdict::Periodic { period_ms, .. } => *period_ms,
        Verdict::Delayed { after_ms, .. } => *after_ms,
    };
    if interval < 1.0 {
        return None;
    }
    Some(((interval / 3.0) as u64).max(1))
}

/// 解析用户输入的地址：支持 `0x1234`、`1234`（十进制）、`7FF6A000`（含 a-f 视作十六进制）。
pub fn parse_addr(s: &str) -> Option<usize> {
    let t = s.trim().replace('_', "");
    if t.is_empty() {
        return None;
    }
    let (radix, digits) = if let Some(rest) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        (16, rest)
    } else if t.chars().all(|c| c.is_ascii_digit()) {
        (10, t.as_str())
    } else {
        (16, t.as_str())
    };
    usize::from_str_radix(digits, radix).ok()
}

/// 格式化地址。
pub fn fmt_addr(a: usize) -> String {
    format!("0x{a:016X}")
}

// ---------------------------------------------------------------------------
// 纯逻辑：x86 / x64「立即数写入」指令扫描
// ---------------------------------------------------------------------------

/// `C7 /0` 的 ModRM → 立即数相对指令起始（含 REX）的偏移。
/// 只收录**长度固定**的几种寻址形式，避免把不定长编码猜错。
fn modrm_imm_offset(modrm: u8) -> Option<usize> {
    match modrm {
        0x05 => Some(6), // mod=00 rm=101 → [disp32]（x64 RIP 相对 / x86-32 绝对）
        0x85 => Some(6), // mod=10 rm=101 → [disp32]
        0x45 => Some(3), // mod=01 rm=101 → [rbp+disp8]
        0x44 => Some(4), // mod=01 rm=100 → [SIB+disp8]
        0x84 => Some(7), // mod=10 rm=100 → [SIB+disp32]
        _ => None,
    }
}

/// 遍历代码里所有「`mov [..], imm`」候选。`base` 是 `code[0]` 的绝对地址，
/// `is_32bit` 决定 `[disp32]` 是绝对地址（x86-32）还是 RIP 相对（x64）。
pub fn for_each_imm_write<F: FnMut(ImmWrite)>(code: &[u8], base: usize, is_32bit: bool, mut f: F) {
    let mut i = 0usize;
    // 已上报指令的结束位置（不含）：落在这段范围内的起始点属于「同一条指令的错位解码」，丢弃。
    let mut covered_until = 0usize;
    while i < code.len() {
        if i < covered_until {
            i += 1;
            continue;
        }
        let (rex_w, op) = match code[i] {
            0xC7 => (false, i),
            // REX.W 前缀 0x48..=0x4F（W 位在 0x08，0x48-0x4F 均置位）。32 位进程里没有 REX，不认。
            0x48..=0x4F if !is_32bit && code.get(i + 1) == Some(&0xC7) => (true, i + 1),
            _ => {
                i += 1;
                continue;
            }
        };
        let Some(&modrm) = code.get(op + 1) else {
            i += 1;
            continue;
        };
        let Some(imm_rel) = modrm_imm_offset(modrm) else {
            i += 1;
            continue;
        };
        let imm_at = op + imm_rel;
        let Some(imm_b) = code.get(imm_at..imm_at + 4) else {
            i += 1;
            continue;
        };
        let imm = u32::from_le_bytes([imm_b[0], imm_b[1], imm_b[2], imm_b[3]]) as u64;
        let size: u16 = if rex_w { 8 } else { 4 };
        let insn_len = imm_at + 4 - i;
        let target = if modrm == 0x05 || modrm == 0x85 {
            let Some(db) = code.get(op + 2..op + 6) else {
                i += 1;
                continue;
            };
            let disp = i32::from_le_bytes([db[0], db[1], db[2], db[3]]);
            if is_32bit {
                Some(disp as u32 as usize)
            } else {
                // x64：RIP = 下一条指令地址 = 指令起始 + 长度
                let next = (base as i64).wrapping_add(i as i64).wrapping_add(insn_len as i64);
                Some(next.wrapping_add(disp as i64) as usize)
            }
        } else {
            None
        };
        covered_until = i + insn_len;
        f(ImmWrite { off: base + i, len: insn_len, size, imm, target });
        i += 1;
    }
}

/// 在代码里找「会写到 `target` 这个地址」的立即数写入点。
pub fn find_imm_writers(code: &[u8], base: usize, target: usize, is_32bit: bool, cap: usize) -> Vec<CodeHit> {
    let mut out = Vec::new();
    for_each_imm_write(code, base, is_32bit, |w| {
        if out.len() >= cap || w.target != Some(target) {
            return;
        }
        out.push(CodeHit { addr: w.off, len: w.len, size: w.size, imm: w.imm, target: w.target });
    });
    out
}

/// 在代码里找「写入常量 `imm`」的指令（不管写到哪）——用于找「把值重置成 N」的指令。
/// 注意：指令里的立即数只有 32 位，所以只比较低 32 位。
pub fn find_imm_constants(code: &[u8], base: usize, imm: u64, is_32bit: bool, cap: usize) -> Vec<CodeHit> {
    let low = imm as u32 as u64;
    let mut out = Vec::new();
    for_each_imm_write(code, base, is_32bit, |w| {
        if out.len() >= cap || w.imm != low {
            return;
        }
        out.push(CodeHit { addr: w.off, len: w.len, size: w.size, imm: w.imm, target: w.target });
    });
    out
}

// ---------------------------------------------------------------------------
// 纯逻辑：已知保护机制名单
// ---------------------------------------------------------------------------

/// (模块名子串, 显示名, 分类, 建议)。匹配不区分大小写。
const MODULE_TABLE: &[(&str, &str, Family, &str)] = &[
    // ---- 在线反作弊：只提示风险 ----
    ("easyanticheat", "Easy Anti-Cheat (EAC)", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存：可能被判定异常并封号；离线单人模式请断网游玩。"),
    ("eac_", "Easy Anti-Cheat (EAC)", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存。"),
    ("becclient", "BattlEye", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存：可能被判定异常并封号。"),
    ("beservice", "BattlEye", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存。"),
    ("battleye", "BattlEye", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存。"),
    ("gameguard", "nProtect GameGuard", Family::OnlineAntiCheat, "在线反作弊（韩国 INCA）。不建议修改内存；单机模式可断网。"),
    ("nprotect", "nProtect GameGuard", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存。"),
    ("npggnt", "nProtect GameGuard", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存。"),
    ("xigncode", "XIGNCODE3 (Wellbia)", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存。"),
    ("x3.xem", "XIGNCODE3 (Wellbia)", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存。"),
    ("wellbia", "XIGNCODE3 (Wellbia)", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存。"),
    ("vanguard", "Riot Vanguard", Family::OnlineAntiCheat, "内核级在线反作弊。强烈不建议修改内存。"),
    ("anticheatexpert", "腾讯 ACE", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存：可能被判定异常并封号。"),
    ("ace-guard", "腾讯 ACE", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存。"),
    ("tensafe", "腾讯 TP / ACE", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存。"),
    ("sguard", "腾讯 TP", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存。"),
    ("fairfight", "FairFight", Family::OnlineAntiCheat, "服务端行为分析反作弊。修改内存意义不大且可能被判定异常。"),
    ("mhyprot", "米哈游 mhyprot", Family::OnlineAntiCheat, "在线反作弊驱动。不建议修改内存。"),
    ("equ8", "EQU8", Family::OnlineAntiCheat, "在线反作弊。不建议修改内存。"),
    // ---- 加壳 / 保护壳：会增加难度但不针对内存修改 ----
    ("themida", "Themida / WinLicense", Family::Packer, "加壳保护。会做反调试与代码虚拟化，内存地址可能不稳定；建议改用存档/脚本路线或先脱壳。"),
    ("winlicense", "Themida / WinLicense", Family::Packer, "加壳保护。会做反调试与代码虚拟化。"),
    ("vmprotect", "VMProtect", Family::Packer, "代码虚拟化保护：被保护函数无法直接读改，建议避开这些地址。"),
    ("enigma", "Enigma Protector", Family::Packer, "加壳保护：含反调试与校验，写入可能被还原。"),
    ("asprotect", "ASProtect", Family::Packer, "老式加壳，含校验；写入被还原时优先怀疑它。"),
    ("aspack", "ASPack", Family::Packer, "压缩壳，主要是体积优化，一般不拦内存修改。"),
    ("upx", "UPX 压缩壳", Family::Packer, "压缩壳，一般不影响内存修改；若地址抖动明显可先脱壳。"),
    ("obsidium", "Obsidium", Family::Packer, "加壳保护，含反调试。"),
    ("safengine", "Safengine Shielden", Family::Packer, "加壳保护，含反调试/反内存修改校验。"),
    ("molebox", "MoleBox", Family::Packer, "虚拟化打包，会重定向文件访问。"),
    // ---- 本地 DRM ----
    ("denuvo", "Denuvo", Family::LocalDrm, "本地 DRM。一般不拦内存修改，但会带来明显性能开销与反调试。"),
    ("securom", "SecuROM", Family::LocalDrm, "老式本地 DRM，含光盘校验。"),
    ("safedisc", "SafeDisc", Family::LocalDrm, "老式本地 DRM。"),
    ("starforce", "StarForce", Family::LocalDrm, "老式本地 DRM，含驱动级校验。"),
    ("steam_api", "Steamworks (DRM)", Family::LocalDrm, "平台 DRM，不拦内存修改；仅提示。"),
    ("steamclient", "Steamworks (DRM)", Family::LocalDrm, "平台 DRM，不拦内存修改；仅提示。"),
];

/// 用已加载模块名匹配已知保护机制（同名机制只报一次）。
pub fn classify_modules(names: &[String]) -> Vec<ModuleHit> {
    let mut out: Vec<ModuleHit> = Vec::new();
    for raw in names {
        let low = raw.to_ascii_lowercase();
        for (pat, name, family, advice) in MODULE_TABLE {
            if !low.contains(pat) {
                continue;
            }
            if out.iter().any(|h| h.name == *name) {
                continue;
            }
            out.push(ModuleHit {
                module: raw.clone(),
                name: (*name).to_string(),
                family: *family,
                advice: (*advice).to_string(),
            });
        }
    }
    out
}

/// 在线反作弊的显示名（用于生成「不建议」方案）。
pub fn online_ac_names(hits: &[ModuleHit]) -> Vec<String> {
    hits.iter().filter(|h| h.family == Family::OnlineAntiCheat).map(|h| h.name.clone()).collect()
}

/// (导入函数名子串, 说明)。匹配不区分大小写。
const ANTIDEBUG_IMPORTS: &[(&str, &str)] = &[
    ("isdebuggerpresent", "检测调试器（最常见）"),
    ("checkremotedebuggerpresent", "检测远程调试器"),
    ("ntqueryinformationprocess", "查询调试端口/调试对象（隐蔽反调试）"),
    ("ntsetinformationthread", "隐藏线程（反调试惯用手法）"),
    ("ntquerydebugfilterstate", "查询调试过滤状态"),
    ("outputdebugstring", "用 OutputDebugString 副作用反调试"),
    ("debugactiveprocess", "主动附加/自调试"),
    ("ntqueryobject", "句柄枚举（检测工具句柄）"),
];

/// 从导入函数名里挑出反调试相关的（同名只报一次）。
pub fn classify_imports(funcs: &[String]) -> Vec<ImportHit> {
    let mut out: Vec<ImportHit> = Vec::new();
    for raw in funcs {
        let low = raw.to_ascii_lowercase();
        for (pat, why) in ANTIDEBUG_IMPORTS {
            if !low.contains(pat) {
                continue;
            }
            if out.iter().any(|h| h.func.eq_ignore_ascii_case(raw)) {
                continue;
            }
            out.push(ImportHit { module: String::new(), func: raw.clone(), why });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 纯逻辑：方案生成
// ---------------------------------------------------------------------------

/// 生成方案所需的事实。
pub struct PlanInput<'a> {
    pub verdict: &'a Verdict,
    pub rewrite: Option<&'a Rewrite>,
    pub page_blocked: bool,
    pub mirrors: usize,
    pub source_addr: Option<usize>,
    pub writer_addr: Option<usize>,
    pub ac_modules: &'a [String],
    pub read_only: bool,
}

/// 按事实生成方案清单（已按可行性排序，最该先试的在前）。
pub fn build_plans(inp: &PlanInput) -> Vec<Plan> {
    let mut plans: Vec<Plan> = Vec::new();

    // 0) 在线反作弊：永远排最前，且明确不建议
    if !inp.ac_modules.is_empty() {
        plans.push(Plan {
            title: "检测到在线反作弊：不建议修改".into(),
            feasibility: Feasibility::NotAdvised,
            detail: format!(
                "已加载模块里发现 {}。这类反作弊面向联网对战，改内存可能被判定异常并封号，\
                 本工具不提供绕过手段。",
                inp.ac_modules.join("、")
            ),
            steps: vec![
                "若是单人/离线模式：先断网再启动游戏，通常反作弊就不会加载。".into(),
                "若必须联网：请改用游戏内正规途径，不要改内存。".into(),
            ],
            auto: None,
        });
    }

    if inp.read_only {
        plans.push(Plan {
            title: "先解决权限：以管理员身份运行".into(),
            feasibility: Feasibility::High,
            detail: "当前只能只读打开目标进程，无法做写入探测与修改，很多结论拿不到。".into(),
            steps: vec![
                "关闭 STool，右键「以管理员身份运行」后重试。".into(),
                "若仍失败：目标可能由内核驱动保护，改用下面的替代路线。".into(),
            ],
            auto: None,
        });
    }

    // 1) 页保护：普通写入会被系统拒绝
    if inp.page_blocked {
        plans.push(Plan {
            title: "用「强制写入」临时解除页保护".into(),
            feasibility: Feasibility::High,
            detail: "目标地址所在页是只读 / Guard，普通写入会被系统拒绝（看起来像「改了没反应」）。\
                     STool 可临时把该页改为可写、写完立刻恢复原保护。"
                .into(),
            steps: vec![
                "在内存扫描面板点「强制写入（解除页保护）」。".into(),
                "若游戏把该页定期改回只读，需要配合锁值反复写。".into(),
            ],
            auto: Some(AutoAction::ForceWrite),
        });
    }

    // 2) 写入被改写：地址是派生/缓存，写它没用
    if let Some(rw) = inp.rewrite {
        plans.push(Plan {
            title: "该地址是被改写/派生的，别只写它".into(),
            feasibility: Feasibility::Medium,
            detail: rw.describe(),
            steps: vec![
                "用「再次扫描 → 没变/变了」确认哪个地址是游戏真正在用的那一份。".into(),
                "优先写「源地址」（本报告若已定位会直接给出）；源地址不存在时走代码补丁或替代路线。".into(),
            ],
            auto: None,
        });
    }

    // 3) 同值镜像 / 已定位数据源
    if let Some(src) = inp.source_addr {
        plans.push(Plan {
            title: format!("写入真正的数据源 {}", fmt_addr(src)),
            feasibility: Feasibility::High,
            detail: "对同值副本逐个反测发现：写这个地址时目标地址会跟着变，说明它才是被游戏采纳的源。"
                .into(),
            steps: vec![
                "先取消对目标地址的锁值，避免两个地址互相覆盖。".into(),
                format!("改为对 {} 写入并锁值。", fmt_addr(src)),
            ],
            auto: Some(AutoAction::WriteSource { addr: src }),
        });
    } else if inp.mirrors > 32 {
        plans.push(Plan {
            title: format!("同值副本有 {} 个（太多，先收敛候选）", inp.mirrors),
            feasibility: Feasibility::Low,
            detail: "同一个数值在内存里存了上百份。逐个试写既费时又容易改坏无关数据，\
                     应当先用「再次扫描」把候选缩小到几个。"
                .into(),
            steps: vec![
                "回游戏让这个数值变化，再用「再次搜索 → 变了/没变」过滤，重复 2–3 轮。".into(),
                "命中降到 5 个以内后，再逐个写入并回游戏确认哪个生效。".into(),
            ],
            auto: None,
        });
    } else if inp.mirrors > 1 {
        plans.push(Plan {
            title: format!("有 {} 个同值副本，逐个试写", inp.mirrors),
            feasibility: Feasibility::Medium,
            detail: "同一个数值在内存里存了多份。只写其中一份时，另一份可能在下一帧把你的值覆盖掉。"
                .into(),
            steps: vec![
                "在命中列表里逐个地址「写入」并回游戏看是否生效（每次只写一个）。".into(),
                "找不到的话勾选锁值，把多个地址一起锁住。".into(),
            ],
            auto: None,
        });
    }

    // 4) 锁值：只在「赢得了」的时候推荐；赢不了就明确劝退
    let period = recommended_period_ms(inp.verdict);
    match (inp.verdict.is_reverted(), period) {
        (false, _) => {}
        (true, Some(ms)) => plans.push(Plan {
            title: format!("自适应锁值：每 {ms} 毫秒写回一次"),
            feasibility: if ms >= 2 { Feasibility::High } else { Feasibility::Medium },
            detail: format!(
                "测得回滚特征为「{}」。把写入周期压到回滚间隔的 1/3 以内，通常能让数值稳定不掉。",
                inp.verdict.measure()
            ),
            steps: vec![
                "在扫描面板点「自适应锁值」，周期已按上面的数值自动设好。".into(),
                "锁定期间若要改锁定值：先解锁 → 改数值 → 再锁定。".into(),
            ],
            auto: Some(AutoAction::Freeze { period_ms: ms }),
        }),
        (true, None) => plans.push(Plan {
            title: "锁值无效：请改用代码补丁".into(),
            feasibility: Feasibility::NotAdvised,
            detail: format!(
                "测得回滚特征为「{}」。轮询式锁值最快也只能每 1 毫秒写一次，\
                 回滚间隔比这更短时时间上必然输 —— 再锁也只会白耗 CPU、并把游戏拖慢。",
                inp.verdict.measure()
            ),
            steps: vec![
                "改用下面的「代码补丁」方案，让还原动作本身失效。".into(),
                "或改用「替代路线」（存档 / 脚本 / 引擎调试接口）。".into(),
            ],
            auto: None,
        }),
    }

    // 5) 代码补丁
    if let Some(w) = inp.writer_addr {
        plans.push(Plan {
            title: format!("代码补丁：改写 {}", fmt_addr(w)),
            feasibility: Feasibility::High,
            detail: "在可执行区找到了「写到这个地址」的立即数写入指令。把它改成你要的值（或把整个指令填 NOP）\
                     就能消除「被还原」—— 这是最彻底的做法。"
                .into(),
            steps: vec![
                "先确认它是还原指令：把它改成写你要的值，回游戏看数值是否稳定。".into(),
                "改前先记下原始字节（STool 的「强制写入」会保留撤销记录）。".into(),
                "若这条指令同时也负责正常逻辑（比如每帧刷新血条），NOP 掉可能让数值不再更新，改成立即数更稳。".into(),
            ],
            auto: None,
        });
    } else if inp.verdict.is_reverted() {
        plans.push(Plan {
            title: "代码补丁：用现成工具定位还原指令".into(),
            feasibility: Feasibility::Medium,
            detail: "STool 只扫了「立即数写入」这一种形式，没找到落点。还有更强的定位手段可用。"
                .into(),
            steps: vec![
                "用 Cheat Engine 的「找出是什么改写了这个地址」下硬件断点，拿到指令地址。".into(),
                "拿到地址后把它改成写入你要的值，或改成 NOP。".into(),
            ],
            auto: None,
        });
    }

    // 6) 稳定：问题不在保护
    if !inp.verdict.is_reverted() && inp.rewrite.is_none() && !inp.page_blocked {
        plans.push(Plan {
            title: "内存层面没被还原：问题在别处".into(),
            feasibility: Feasibility::High,
            detail: "写入被完整保留了，说明没有保护在还原它。那「改了没用」通常是下面几种情况。"
                .into(),
            steps: vec![
                "地址不对：那是「显示值的副本」而非游戏真正读取的那一份 —— 用「再次扫描」重新收敛。".into(),
                "需要刷新：切一次界面 / 退出菜单 / 重新打开数值面板才会重新读取。".into(),
                "值被用来推导别的量：改了它但界面显示的是推导结果（本报告若检出改写关系会指出）。".into(),
            ],
            auto: None,
        });
    }

    // 7) 兜底：替代路线（几乎所有情况都值得作为备选）
    plans.push(Plan {
        title: "替代路线：改存档 / 改脚本 / 引擎调试接口".into(),
        feasibility: Feasibility::High,
        detail: "内存里的数值大多来自存档或脚本变量。改「源头」比改内存稳定得多：不会掉、不受校验影响。"
            .into(),
        steps: vec![
            "存档类：用 STool 的「存档」页搜索数值并直接改写（改前自动备份）。".into(),
            "脚本类：解包后改脚本/数据文件，再用补丁包回写（KiriKiri 用 patchN.xp3，删掉即还原）。".into(),
            "RPG Maker MV/MZ：用「运行时修改」里的调试协议直接改游戏变量/开关/物品，比扫内存可靠。".into(),
        ],
        auto: None,
    });

    // 排序：可行性高的在前，但「不建议」的保持原位（它在最前面起警示作用）
    let mut head: Vec<Plan> = Vec::new();
    let mut rest: Vec<Plan> = Vec::new();
    for p in plans {
        if p.feasibility == Feasibility::NotAdvised {
            head.push(p);
        } else {
            rest.push(p);
        }
    }
    rest.sort_by_key(|p| Reverse(p.feasibility));
    head.extend(rest);
    head
}

// ---------------------------------------------------------------------------
// 纯逻辑：页保护汇总
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionStat {
    pub base: usize,
    pub size: usize,
    pub protect: u32,
    pub kind: u32,
    pub merged: usize,
}

/// 把相邻且保护/类型相同的区域合并，便于人看。
pub fn region_summary(regions: &[Region]) -> (Vec<RegionStat>, PageSummary) {
    let mut stats: Vec<RegionStat> = Vec::new();
    let mut sum = PageSummary::default();
    for r in regions {
        if r.state != memapi::MEM_COMMIT || r.size == 0 {
            continue;
        }
        sum.committed += 1;
        sum.total_bytes = sum.total_bytes.saturating_add(r.size);
        let base_p = memapi::protect_base(r.protect);
        if memapi::is_directly_writable(r.protect) {
            sum.writable += 1;
        } else {
            sum.readonly += 1;
        }
        if r.protect & memapi::PAGE_GUARD != 0 {
            sum.guard += 1;
        }
        if memapi::is_executable(r.protect) {
            sum.execute += 1;
            if memapi::is_directly_writable(r.protect) {
                sum.rwx += 1;
            }
        }
        match r.kind {
            memapi::MEM_IMAGE => sum.image += 1,
            memapi::MEM_MAPPED => sum.mapped += 1,
            _ => sum.private += 1,
        }
        let mergeable = stats.last().is_some_and(|s| {
            let end = s.base.saturating_add(s.size.saturating_mul(s.merged));
            end == r.base && memapi::protect_base(s.protect) == base_p && s.kind == r.kind
        });
        if mergeable {
            if let Some(last) = stats.last_mut() {
                last.merged += 1;
                last.size = last.size.max(r.size);
            }
        } else {
            stats.push(RegionStat { base: r.base, size: r.size, protect: r.protect, kind: r.kind, merged: 1 });
        }
    }
    (stats, sum)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageSummary {
    pub committed: usize,
    pub total_bytes: usize,
    pub writable: usize,
    pub readonly: usize,
    pub execute: usize,
    /// 可写又可执行的页数 —— 这类页很可疑（正常程序几乎没有）。
    pub rwx: usize,
    pub guard: usize,
    pub image: usize,
    pub mapped: usize,
    pub private: usize,
}

// ---------------------------------------------------------------------------
// PE 导入表（远程读取）
// ---------------------------------------------------------------------------

fn rd_u16(b: &[u8], off: usize) -> Option<u16> {
    let s = b.get(off..off + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

fn rd_u32(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

/// 解析一个已加载模块的导入表，返回 (导入的 DLL 名, 导入的函数名)。
///
/// `read(addr, size)` 由调用方提供（本地测试给一个切片读取器即可）。
/// 已加载模块的 RVA 就等于「相对模块基址的偏移」，所以不需要再走节表换算。
pub fn parse_imports<F>(read: F) -> (Vec<String>, Vec<String>)
where
    F: Fn(usize, usize) -> Option<Vec<u8>>,
{
    const MAX_DESCRIPTORS: usize = 96;
    const MAX_FUNCS_PER_DLL: usize = 128;
    const MAX_FUNCS: usize = 768;
    const MAX_NAME: usize = 256;

    let mut dlls = Vec::new();
    let mut funcs = Vec::new();

    // 读字符串时逐级缩小读取长度：名字可能贴在区域末尾，一次读 256 字节会失败。
    let read_str = |addr: usize| -> String {
        for n in [MAX_NAME, 64, 16] {
            if let Some(b) = read(addr, n) {
                let s = cstr(&b);
                if !s.is_empty() {
                    return s;
                }
            }
        }
        String::new()
    };

    let Some(hdr) = read(0, 0x400) else { return (dlls, funcs) };
    let Some(sig) = hdr.get(0..2) else { return (dlls, funcs) };
    if sig != b"MZ".as_slice() {
        return (dlls, funcs);
    }
    let Some(e_lfanew) = rd_u32(&hdr, 0x3C).map(|v| v as usize) else { return (dlls, funcs) };
    if e_lfanew < 0x40 {
        return (dlls, funcs);
    }
    // PE 签名 + 可选头。头部一般不超过 0x400，e_lfanew 大时再补读一次。
    let hdr = if e_lfanew + 0x120 > hdr.len() {
        match read(e_lfanew, 0x200) {
            Some(h) => h,
            None => return (dlls, funcs),
        }
    } else {
        hdr[e_lfanew..].to_vec()
    };
    if hdr.get(0..4) != Some(b"PE\0\0".as_slice()) {
        return (dlls, funcs);
    }
    let opt = 24usize;
    let Some(magic) = rd_u16(&hdr, opt) else { return (dlls, funcs) };
    let pe32plus = magic == 0x20B;
    if magic != 0x10B && !pe32plus {
        return (dlls, funcs);
    }
    let dd = opt + if pe32plus { 112 } else { 96 };
    // 数据目录[1] = 导入表
    let Some(imp_rva) = rd_u32(&hdr, dd + 8) else { return (dlls, funcs) };
    if imp_rva == 0 {
        return (dlls, funcs);
    }
    let thunk_size = if pe32plus { 8usize } else { 4 };
    let thunk_high = if pe32plus { 1u64 << 63 } else { 1u64 << 31 };

    for i in 0..MAX_DESCRIPTORS {
        let Some(desc) = read(imp_rva as usize + i * 20, 20) else { break };
        if desc.len() < 20 {
            break;
        }
        let oft = rd_u32(&desc, 0).unwrap_or(0);
        let name_rva = rd_u32(&desc, 12).unwrap_or(0);
        let first_thunk = rd_u32(&desc, 16).unwrap_or(0);
        if oft == 0 && name_rva == 0 && first_thunk == 0 {
            break;
        }
        if name_rva != 0 {
            let n = read_str(name_rva as usize);
            if !n.is_empty() {
                dlls.push(n);
            }
        }
        let thunk_rva = if oft != 0 { oft } else { first_thunk };
        if thunk_rva == 0 {
            continue;
        }
        for k in 0..MAX_FUNCS_PER_DLL {
            if funcs.len() >= MAX_FUNCS {
                break;
            }
            let Some(tb) = read(thunk_rva as usize + k * thunk_size, thunk_size) else { break };
            let entry = if pe32plus {
                let Some(s) = tb.get(0..8) else { break };
                u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
            } else {
                let Some(s) = tb.get(0..4) else { break };
                u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64
            };
            if entry == 0 {
                break;
            }
            if entry & thunk_high != 0 {
                continue; // 按序号导入，没有名字
            }
            // IMAGE_IMPORT_BY_NAME：u16 hint + 名字
            let n = read_str(entry as usize + 2);
            if !n.is_empty() {
                funcs.push(n);
            }
        }
    }
    (dlls, funcs)
}

/// 读取目标进程主模块（+ 同目录下体积最大的几个模块）的导入函数名。
fn read_imports_of(proc: &Proc, modules: &[memapi::ModuleInfo], max_modules: usize) -> Vec<String> {
    let mut out = Vec::new();
    // 主模块一般是第一个；其余挑体积大的（游戏逻辑多在自己写的 DLL 里）
    let mut cands: Vec<&memapi::ModuleInfo> = modules.iter().collect();
    cands.sort_by_key(|m| std::cmp::Reverse(m.size));
    let mut picked: Vec<&memapi::ModuleInfo> = Vec::new();
    if let Some(first) = modules.first() {
        picked.push(first);
    }
    for m in cands {
        if picked.len() >= max_modules {
            break;
        }
        if picked.iter().any(|p| p.base == m.base) {
            continue;
        }
        picked.push(m);
    }
    let h = proc.raw();
    for m in picked {
        let base = m.base;
        let (_, funcs) = parse_imports(|off, size| memapi::read_raw(h, base + off, size));
        out.extend(funcs);
    }
    out.sort();
    out.dedup();
    out
}

// ---------------------------------------------------------------------------
// 探测实现
// ---------------------------------------------------------------------------

/// 从同值副本里挑出「可安全试写」的候选，最多 `max` 个（保持原有先后顺序）。
///
/// 返回 `(候选, 被跳过的个数)`。调用方给的判据是「页保护允许直接写入**且**不可执行」：
/// 只读副本（代码段 / 常量 / 映像数据）几乎不可能是游戏变量，而**可执行且可写**的页
/// （JIT / 自修改代码）往里面写探测值风险太大 —— 两类都在反测前直接排除。
fn pick_writable_mirrors(mirrors: &[usize], max: usize, writable: impl Fn(usize) -> bool) -> (Vec<usize>, usize) {
    let mut picked = Vec::new();
    let mut skipped = 0usize;
    for m in mirrors {
        if picked.len() >= max {
            break;
        }
        if writable(*m) {
            picked.push(*m);
        } else {
            skipped += 1;
        }
    }
    (picked, skipped)
}

/// 找同值副本：在可写内存里搜索 `needle` 字节序列。返回 (地址列表, 是否因超限截断)。
fn scan_mirrors(proc: &Proc, needle: &[u8], exclude: usize, cap: usize) -> (Vec<usize>, bool) {
    let mut out = Vec::new();
    let mut scanned = 0usize;
    let mut truncated = false;
    if needle.is_empty() {
        return (out, false);
    }
    let h = proc.raw();
    let first = needle[0];
    for reg in proc.regions() {
        if out.len() >= cap || scanned >= MIRROR_MAX_BYTES {
            truncated = true;
            break;
        }
        if !reg.is_scannable_writable(MIRROR_MAX_REGION) {
            continue;
        }
        let Some(buf) = memapi::read_raw(h, reg.base, reg.size) else { continue };
        scanned += buf.len();
        let n = needle.len();
        if buf.len() < n {
            continue;
        }
        for i in 0..=(buf.len() - n) {
            if buf[i] != first {
                continue;
            }
            if &buf[i..i + n] != needle {
                continue;
            }
            let addr = reg.base + i;
            if addr == exclude {
                continue;
            }
            out.push(addr);
            if out.len() >= cap {
                truncated = true;
                break;
            }
        }
    }
    (out, truncated)
}

/// 诊断报告。
#[derive(Debug, Clone)]
pub struct ProbeReport {
    pub pid: u32,
    pub addr: usize,
    pub ty: ScanType,
    pub is_32bit: bool,
    pub original: ScanValue,
    pub probe: ScanValue,
    pub verdict: Verdict,
    /// 每轮写入的存活时间（微秒）；0 = 该轮一直存活到窗口结束。
    pub survivals_us: Vec<u64>,
    /// 回滚后读到的值（第一次回滚时）。
    pub restored: Option<ScanValue>,
    pub restored_is_original: bool,
    pub rewrite: Option<Rewrite>,
    pub page: Option<Region>,
    pub page_blocked: bool,
    pub mirrors: Vec<usize>,
    pub mirror_truncated: bool,
    pub source_addr: Option<usize>,
    pub ac_modules: Vec<ModuleHit>,
    pub prot_modules: Vec<ModuleHit>,
    pub anti_debug: Vec<ImportHit>,
    pub import_funcs: usize,
    pub module_count: usize,
    pub writers: Vec<CodeHit>,
    pub const_hits: Vec<CodeHit>,
    pub code_bytes: usize,
    pub code_truncated: bool,
    pub read_only: bool,
    pub notes: Vec<String>,
    pub plans: Vec<Plan>,
}

impl ProbeReport {
    /// 一句话结论。
    pub fn headline(&self) -> String {
        if !self.ac_modules.is_empty() {
            return format!("发现在线反作弊（{}）—— 不建议修改，本工具不提供绕过。", self.ac_modules.iter().map(|m| m.name.clone()).collect::<Vec<_>>().join("、"));
        }
        if self.page_blocked {
            return format!("该地址所在页不可写（{}）—— 用「强制写入」可解决。", self.page.map(|p| p.protect_name()).unwrap_or("未知"));
        }
        match &self.verdict {
            Verdict::Stable if self.rewrite.is_none() => {
                "内存层面没有被还原：改动是生效的，「改了没用」来自别处（地址不对 / 未刷新 / 值由别处推导）。".into()
            }
            Verdict::Stable => "写入会被引擎改写成别的值：该地址是派生/缓存副本，写它无效。".into(),
            v if recommended_period_ms(v).is_none() => format!(
                "{} —— 属于强保护，锁值无效，需要代码补丁或改用替代路线。",
                v.measure()
            ),
            v => format!(
                "{} —— 用自适应锁值（周期 {} 毫秒）可稳定住。",
                v.measure(),
                recommended_period_ms(v).unwrap_or(1)
            ),
        }
    }

    /// 纯文本报告（CLI 直接打印，GUI 也可复用）。
    pub fn lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        v.push(format!(
            "对象: 进程 {}（{} 位）· 地址 {} · 类型 {}",
            self.pid,
            if self.is_32bit { "32" } else { "64" },
            fmt_addr(self.addr),
            self.ty.label()
        ));
        if self.read_only {
            v.push("⚠ 只读模式：无法打开可写句柄，未做写入探测（请以管理员身份运行后重试）。".into());
        }
        v.push(format!("原值: {} · 探测值: {}", self.original.display(), self.probe.display()));
        v.push(String::new());
        v.push(format!("【结论】{}", self.headline()));
        v.push(format!("【回滚判定】{}", self.verdict.label()));
        if !self.survivals_us.is_empty() {
            let shown: Vec<String> = self
                .survivals_us
                .iter()
                .map(|t| if *t == 0 { "存活满窗口".to_string() } else { format!("{:.2}ms", *t as f64 / 1000.0) })
                .collect();
            v.push(format!("　每轮存活: {}", shown.join(" / ")));
        }
        if let Some(r) = &self.restored {
            v.push(format!(
                "　被改回为: {}{}",
                r.display(),
                if self.restored_is_original { "（= 原值，即被还原）" } else { "（≠ 原值，是被别处覆盖）" }
            ));
        }
        if let Some(rw) = &self.rewrite {
            v.push(format!("【写入被改写】{}", rw.describe()));
        }

        v.push(String::new());
        match &self.page {
            Some(p) => v.push(format!(
                "【页保护】{}（{}，{} 字节）{}",
                p.protect_name(),
                p.kind_name(),
                p.size,
                if self.page_blocked { "→ 不可直接写，需强制写入" } else { "" }
            )),
            None => v.push("【页保护】地址不在已提交区域（可能已失效，请重新扫描）".into()),
        }

        v.push(format!(
            "【同值副本】{} 个{}",
            self.mirrors.len(),
            if self.mirror_truncated { "（已截断）" } else { "" }
        ));
        if let Some(src) = self.source_addr {
            v.push(format!("　数据源锁定: {}（写它会带动目标地址跟着变）", fmt_addr(src)));
        }

        if !self.ac_modules.is_empty() || !self.prot_modules.is_empty() {
            v.push(String::new());
            v.push(format!("【保护机制线索】共加载 {} 个模块", self.module_count));
            for m in self.ac_modules.iter().chain(self.prot_modules.iter()) {
                v.push(format!("　[{}] {} ← 模块 {}", m.family.label(), m.name, m.module));
                v.push(format!("　　　{}", m.advice));
            }
        }
        if !self.anti_debug.is_empty() {
            v.push(format!(
                "【反调试线索】扫描 {} 个导入函数，命中 {} 条",
                self.import_funcs,
                self.anti_debug.len()
            ));
            v.push(
                "　（低置信度：这几个 API 在普通程序里也常被 CRT / 运行库导入，只有配合\
                 「已加载模块里有加壳 / DRM」或「写入被立即还原」才值得当真）"
                    .into(),
            );
            for h in self.anti_debug.iter().take(8) {
                v.push(format!("　{} — {}", h.func, h.why));
            }
        }

        // 常量写入点噪声很大（普通程序里到处都在写 0/100 这类常见值），
        // 只有在「确实被还原」时才有参考价值，所以只在这个前提下展示。
        let show_consts = self.verdict.is_reverted() && !self.const_hits.is_empty();
        if !self.writers.is_empty() || show_consts {
            v.push(String::new());
            v.push(format!(
                "【代码写入点】已扫可执行区 {}",
                crate::features::precheck::human_bytes(self.code_bytes as u64)
            ));
            if self.writers.is_empty() {
                v.push("　没找到「写到本地址」的立即数指令；下面列出的是「写同一个常量」的候选。".into());
            }
            for w in self.writers.iter().take(8) {
                v.push(format!("　★ 写到本地址的指令 {} —— {}", fmt_addr(w.addr), w.describe()));
            }
            if show_consts {
                v.push(format!(
                    "　· 另有 {} 处指令在写常量 {}（仅作候选：普通程序里到处都在写常见数值，需人工确认）",
                    self.const_hits.len(),
                    self.const_hits.first().map(|c| c.imm).unwrap_or(0)
                ));
                for c in self.const_hits.iter().take(4) {
                    v.push(format!("　　{} —— {}", fmt_addr(c.addr), c.describe()));
                }
            }
            if self.code_truncated {
                v.push("　（可执行区过大，已截断；未覆盖的部分可能还有落点）".into());
            }
        } else if self.code_bytes > 0 {
            v.push(String::new());
            v.push(format!(
                "【代码写入点】已扫可执行区 {}，未找到「立即数写入」形式的落点（还原也可能由寄存器寻址或外部线程完成）",
                crate::features::precheck::human_bytes(self.code_bytes as u64)
            ));
        }

        for n in &self.notes {
            v.push(format!("注: {n}"));
        }

        v.push(String::new());
        v.push("【应对方案】按可行性排序".into());
        for (i, p) in self.plans.iter().enumerate() {
            v.push(format!("{}. [{}] {}", i + 1, p.feasibility.label(), p.title));
            v.push(format!("   {}", p.detail));
            for s in &p.steps {
                v.push(format!("   - {s}"));
            }
        }
        v
    }
}

/// 只读诊断：列出目标进程的内存区域汇总。
pub fn regions_of(pid: u32) -> Result<(Vec<RegionStat>, PageSummary), String> {
    let proc = Proc::open(pid)?;
    let regions = proc.regions();
    if regions.is_empty() {
        return Err("没有读到任何内存区域（进程可能已退出，或权限不足）。".into());
    }
    Ok(region_summary(&regions))
}

/// 完整诊断：写入探测 + 页保护 + 保护机制线索 + 代码写入点 + 方案。
pub fn analyze(pid: u32, addr: usize, ty: ScanType, opts: &ProbeOpts) -> Result<ProbeReport, String> {
    if !memscan::is_numeric(ty) {
        return Err(format!("{} 不支持保护探测（只有整数 / 小数类型能做 ± 探测）", ty.label()));
    }
    let (proc, read_only) = match Proc::open(pid) {
        Ok(p) => (p, false),
        Err(rw_err) => match Proc::open_query(pid) {
            Ok(p) => (p, true),
            Err(_) => return Err(rw_err),
        },
    };

    let mut notes: Vec<String> = Vec::new();
    if read_only {
        notes.push("只能只读打开目标进程，未做写入探测与镜像反测。".into());
    }

    // 地址有效性 + 页信息
    let page = proc.region(addr);
    if page.is_none() {
        return Err(format!("{} 不在目标进程的任何已提交区域里。请重新扫描取值（游戏重启后地址通常会变）。", fmt_addr(addr)));
    }
    let page_blocked = page.map(|p| !memapi::is_directly_writable(p.protect)).unwrap_or(false);

    let size = ty.value_size().max(4);
    let h = proc.raw();
    let Some(orig_bytes) = memapi::read_raw(h, addr, size) else {
        return Err(format!("读取 {} 失败（地址已失效？请重新扫描）。", fmt_addr(addr)));
    };
    if orig_bytes.len() < ty.value_size() {
        return Err(format!("{} 处只读到 {} 字节，不足以构成一个 {}（可能压在页尾）。", fmt_addr(addr), orig_bytes.len(), ty.label()));
    }
    let original = memscan::read_value_at(&orig_bytes, ty)
        .ok_or_else(|| "无法把该地址的内容解释成所选类型".to_string())?;

    // 探测值
    let (probe, probe_from_input) = match &opts.probe {
        Some(s) => (memscan::parse_value(ty, s)?, true),
        None => (memscan::auto_probe(ty, &original).ok_or("无法自动构造探测值，请手动填写")?, false),
    };
    let probe_bytes = memscan::pattern_bytes_of(ty, &probe).ok_or("探测值无法编码")?;
    let orig_pattern = memscan::pattern_u64(ty, &original);
    let probe_pattern = memscan::pattern_u64(ty, &probe);

    let mut survivals: Vec<u64> = Vec::new();
    let mut restored: Option<ScanValue> = None;
    let mut rewrite: Option<Rewrite> = None;

    if !read_only {
        // --- 1) 改写判定：两次不同写入，各自立刻读回，用差分判断存在什么关系 ---
        let second = memscan::auto_probe(ty, &probe)
            .and_then(|v| memscan::pattern_bytes_of(ty, &v))
            .unwrap_or_else(|| {
                // 兜底：翻掉最后一个字节，保证与 probe 不同
                let mut b = probe_bytes.clone();
                if let Some(last) = b.last_mut() {
                    *last ^= 0x5A;
                }
                b
            });
        if memapi::write_raw(h, addr, &probe_bytes) {
            let r1 = memapi::read_raw(h, addr, size).and_then(|b| memscan::bytes_u64(&b));
            let wrote2 = memapi::write_raw(h, addr, &second);
            let r2 = if wrote2 {
                memapi::read_raw(h, addr, size).and_then(|b| memscan::bytes_u64(&b))
            } else {
                None
            };
            if let (Some(w1), Some(r1), Some(w2), Some(r2)) = (probe_pattern, r1, memscan::bytes_u64(&second), r2) {
                rewrite = classify_rewrite(w1, r1, w2, r2);
                // Constant 分支拿到的只是定长 8 字节视图，换成对应类型的展示值
                if matches!(rewrite, Some(Rewrite::Constant { .. })) {
                    let fb = memapi::read_raw(h, addr, size).unwrap_or_default();
                    if let Some(v) = memscan::read_value_at(&fb, ty) {
                        rewrite = Some(Rewrite::Constant { fixed: v });
                    }
                }
            }
            memapi::force_write_raw(h, addr, &orig_bytes).ok();
        } else {
            notes.push("普通写入被拒绝（页不可写）；请用「强制写入」。".into());
        }

        // --- 2) 存活探测：反复写入，测每次能被保持多久 ---
        let window = Duration::from_millis(opts.window_ms.clamp(20, 3000));
        let rounds = opts.rounds.clamp(1, 20);
        let total_start = Instant::now();
        let mut samples: u64 = 0;
        for _ in 0..rounds {
            if total_start.elapsed() > Duration::from_millis(opts.window_ms.clamp(20, 3000) * 6) {
                break;
            }
            if !memapi::write_raw(h, addr, &probe_bytes)
                && memapi::force_write_raw(h, addr, &probe_bytes).is_err()
            {
                notes.push("写入探测值失败，存活探测无法进行。".into());
                break;
            }
            let t0 = Instant::now();
            let mut survived = true;
            while t0.elapsed() < window {
                samples += 1;
                if samples > MAX_SAMPLES {
                    break;
                }
                match memapi::read_raw(h, addr, size) {
                    Some(cur) if cur == probe_bytes => {}
                    Some(cur) => {
                        survived = false;
                        survivals.push(t0.elapsed().as_micros() as u64);
                        if restored.is_none() {
                            restored = memscan::read_value_at(&cur, ty);
                        }
                        break;
                    }
                    None => {
                        notes.push("读取内存失败，探测提前结束。".into());
                        survived = false;
                        break;
                    }
                }
            }
            if survived {
                survivals.push(0);
                break; // 稳定，不用再测
            }
        }
        // 诊断结束，恢复原值（诊断不该留下副作用）
        memapi::force_write_raw(h, addr, &orig_bytes).ok();
    }

    let verdict = if read_only {
        Verdict::Stable
    } else {
        classify_revert(&survivals)
    };

    // --- 3) 同值镜像 + 数据源反测 ---
    let mut mirrors: Vec<usize> = Vec::new();
    let mut mirror_truncated = false;
    let mut source_addr: Option<usize> = None;
    if opts.mirror && !read_only {
        let (m, t) = scan_mirrors(&proc, &orig_bytes, addr, opts.max_mirrors.max(1));
        mirrors = m;
        mirror_truncated = t;
        // 反测数据源：写一个副本 → 看目标地址是否跟着变（变了说明它才是被游戏采纳的「源」）。
        // 只试「可写且不可执行」的副本页 —— 理由见 [`pick_writable_mirrors`]。
        let (candidates, _skipped_ro) = pick_writable_mirrors(&mirrors, MAX_SOURCE_TRIES, |m| {
            proc.region(m)
                .map(|r| memapi::is_directly_writable(r.protect) && !memapi::is_executable(r.protect))
                .unwrap_or(false)
        });
        for m in candidates.iter() {
            if memapi::write_raw(h, *m, &probe_bytes) {
                let changed = memapi::read_raw(h, addr, size).map(|b| b == probe_bytes).unwrap_or(false);
                // 恢复副本（兜底再试一次强制写入，宁可多一次调用也不把探测值留在别人数据上）
                if !memapi::write_raw(h, *m, &orig_bytes) {
                    memapi::force_write_raw(h, *m, &orig_bytes).ok();
                }
                if changed {
                    source_addr = Some(*m);
                    break;
                }
            }
        }
        if source_addr.is_none() && !mirrors.is_empty() {
            if candidates.is_empty() {
                notes.push(format!(
                    "{} 个同值副本都不在可写的普通数据页上（多半是常量 / 代码 / 映像数据），未做写入反测。",
                    mirrors.len()
                ));
            } else if candidates.len() >= MAX_SOURCE_TRIES {
                notes.push(format!(
                    "同值副本有 {} 个，只反测了前 {MAX_SOURCE_TRIES} 个可写的。",
                    mirrors.len()
                ));
            }
        }
        // 反测可能改动了目标地址，恢复
        memapi::force_write_raw(h, addr, &orig_bytes).ok();
    }

    // --- 4) 模块与导入表 ---
    let modules = proc.modules();
    let names: Vec<String> = modules.iter().map(|m| m.name.clone()).collect();
    let hits = classify_modules(&names);
    let ac_modules: Vec<ModuleHit> = hits.iter().filter(|h| h.family == Family::OnlineAntiCheat).cloned().collect();
    let prot_modules: Vec<ModuleHit> = hits.iter().filter(|h| h.family != Family::OnlineAntiCheat).cloned().collect();
    let import_funcs = if modules.is_empty() { Vec::new() } else { read_imports_of(&proc, &modules, 6) };
    let anti_debug = classify_imports(&import_funcs);

    // --- 5) 代码写入点 ---
    let mut writers: Vec<CodeHit> = Vec::new();
    let mut const_hits: Vec<CodeHit> = Vec::new();
    let mut code_bytes = 0usize;
    let mut code_truncated = false;
    if opts.code {
        let is32 = proc.is_32bit();
        let orig_imm = orig_pattern.unwrap_or(0);
        for reg in proc.regions() {
            if code_bytes >= CODE_MAX_BYTES {
                code_truncated = true;
                break;
            }
            if reg.state != memapi::MEM_COMMIT || !memapi::is_executable(reg.protect) || reg.size == 0 || reg.size > CODE_MAX_REGION {
                continue;
            }
            let Some(code) = memapi::read_raw(h, reg.base, reg.size) else { continue };
            code_bytes += code.len();
            let mut w = find_imm_writers(&code, reg.base, addr, is32, 16);
            writers.append(&mut w);
            let mut c = find_imm_constants(&code, reg.base, orig_imm, is32, 16);
            const_hits.append(&mut c);
        }
    }
    writers.sort_by_key(|c| c.addr);
    writers.dedup();
    writers.truncate(16);
    const_hits.sort_by_key(|c| c.addr);
    const_hits.dedup();
    const_hits.truncate(16);

    if !probe_from_input {
        notes.push(format!("探测值是自动构造的（{}）；如需指定请用 --opt:probe=。", probe.display()));
    }
    if mirror_truncated {
        notes.push("同值副本扫描已截断（同值地址太多或内存过大），可能还有未列出的副本。".into());
    }

    // --- 6) 方案 ---
    let ac_names = online_ac_names(&hits);
    let plans = build_plans(&PlanInput {
        verdict: &verdict,
        rewrite: rewrite.as_ref(),
        page_blocked,
        mirrors: mirrors.len() + 1,
        source_addr,
        writer_addr: writers.first().map(|w| w.addr),
        ac_modules: &ac_names,
        read_only,
    });

    let restored_is_original = restored.as_ref() == Some(&original);
    Ok(ProbeReport {
        pid,
        addr,
        ty,
        is_32bit: proc.is_32bit(),
        original,
        probe,
        verdict,
        survivals_us: survivals,
        restored,
        restored_is_original,
        rewrite,
        page,
        page_blocked,
        mirrors,
        mirror_truncated,
        source_addr,
        ac_modules,
        prot_modules,
        anti_debug,
        import_funcs: import_funcs.len(),
        module_count: modules.len(),
        writers,
        const_hits,
        code_bytes,
        code_truncated,
        read_only,
        notes,
        plans,
    })
}

/// 供 GUI 复用的「强制写入」入口（独立打开一次进程句柄）。
pub fn force_write(pid: u32, addr: usize, ty: ScanType, value: &str) -> Result<PageFix, String> {
    let v = memscan::parse_value(ty, value)?;
    let bytes = memscan::pattern_bytes_of(ty, &v).ok_or("值无法编码")?;
    let proc = Proc::open(pid)?;
    proc.force_write(addr, &bytes)
}

#[cfg(test)]
#[path = "guard_tests.rs"]
mod tests;
