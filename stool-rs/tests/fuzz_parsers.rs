//! 解析器健壮性 / 模糊回归测试。
//!
//! 目标：面对**损坏、截断、伪造**的封包（用户完全可控的输入）时，
//! 所有公开解析入口都必须优雅返回 `Err` / 空结果，**绝不能 panic**。
//!
//! 这里用一个确定性 PRNG（xorshift）生成大量畸形样本，并针对每种格式
//! 构造「正确 magic + 被截断 / 被随机化」的边界样本。任何 panic 都会让
//! 测试失败，从而锁死回归。

use std::collections::BTreeMap;
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;

use stool::formats::{asar, marshal, nscript, pck, pickle, rgss, rpgmmv, xp3};

// ---------- 工具 ----------

/// 捕获并断言 `f` 不 panic（panic 时静默，避免污染测试输出）。
fn no_panic<T>(what: &str, f: impl FnOnce() -> T) {
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let r = panic::catch_unwind(AssertUnwindSafe(f));
    panic::set_hook(prev);
    assert!(r.is_ok(), "✘ 解析入口「{what}」在畸形输入上 panic 了");
}

/// 确定性 xorshift64 PRNG（无需外部依赖，跨平台稳定）。
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next() & 0xFF) as u8).collect()
    }
}

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("stool_fuzz_{}", std::process::id()))
        .join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// 生成一组「刁钻」样本：空、单字节、全 0、全 0xFF、随机、以及针对某 magic
/// 前缀逐长度截断的样本。
fn tricky_samples(prefix: &[u8], rng: &mut Rng) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = vec![
        Vec::new(),
        vec![0],
        vec![0xFF],
        vec![0u8; 16],
        vec![0xFFu8; 64],
    ];
    // 前缀 + 0..prefix.len()+80 长度的随机尾巴（覆盖所有头字段截断点）
    for extra in 0..=(prefix.len() + 80) {
        let mut s = prefix.to_vec();
        s.extend(rng.bytes(extra));
        v.push(s);
    }
    // 纯随机
    for len in [1usize, 3, 8, 11, 12, 16, 20, 24, 32, 40, 64, 128, 255] {
        v.push(rng.bytes(len));
    }
    // 前缀本身（恰好只有 magic）
    if !prefix.is_empty() {
        v.push(prefix.to_vec());
    }
    v
}

// ---------- 各解析器 ----------

#[test]
fn fuzz_pck_parse_bytes() {
    let mut rng = Rng::new(0x1234_5678_9abc_def0);
    for (i, s) in tricky_samples(b"GDPC", &mut rng).into_iter().enumerate() {
        no_panic(&format!("pck::parse_bytes #{i} (len={})", s.len()), || {
            let _ = pck::parse_bytes(&s);
        });
    }
}

#[test]
fn fuzz_xp3_parse_bytes() {
    let mut rng = Rng::new(0xdead_beef_0bad_f00d);
    let mut samples = tricky_samples(xp3::MAGIC, &mut rng);
    // 额外覆盖 flag 字节 0x17（伪装头变体）与 0x80（u64 索引）
    for flag in [0x00u8, 0x17, 0x80, 0xaf] {
        for extra in 0..128usize {
            let mut s = xp3::MAGIC.to_vec();
            s.push(flag);
            s.extend(rng.bytes(extra));
            samples.push(s);
        }
    }
    for (i, s) in samples.into_iter().enumerate() {
        no_panic(&format!("xp3::parse_bytes #{i} (len={})", s.len()), || {
            let _ = xp3::parse_bytes(&s);
        });
    }
}

#[test]
fn fuzz_asar_parse_bytes() {
    let mut rng = Rng::new(0x0f0f_1e1e_2d2d_3c3c);
    // asar magic = u32 LE 4（非 ASCII），另外构造 \x04 + header 结构的样本
    let mut samples = tricky_samples(&4u32.to_le_bytes(), &mut rng);
    for extra in 0..96usize {
        let mut s = 4u32.to_le_bytes().to_vec();
        // header_size(4) + header_string_size(4) + json_size(4) 随机
        s.extend(rng.bytes(extra));
        samples.push(s);
    }
    for (i, s) in samples.into_iter().enumerate() {
        no_panic(&format!("asar::parse_bytes #{i} (len={})", s.len()), || {
            let _ = asar::parse_bytes(&s);
        });
    }
}

#[test]
fn fuzz_marshal_and_pickle() {
    let mut rng = Rng::new(0xcafe_babe_1234_5678);
    for len in [0usize, 1, 2, 4, 8, 16, 32, 64, 128, 512, 4096] {
        let s = rng.bytes(len);
        no_panic(&format!("marshal::load len={len}"), || {
            let _ = marshal::load(&s);
        });
        no_panic(&format!("marshal::extract_scripts len={len}"), || {
            let _ = marshal::extract_scripts(&s);
        });
        no_panic(&format!("marshal::to_json_string len={len}"), || {
            let _ = marshal::to_json_string(&s);
        });
        no_panic(&format!("pickle::loads len={len}"), || {
            let _ = pickle::loads(&s);
        });
    }
    // marshal 常见版本头 4.8 = {0x04, 0x08}，再随机身体
    for extra in 0..64usize {
        let mut s = vec![0x04u8, 0x08];
        s.extend(rng.bytes(extra));
        no_panic(&format!("marshal::load 4.8+rand {extra}"), || {
            let _ = marshal::load(&s);
        });
    }
}

#[test]
fn fuzz_rpgmmv_helpers() {
    let mut rng = Rng::new(0x9999_8888_7777_6666);
    for len in [0usize, 1, 15, 16, 17, 31, 32, 33, 64, 256] {
        let s = rng.bytes(len);
        no_panic(&format!("rpgmmv::decrypt len={len}"), || {
            let _ = rpgmmv::decrypt(&s);
        });
        no_panic(&format!("rpgmmv::encrypt len={len}"), || {
            let _ = rpgmmv::encrypt(&s, len % 2 == 0);
        });
        no_panic(&format!("rpgmmv::mv_save_decode len={len}"), || {
            let _ = rpgmmv::mv_save_decode(&s);
        });
        no_panic(&format!("rpgmmv::mz_save_decode len={len}"), || {
            let _ = rpgmmv::mz_save_decode(&s);
        });
    }
}

#[test]
fn fuzz_nscript_decode() {
    let mut rng = Rng::new(0x4444_5555_6666_7777);
    for len in [0usize, 1, 2, 3, 4, 17, 64, 1024] {
        let s = rng.bytes(len);
        no_panic(&format!("nscript::decode len={len}"), || {
            let _ = nscript::decode(&s);
        });
        no_panic(&format!("nscript::printable_ratio len={len}"), || {
            let _ = nscript::printable_ratio(&s);
        });
    }
}

/// 文件版解析器：写畸形文件再解析。
#[test]
fn fuzz_file_based_parsers() {
    let dir = tmp_dir("files");
    let mut rng = Rng::new(0xabcd_ef01_2345_6789);

    let cases: Vec<(&str, Vec<u8>)> = {
        let mut v: Vec<(&str, Vec<u8>)> = Vec::new();
        // RGSS v1 / v3 magic
        for extra in 0..64usize {
            let mut a = b"RGSSAD\x00".to_vec();
            a.extend(rng.bytes(extra));
            v.push(("rgss_v1", a));
            let mut b = b"RGSSAD\x00\x03".to_vec();
            b.extend(rng.bytes(extra));
            v.push(("rgss_v3", b));
        }
        // PCK / XP3 截断
        for extra in 0..64usize {
            let mut a = b"GDPC".to_vec();
            a.extend(rng.bytes(extra));
            v.push(("pck", a));
            let mut b = xp3::MAGIC.to_vec();
            b.extend(rng.bytes(extra));
            v.push(("xp3", b));
        }
        // 全随机
        for len in [0usize, 8, 24, 64, 200] {
            v.push(("random", rng.bytes(len)));
        }
        v
    };

    for (idx, (kind, bytes)) in cases.into_iter().enumerate() {
        let path = dir.join(format!("{kind}_{idx}.bin"));
        fs::write(&path, &bytes).unwrap();
        let p = path.clone();
        no_panic(&format!("pck::parse #{idx} ({kind})"), || {
            let _ = pck::parse(&p);
        });
        let p = path.clone();
        no_panic(&format!("xp3::parse #{idx} ({kind})"), || {
            let _ = xp3::parse(&p);
        });
        let p = path.clone();
        no_panic(&format!("asar::parse #{idx} ({kind})"), || {
            let _ = asar::parse(&p);
        });
        let p = path.clone();
        no_panic(&format!("rgss::parse_v1 #{idx} ({kind})"), || {
            let _ = rgss::parse_v1(&p);
        });
        no_panic(&format!("rgss::parse_v3 #{idx} ({kind})"), || {
            let _ = rgss::parse_v3(&path);
        });
    }
}

/// 已解析条目的数据读取入口也必须安全（越界 offset/size 不 panic）。
#[test]
fn fuzz_entry_readers() {
    // pck::read_file：构造极端 offset/size
    let data = vec![0xABu8; 64];
    for (off, size) in [(0u64, 0u64), (0, u64::MAX), (u64::MAX, 10), (100, 10), (63, 999)] {
        let e = pck::PckEntry { path: "x".into(), offset: off, size };
        no_panic(&format!("pck::read_file off={off} size={size}"), || {
            let _ = pck::read_file(&data, &e);
        });
    }
    // rgss 数据解密：极端 offset/size
    for (off, size) in [(0u64, 0u32), (0, u32::MAX), (u64::MAX, 10), (10, 999)] {
        no_panic(&format!("rgss::extract_v1_file off={off}"), || {
            let _ = rgss::extract_v1_file(&data, off, size, 0xDEAD_CAFE);
        });
        no_panic(&format!("rgss::extract_v3_file off={off}"), || {
            let _ = rgss::extract_v3_file(&data, off, size as u64, 12345);
        });
    }
    // xp3::read_file：越界 segment
    let mut files = BTreeMap::new();
    files.insert(
        "a".to_string(),
        xp3::Xp3Entry {
            name: "a".into(),
            protected: 0,
            segments: vec![
                xp3::Segment { offset: u64::MAX, orig_size: 10, archive_size: 10 },
                xp3::Segment { offset: 0, orig_size: 5, archive_size: u64::MAX },
                xp3::Segment { offset: 32, orig_size: 0, archive_size: 0 },
            ],
        },
    );
    for (n, e) in &files {
        no_panic(&format!("xp3::read_file {n}"), || {
            let _ = xp3::read_file(&data, e);
        });
    }
}

/// 合法样本的「变异」测试：对真实封包逐字节翻转/截断，确保不 panic。
#[test]
fn fuzz_mutated_valid_packets() {
    let mut rng = Rng::new(0x1357_9bdf_2468_ace0);

    // 用 writer 生成合法 XP3 / PCK / RGSS 样本
    let dir = tmp_dir("mutate");
    let mut kv: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    kv.insert("res://a.txt".to_string(), b"hello world 12345".to_vec());
    kv.insert("res://b.bin".to_string(), (0..200u8).collect());
    let pck_path = dir.join("sample.pck");
    pck::write_v1(&pck_path, &kv).unwrap();
    let xp3_path = dir.join("sample.xp3");
    xp3::write(&xp3_path, &kv).unwrap();
    let rgss1 = dir.join("sample.rgssad");
    rgss::write_v1(&rgss1, &kv).unwrap();
    let rgss3 = dir.join("sample.rgss3a");
    rgss::write_v3(&rgss3, &kv).unwrap();

    let bases: Vec<(&str, Vec<u8>)> = vec![
        ("pck", fs::read(&pck_path).unwrap()),
        ("xp3", fs::read(&xp3_path).unwrap()),
        ("rgss_v1", fs::read(&rgss1).unwrap()),
        ("rgss_v3", fs::read(&rgss3).unwrap()),
    ];

    for (kind, base) in bases {
        // 逐字节翻转（每个位置翻 1 bit）
        for i in 0..base.len().min(600) {
            let mut m = base.clone();
            m[i] ^= 1 << (i % 8);
            dispatch(kind, &m);
        }
        // 随机截断
        for _ in 0..40 {
            let cut = (rng.next() as usize) % (base.len() + 1);
            dispatch(kind, &base[..cut]);
        }
        // 随机替换若干字节
        for _ in 0..40 {
            let mut m = base.clone();
            for _ in 0..8 {
                let pos = (rng.next() as usize) % m.len();
                m[pos] = (rng.next() & 0xFF) as u8;
            }
            dispatch(kind, &m);
        }
    }
}

fn dispatch(kind: &str, bytes: &[u8]) {
    let dir = std::env::temp_dir().join(format!("stool_fuzz_{}", std::process::id()));
    let path = dir.join(format!("mutate_{kind}.bin"));
    fs::write(&path, bytes).unwrap();
    match kind {
        "pck" => {
            no_panic("mutate pck", || {
                let _ = pck::parse_bytes(bytes);
            });
            let p = path.clone();
            no_panic("mutate pck file", || {
                let _ = pck::parse(&p);
            });
        }
        "xp3" => {
            no_panic("mutate xp3", || {
                let _ = xp3::parse_bytes(bytes);
            });
            let p = path.clone();
            no_panic("mutate xp3 file", || {
                let _ = xp3::parse(&p);
            });
        }
        "rgss_v1" => {
            let p = path.clone();
            no_panic("mutate rgss v1", || {
                let _ = rgss::parse_v1(&p);
            });
        }
        "rgss_v3" => {
            let p = path.clone();
            no_panic("mutate rgss v3", || {
                let _ = rgss::parse_v3(&p);
            });
        }
        _ => {}
    }
}
