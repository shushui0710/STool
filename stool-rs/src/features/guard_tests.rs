//! `guard` 的单元测试（纯逻辑，不需要真实进程）。
//!
//! 单独成文件是为了让 `guard.rs` 主体保持可读。

use super::*;

// ---------------- 地址解析 ----------------

#[test]
fn parse_addr_accepts_hex_dec_and_underscores() {
    assert_eq!(parse_addr("0x1000"), Some(0x1000));
    assert_eq!(parse_addr("0X7FF6A000"), Some(0x7FF6A000));
    assert_eq!(parse_addr("4096"), Some(4096));
    assert_eq!(parse_addr(" 0x_1_000 "), Some(0x1000));
    // 没有 0x 前缀但含 a-f → 当十六进制
    assert_eq!(parse_addr("7ff6a000"), Some(0x7ff6a000));
    // 非法输入不 panic
    assert_eq!(parse_addr(""), None);
    assert_eq!(parse_addr("   "), None);
    assert_eq!(parse_addr("0xzz"), None);
    assert_eq!(parse_addr("99999999999999999999999999"), None);
}

#[test]
fn fmt_addr_is_16_wide_hex() {
    assert_eq!(fmt_addr(0x1234), "0x0000000000001234");
}

// ---------------- 回滚判定 ----------------

#[test]
fn classify_revert_stable_when_never_rewritten() {
    assert_eq!(classify_revert(&[]), Verdict::Stable);
    assert_eq!(classify_revert(&[0]), Verdict::Stable);
    assert_eq!(classify_revert(&[0, 0, 0]), Verdict::Stable);
    assert!(!classify_revert(&[0]).is_reverted());
}

#[test]
fn classify_revert_instant_only_when_every_round_is_sub_ms() {
    assert_eq!(classify_revert(&[120]), Verdict::Instant { after_us: 120 });
    assert_eq!(classify_revert(&[999]), Verdict::Instant { after_us: 999 });
    assert!(classify_revert(&[50]).is_reverted());
    // 1000us 及以上不再算「立即」
    assert!(matches!(classify_revert(&[1000]), Verdict::Delayed { .. }));
    // 有一轮撑到了窗口结束 → 说明还原不是必经路径，不能叫「立即」
    assert!(matches!(classify_revert(&[120, 0]), Verdict::Delayed { .. }));
    // 混合样本里只要出现过 >= 1ms，就不算「立即」
    assert!(matches!(classify_revert(&[120, 1500]), Verdict::Delayed { .. }));
    // 拿最保守（最短）的那次当间隔估计
    match classify_revert(&[900, 400, 1200, 8000]) {
        Verdict::Delayed { after_ms, rounds } => {
            assert!((after_ms - 0.4).abs() < 1e-9, "after_ms={after_ms}");
            assert_eq!(rounds, 4);
        }
        other => panic!("期望 Delayed，实际 {other:?}"),
    }
}

#[test]
fn classify_revert_periodic_when_survivals_are_consistent() {
    let v = classify_revert(&[10_000, 10_200, 9_800, 10_100]);
    match v {
        Verdict::Periodic { period_ms, rounds } => {
            assert!((period_ms - 10.0).abs() < 0.5, "period={period_ms}");
            assert_eq!(rounds, 4);
        }
        other => panic!("期望 Periodic，实际 {other:?}"),
    }
    // 抖动过大 → 退化成 Delayed
    let v = classify_revert(&[2_000, 50_000, 3_000, 90_000]);
    assert!(matches!(v, Verdict::Delayed { .. }), "{v:?}");
    // 样本不足 3 个也不做周期判定
    assert!(matches!(classify_revert(&[5_000, 5_100]), Verdict::Delayed { .. }));
}

#[test]
fn recommended_period_is_third_of_observed_interval() {
    assert_eq!(recommended_period_ms(&Verdict::Stable), None);
    assert_eq!(recommended_period_ms(&Verdict::Instant { after_us: 30 }), None);
    assert_eq!(recommended_period_ms(&Verdict::Periodic { period_ms: 30.0, rounds: 3 }), Some(10));
    assert_eq!(recommended_period_ms(&Verdict::Delayed { after_ms: 90.0, rounds: 2 }), Some(30));
    // 极短周期被夹到 1ms，而不是 0
    assert_eq!(recommended_period_ms(&Verdict::Periodic { period_ms: 1.5, rounds: 3 }), Some(1));
    // 亚毫秒回滚：轮询式锁值必然输 → 不给锁值方案
    assert_eq!(recommended_period_ms(&Verdict::Periodic { period_ms: 0.4, rounds: 3 }), None);
    assert_eq!(recommended_period_ms(&Verdict::Delayed { after_ms: 0.37, rounds: 2 }), None);
}

#[test]
fn sub_millisecond_revert_also_marks_freeze_as_not_advised() {
    // 0.4ms 间隔（不是「立即」，但一样赢不了）
    let v = Verdict::Delayed { after_ms: 0.4, rounds: 2 };
    let plans = build_plans(&plain_input(&v));
    assert!(!plans.iter().any(|p| p.title.contains("自适应锁值")), "{plans:#?}");
    assert!(plans.iter().any(|p| p.feasibility == Feasibility::NotAdvised && p.title.contains("锁值无效")));
    // 报告中要给出实测数值，便于用户核对
    assert!(plans.iter().any(|p| p.detail.contains("0.4")));
}

// ---------------- 改写判定 ----------------

#[test]
fn classify_rewrite_detects_none_xor_shift_constant() {
    // 读回 == 写入 → 没有被改写
    assert_eq!(classify_rewrite(100, 100, 200, 200), None);

    // XOR：r = w ^ 0x5A
    assert_eq!(classify_rewrite(100, 100 ^ 0x5A, 200, 200 ^ 0x5A), Some(Rewrite::Xor { key: 0x5A }));

    // 偏移：r = w + 7
    assert_eq!(classify_rewrite(100, 107, 200, 207), Some(Rewrite::Shift { delta: 7 }));

    // 常量：写什么都读回同一个值
    assert_eq!(classify_rewrite(100, 42, 200, 42), Some(Rewrite::Constant { fixed: ScanValue::Int(42) }));

    // 两次写入对应同一个读回值也算常量
    assert_eq!(classify_rewrite(1, 9, 2, 9), Some(Rewrite::Constant { fixed: ScanValue::Int(9) }));

    // 非线性、不规则 → Opaque（但仍要报出来）
    assert_eq!(classify_rewrite(100, 5, 200, 900), Some(Rewrite::Opaque));
}

#[test]
fn rewrite_describe_mentions_the_relation() {
    assert!(Rewrite::Xor { key: 0x5A }.describe().contains("0x5A"));
    assert!(Rewrite::Shift { delta: -3 }.describe().contains("-3"));
    assert!(Rewrite::Constant { fixed: ScanValue::Float(1.5) }.describe().contains("1.5"));
    assert!(!Rewrite::Opaque.describe().is_empty());
}

// ---------------- 立即数写入指令扫描 ----------------

/// 造一条 x64 `C7 05 <disp32> <imm32>`（写 4 字节）。
fn x64_imm_write(disp: i32, imm: u32) -> Vec<u8> {
    let mut v = vec![0xC7u8, 0x05];
    v.extend_from_slice(&disp.to_le_bytes());
    v.extend_from_slice(&imm.to_le_bytes());
    v
}

#[test]
fn finds_x64_rip_relative_imm_writer() {
    let base = 0x1400_001000usize;
    let mut code = vec![0x90u8; 4]; // 前置填充，让指令不在区域开头
    code.extend_from_slice(&x64_imm_write(0x20, 777));
    code.extend_from_slice(&[0x90; 8]);
    // 指令从 base+4 开始，长度 10 → RIP = base+4+10
    let target = base + 4 + 10 + 0x20;
    let hits = find_imm_writers(&code, base, target, false, 8);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].addr, base + 4);
    assert_eq!(hits[0].len, 10);
    assert_eq!(hits[0].size, 4);
    assert_eq!(hits[0].imm, 777);
    assert_eq!(hits[0].target, Some(target));
    // 换一个目标地址就找不到
    assert!(find_imm_writers(&code, base, target + 4, false, 8).is_empty());
}

#[test]
fn finds_x86_32_absolute_imm_writer() {
    let base = 0x0040_0000usize;
    let mut code = x64_imm_write(0x401000, 1234);
    code.extend_from_slice(&[0x90; 6]);
    // 32 位下 disp32 就是绝对地址
    let hits = find_imm_writers(&code, base, 0x401000, true, 8);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].addr, base);
    assert_eq!(hits[0].imm, 1234);
}

#[test]
fn rex_w_form_writes_eight_bytes() {
    let base = 0x1400_002000usize;
    let mut code = vec![0x48u8, 0xC7, 0x05];
    code.extend_from_slice(&0x30i32.to_le_bytes());
    code.extend_from_slice(&55u32.to_le_bytes());
    let target = base + 11 + 0x30; // 指令长度 11
    let hits = find_imm_writers(&code, base, target, false, 8);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].size, 8, "REX.W 应识别成 8 字节写入");
    // 32 位进程里没有 REX，不应把 0x48 当 REX 用（会当成普通 C7 解码）
    let hits32 = find_imm_writers(&code, base, target, true, 8);
    assert!(hits32.iter().all(|h| h.size == 4));
}

#[test]
fn imm_with_register_base_has_no_target_but_still_reports_constant() {
    let base = 0x1000usize;
    // C7 45 10 <imm32> = mov dword [rbp+0x10], imm
    let mut code = vec![0xC7u8, 0x45, 0x10];
    code.extend_from_slice(&4242u32.to_le_bytes());
    assert!(find_imm_writers(&code, base, 0xDEAD, false, 8).is_empty());
    let hits = find_imm_constants(&code, base, 4242, false, 8);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].addr, base);
    assert_eq!(hits[0].target, None);
    assert!(hits[0].describe().contains("4242"));
}

#[test]
fn imm_constants_match_low_32_bits_only() {
    let base = 0x2000usize;
    let mut code = x64_imm_write(0, 500);
    code.extend_from_slice(&[0x90; 4]);
    // 8 字节值的低 32 位等于 500 → 仍应命中（指令里只有 32 位立即数）
    assert_eq!(find_imm_constants(&code, base, 500, false, 8).len(), 1);
    assert_eq!(find_imm_constants(&code, base, (1u64 << 32) | 500, false, 8).len(), 1);
    assert!(find_imm_constants(&code, base, 501, false, 8).is_empty());
}

#[test]
fn imm_write_scan_never_panics_on_arbitrary_bytes() {
    // 覆盖所有单字节、所有双字节组合、所有 ModRM 值
    for b in 0u8..=255 {
        for_each_imm_write(&[b], 0x1000, false, |_| {});
        for_each_imm_write(&[0xC7, b], 0x1000, false, |_| {});
        for_each_imm_write(&[0x48, 0xC7, b], 0x1000, false, |_| {});
        for_each_imm_write(&[0xC7, b, 1, 2, 3, 4, 5, 6, 7, 8, 9], 0x1000, true, |_| {});
    }
    // 截断的指令（缺后半个立即数）不应 panic
    for_each_imm_write(&[0xC7, 0x05, 1, 2, 3], 0x1000, false, |_| {});
    for_each_imm_write(&[0x48, 0xC7], 0x1000, false, |_| {});
    for_each_imm_write(&[], 0x1000, false, |_| {});
}

// ---------------- 保护机制名单 ----------------

#[test]
fn module_names_map_to_known_protections() {
    let names = vec![
        "EasyAntiCheat_x64.dll".to_string(),
        "BEService.exe".to_string(),
        "VMProtectSDK64.dll".to_string(),
        "steam_api64.dll".to_string(),
        "kernel32.dll".to_string(),
    ];
    let hits = classify_modules(&names);
    assert!(hits.iter().any(|h| h.family == Family::OnlineAntiCheat && h.name.contains("Easy Anti-Cheat")));
    assert!(hits.iter().any(|h| h.family == Family::OnlineAntiCheat && h.name == "BattlEye"));
    assert!(hits.iter().any(|h| h.family == Family::Packer && h.name == "VMProtect"));
    assert!(hits.iter().any(|h| h.family == Family::LocalDrm));
    // kernel32 不该误报
    assert!(!hits.iter().any(|h| h.module.eq_ignore_ascii_case("kernel32.dll")));
}

#[test]
fn module_hits_are_deduped_per_protection() {
    let names = vec![
        "EasyAntiCheat_x64.dll".to_string(),
        "EasyAntiCheat.dll".to_string(),
        "BEClient_x64.dll".to_string(),
        "BEService.exe".to_string(),
    ];
    let hits = classify_modules(&names);
    let eac = hits.iter().filter(|h| h.name.contains("Easy Anti-Cheat")).count();
    assert_eq!(eac, 1, "同一机制只应报一次: {hits:?}");
    assert_eq!(hits.iter().filter(|h| h.name == "BattlEye").count(), 1);
    let ac = online_ac_names(&hits);
    assert_eq!(ac.len(), 2, "{ac:?}");
}

#[test]
fn clean_process_reports_nothing() {
    let names = vec!["game.exe".to_string(), "user32.dll".to_string(), "d3d11.dll".to_string()];
    assert!(classify_modules(&names).is_empty());
    assert!(classify_imports(&names).is_empty());
}

#[test]
fn anti_debug_imports_are_matched_case_insensitively() {
    let funcs = vec![
        "IsDebuggerPresent".to_string(),
        "NtQueryInformationProcess".to_string(),
        "CreateFileW".to_string(),
        "OutputDebugStringW".to_string(),
    ];
    let hits = classify_imports(&funcs);
    assert_eq!(hits.len(), 3, "{hits:?}");
    assert!(hits.iter().any(|h| h.func.eq_ignore_ascii_case("IsDebuggerPresent")));
    assert!(!hits.iter().any(|h| h.func == "CreateFileW"));
    // 全大写也能命中
    assert_eq!(classify_imports(&["CHECKREMOTEDEBUGGERPRESENT".to_string()]).len(), 1);
}

// ---------------- 方案生成 ----------------

fn verdicts() -> Vec<Verdict> {
    vec![
        Verdict::Stable,
        Verdict::Instant { after_us: 40 },
        Verdict::Periodic { period_ms: 30.0, rounds: 4 },
        Verdict::Delayed { after_ms: 250.0, rounds: 2 },
    ]
}

fn plain_input(v: &Verdict) -> PlanInput<'_> {
    PlanInput {
        verdict: v,
        rewrite: None,
        page_blocked: false,
        mirrors: 1,
        source_addr: None,
        writer_addr: None,
        ac_modules: &[],
        read_only: false,
    }
}

#[test]
fn plans_always_offer_a_fallback_route() {
    for v in verdicts() {
        let plans = build_plans(&plain_input(&v));
        assert!(!plans.is_empty(), "{v:?} 应至少给一条方案");
        assert!(plans.iter().any(|p| p.title.contains("替代路线")), "{v:?} 缺兜底路线");
        assert!(plans.iter().all(|p| !p.detail.is_empty() && !p.title.is_empty()));
    }
}

#[test]
fn instant_revert_marks_freeze_as_not_advised() {
    let v = Verdict::Instant { after_us: 40 };
    let inp = PlanInput { writer_addr: Some(0x1400_1234), ..plain_input(&v) };
    let plans = build_plans(&inp);
    // 不能出现「自适应锁值」方案（那是注定无效的）
    assert!(!plans.iter().any(|p| p.title.contains("自适应锁值")), "{plans:#?}");
    assert!(plans.iter().any(|p| p.feasibility == Feasibility::NotAdvised && p.title.contains("锁值无效")));
    assert!(plans.iter().any(|p| p.title.contains("代码补丁")));
    // 已定位到写入点 → 不再建议「用 CE 找落点」
    assert!(!plans.iter().any(|p| p.title.contains("用现成工具定位")));
}

#[test]
fn periodic_revert_recommends_adaptive_freeze() {
    let v = Verdict::Periodic { period_ms: 30.0, rounds: 4 };
    let plans = build_plans(&plain_input(&v));
    let freeze = plans.iter().find(|p| p.title.contains("自适应锁值")).expect("应有锁值方案");
    assert_eq!(freeze.auto, Some(AutoAction::Freeze { period_ms: 10 }));
    assert_eq!(freeze.feasibility, Feasibility::High);
    // 没有写入点 → 给出「用 CE 定位」的中等方案
    assert!(plans.iter().any(|p| p.title.contains("用现成工具定位")));
}

#[test]
fn stable_write_reports_problem_is_elsewhere() {
    let v = Verdict::Stable;
    let plans = build_plans(&plain_input(&v));
    assert!(plans.iter().any(|p| p.title.contains("内存层面没被还原")));
    assert!(!plans.iter().any(|p| p.title.contains("自适应锁值")));
}

#[test]
fn page_blocked_and_source_addr_produce_high_feasibility_plans() {
    let v = Verdict::Periodic { period_ms: 30.0, rounds: 3 };
    let rw = Rewrite::Xor { key: 0x11 };
    let inp = PlanInput {
        rewrite: Some(&rw),
        page_blocked: true,
        mirrors: 4,
        source_addr: Some(0x2000),
        writer_addr: Some(0x3000),
        ..plain_input(&v)
    };
    let plans = build_plans(&inp);
    assert!(plans.iter().any(|p| p.auto == Some(AutoAction::ForceWrite)));
    assert!(plans.iter().any(|p| p.auto == Some(AutoAction::WriteSource { addr: 0x2000 })));
    assert!(plans.iter().any(|p| p.detail.contains("0x11")));
    assert!(plans.iter().any(|p| p.title.contains("代码补丁")));
}

#[test]
fn too_many_mirrors_advises_converging_first() {
    let v = Verdict::Periodic { period_ms: 30.0, rounds: 3 };
    let inp = PlanInput { mirrors: 500, ..plain_input(&v) };
    let plans = build_plans(&inp);
    assert!(plans.iter().any(|p| p.title.contains("先收敛候选")), "{plans:#?}");
    assert!(plans.iter().any(|p| p.detail.contains("再次扫描")));
    // 少量副本时给的是「逐个试写」的中等方案
    let inp2 = PlanInput { mirrors: 4, ..plain_input(&v) };
    let plans2 = build_plans(&inp2);
    assert!(plans2.iter().any(|p| p.title.contains("逐个试写")));
    assert!(!plans2.iter().any(|p| p.title.contains("先收敛候选")));
}

#[test]
fn online_anticheat_is_first_and_not_advised() {
    let v = Verdict::Periodic { period_ms: 30.0, rounds: 3 };
    let ac = vec!["Easy Anti-Cheat (EAC)".to_string()];
    let inp = PlanInput { ac_modules: &ac, ..plain_input(&v) };
    let plans = build_plans(&inp);
    assert_eq!(plans[0].feasibility, Feasibility::NotAdvised);
    assert!(plans[0].title.contains("不建议修改"));
    assert!(plans[0].detail.contains("Easy Anti-Cheat"));
    // 「不建议」永远排在「可行性高」前面
    let first_high = plans.iter().position(|p| p.feasibility == Feasibility::High).unwrap_or(usize::MAX);
    assert!(plans.iter().enumerate().all(|(i, p)| p.feasibility != Feasibility::NotAdvised || i < first_high));
    // 其余方案按可行性降序
    let feas: Vec<Feasibility> =
        plans.iter().filter(|p| p.feasibility != Feasibility::NotAdvised).map(|p| p.feasibility).collect();
    assert!(feas.windows(2).all(|w| w[0] >= w[1]), "{feas:?}");
}

#[test]
fn read_only_mode_tells_user_to_run_as_admin() {
    let v = Verdict::Stable;
    let inp = PlanInput { read_only: true, mirrors: 0, ..plain_input(&v) };
    let plans = build_plans(&inp);
    assert!(plans.iter().any(|p| p.title.contains("管理员")));
}

// ---------------- 页保护汇总 ----------------

#[test]
fn region_summary_merges_adjacent_and_counts() {
    use memapi::{Region, MEM_IMAGE, MEM_PRIVATE, PAGE_EXECUTE_READ, PAGE_GUARD, PAGE_READONLY, PAGE_READWRITE};
    let rw = Region { base: 0x1000, size: 0x1000, protect: PAGE_READWRITE, state: memapi::MEM_COMMIT, kind: MEM_PRIVATE };
    let regions = vec![
        rw,
        Region { base: 0x2000, ..rw },
        Region { base: 0x3000, ..rw },
        Region { base: 0x4000, size: 0x2000, protect: PAGE_EXECUTE_READ, state: memapi::MEM_COMMIT, kind: MEM_IMAGE },
        Region { base: 0x6000, size: 0x1000, protect: PAGE_READONLY | PAGE_GUARD, state: memapi::MEM_COMMIT, kind: MEM_PRIVATE },
        // 未提交的区域应被忽略
        Region { base: 0x7000, size: 0x1000, protect: PAGE_READWRITE, state: memapi::MEM_RESERVE, kind: MEM_PRIVATE },
    ];
    let (stats, sum) = region_summary(&regions);
    assert_eq!(sum.committed, 5, "保留区不算已提交");
    assert_eq!(sum.writable, 3);
    assert_eq!(sum.readonly, 2);
    assert_eq!(sum.execute, 1);
    assert_eq!(sum.guard, 1);
    assert_eq!(sum.image, 1);
    assert_eq!(sum.private, 4);
    assert_eq!(sum.total_bytes, 0x1000 * 4 + 0x2000);
    // 三个连续的 PAGE_READWRITE 私有区合并成一条
    assert_eq!(stats.len(), 3, "{stats:#?}");
    assert_eq!(stats[0].base, 0x1000);
    assert_eq!(stats[0].merged, 3);
}

#[test]
fn region_summary_handles_empty_and_huge() {
    let (stats, sum) = region_summary(&[]);
    assert!(stats.is_empty());
    assert_eq!(sum, PageSummary::default());
    // 巨型区域不应在累加时溢出 panic
    let big = memapi::Region {
        base: 0,
        size: usize::MAX / 2,
        protect: memapi::PAGE_READWRITE,
        state: memapi::MEM_COMMIT,
        kind: memapi::MEM_PRIVATE,
    };
    let (_, sum) = region_summary(&[big, big]);
    assert!(sum.total_bytes > 0);
}

// ---------------- 同值副本的「数据源反测」候选筛选 ----------------

#[test]
fn pick_writable_mirrors_skips_readonly_and_keeps_order() {
    // 0x10(可写) 0x20(只读) 0x30(可写)：只读的被跳过，可写的保持先后顺序
    let mirrors = [0x10usize, 0x20, 0x30];
    let (picked, skipped) = pick_writable_mirrors(&mirrors, 8, |m| m != 0x20);
    assert_eq!(picked, vec![0x10, 0x30]);
    assert_eq!(skipped, 1);
}

#[test]
fn pick_writable_mirrors_caps_at_max_writable_ones() {
    // 上限数的是**可写的**副本；走到上限就停，之后的副本不再看（skipped 因此只记前两个只读的）
    let mirrors: Vec<usize> = (1..=10).map(|i| 0x100 * i).collect();
    let (picked, skipped) = pick_writable_mirrors(&mirrors, 3, |m| m >= 0x300);
    assert_eq!(picked, vec![0x300, 0x400, 0x500]);
    assert_eq!(skipped, 2);
}

#[test]
fn pick_writable_mirrors_reports_all_readonly() {
    let mirrors = [0x10usize, 0x20, 0x30];
    let (picked, skipped) = pick_writable_mirrors(&mirrors, 8, |_| false);
    assert!(picked.is_empty());
    assert_eq!(skipped, 3);
}

#[test]
fn pick_writable_mirrors_handles_empty_list() {
    let (picked, skipped) = pick_writable_mirrors(&[], 8, |_| true);
    assert!(picked.is_empty());
    assert_eq!(skipped, 0);
}

// ---------------- PE 导入表解析 ----------------

/// 造一个最小 PE32，导入表里只有 KERNEL32.dll!IsDebuggerPresent。
fn synthetic_pe32() -> Vec<u8> {
    let mut b = vec![0u8; 0x400];
    b[0] = b'M';
    b[1] = b'Z';
    b[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    b[0x40..0x44].copy_from_slice(b"PE\0\0");
    // 可选头在 0x58，magic = PE32
    b[0x58..0x5A].copy_from_slice(&0x10Bu16.to_le_bytes());
    // 数据目录[1]（导入表）在 opt + 96 + 8 = 0xC0
    b[0xC0..0xC4].copy_from_slice(&0x200u32.to_le_bytes());
    // 导入描述符 @0x200：OFT=0x300, Name=0x280, FirstThunk=0x300
    b[0x200..0x204].copy_from_slice(&0x300u32.to_le_bytes());
    b[0x20C..0x210].copy_from_slice(&0x280u32.to_le_bytes());
    b[0x210..0x214].copy_from_slice(&0x300u32.to_le_bytes());
    // 描述符结束（全 0 已经在 0x214..0x228）
    b[0x280..0x28D].copy_from_slice(b"KERNEL32.dll\0");
    // 名字 thunk 数组 @0x300：第一项指向 0x340，第二项 0 = 结束
    b[0x300..0x304].copy_from_slice(&0x340u32.to_le_bytes());
    // IMAGE_IMPORT_BY_NAME @0x340：hint(2) + 名字
    b[0x342..0x354].copy_from_slice(b"IsDebuggerPresent\0");
    b
}

#[test]
fn parse_imports_reads_dll_and_function_names() {
    let pe = synthetic_pe32();
    let read = |off: usize, size: usize| pe.get(off..off + size).map(|s| s.to_vec());
    let (dlls, funcs) = parse_imports(read);
    assert_eq!(dlls, vec!["KERNEL32.dll".to_string()]);
    assert_eq!(funcs, vec!["IsDebuggerPresent".to_string()]);
    // 与反调试名单联动
    assert_eq!(classify_imports(&funcs).len(), 1);
}

#[test]
fn parse_imports_rejects_non_pe_and_truncated_input() {
    let read_of = |buf: Vec<u8>| move |off: usize, size: usize| buf.get(off..off + size).map(|s| s.to_vec());
    // 空 / 太短 / 没有 MZ / MZ 但没有 PE 签名 / 坏 e_lfanew
    let no_sig = {
        let mut v = vec![0u8; 0x400];
        v[0] = b'M';
        v[1] = b'Z';
        v[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        v
    };
    let bad_lfanew = {
        let mut v = vec![0u8; 0x400];
        v[0] = b'M';
        v[1] = b'Z';
        v[0x3C..0x40].copy_from_slice(&0x10u32.to_le_bytes());
        v
    };
    for buf in [vec![], vec![0u8; 4], vec![b'X'; 0x400], no_sig, bad_lfanew] {
        let (d, f) = parse_imports(read_of(buf));
        assert!(d.is_empty() && f.is_empty());
    }
    // 导入表 RVA 指向区域外：读不到描述符就该干净退出
    let mut pe = synthetic_pe32();
    pe[0xC0..0xC4].copy_from_slice(&0xFFFF_0000u32.to_le_bytes());
    let (d, f) = parse_imports(read_of(pe));
    assert!(d.is_empty() && f.is_empty());
}

#[test]
fn parse_imports_never_panics_under_mutation() {
    let base = synthetic_pe32();
    // 逐个字节改坏，只要求不 panic
    for i in 0..base.len() {
        for mask in [0xFFu8, 0x01, 0x80] {
            let mut b = base.clone();
            b[i] ^= mask;
            let read = |off: usize, size: usize| b.get(off..off + size).map(|s| s.to_vec());
            let _ = parse_imports(read);
        }
    }
}

// ---------------- 报告渲染 ----------------

fn sample_report(verdict: Verdict) -> ProbeReport {
    ProbeReport {
        pid: 1234,
        addr: 0x1_0000_2000,
        ty: ScanType::I32,
        is_32bit: false,
        original: ScanValue::Int(100),
        probe: ScanValue::Int(1100),
        verdict,
        survivals_us: vec![12_000, 11_500],
        restored: Some(ScanValue::Int(100)),
        restored_is_original: true,
        rewrite: None,
        page: Some(memapi::Region {
            base: 0x1_0000_0000,
            size: 0x1000,
            protect: memapi::PAGE_READWRITE,
            state: memapi::MEM_COMMIT,
            kind: memapi::MEM_PRIVATE,
        }),
        page_blocked: false,
        mirrors: vec![0x1_0000_3000],
        mirror_truncated: false,
        source_addr: None,
        ac_modules: Vec::new(),
        prot_modules: Vec::new(),
        anti_debug: vec![ImportHit {
            module: String::new(),
            func: "IsDebuggerPresent".into(),
            why: "检测调试器（最常见）",
        }],
        import_funcs: 42,
        module_count: 33,
        writers: vec![CodeHit { addr: 0x1400_0500, len: 10, size: 4, imm: 100, target: Some(0x1_0000_2000) }],
        const_hits: Vec::new(),
        code_bytes: 8 * 1024 * 1024,
        code_truncated: false,
        read_only: false,
        notes: vec!["测试用报告".into()],
        plans: Vec::new(),
    }
}

#[test]
fn report_lines_are_well_formed() {
    for v in verdicts() {
        let mut r = sample_report(v.clone());
        r.plans = build_plans(&PlanInput {
            writer_addr: Some(0x1400_0500),
            mirrors: 2,
            ..plain_input(&r.verdict)
        });
        let lines = r.lines();
        let text = lines.join("\n");
        assert!(text.contains("【结论】"), "{v:?}");
        assert!(text.contains("【回滚判定】"));
        assert!(text.contains("【页保护】"));
        assert!(text.contains("【应对方案】"));
        assert!(text.contains("【代码写入点】"));
        assert!(text.contains("IsDebuggerPresent"));
        assert!(text.contains("测试用报告"));
        assert!(!lines.iter().any(|l| l.contains("{}")), "不该漏下未替换的占位符");
        // 方案编号从 1 开始且连续
        for (i, p) in r.plans.iter().enumerate() {
            assert!(text.contains(&format!("{}. [{}] {}", i + 1, p.feasibility.label(), p.title)));
        }
    }
}

#[test]
fn headline_covers_every_verdict_and_priority_cases() {
    for v in verdicts() {
        let r = sample_report(v.clone());
        assert!(!r.headline().is_empty(), "{v:?}");
    }
    let mut r = sample_report(Verdict::Stable);
    assert!(r.headline().contains("内存层面没有被还原"));
    r.ac_modules.push(ModuleHit {
        module: "EasyAntiCheat_x64.dll".into(),
        name: "Easy Anti-Cheat (EAC)".into(),
        family: Family::OnlineAntiCheat,
        advice: String::new(),
    });
    assert!(r.headline().contains("在线反作弊"));
    r.ac_modules.clear();
    r.page_blocked = true;
    assert!(r.headline().contains("强制写入"));
}

#[test]
fn readonly_report_says_so() {
    let mut r = sample_report(Verdict::Stable);
    r.read_only = true;
    let text = r.lines().join("\n");
    assert!(text.contains("只读模式"));
}

#[test]
fn verdict_labels_and_reverted_flag_are_consistent() {
    for v in verdicts() {
        assert!(!v.label().is_empty());
        assert_eq!(v.is_reverted(), v != Verdict::Stable, "{v:?}");
    }
    assert!(!Feasibility::High.label().is_empty());
    for f in [Family::OnlineAntiCheat, Family::LocalDrm, Family::Packer] {
        assert!(!f.label().is_empty());
    }
}
