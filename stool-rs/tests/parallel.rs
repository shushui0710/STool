//! P2-2 解包并行化端到端测试。
//!
//! 对合成封包分别用「单线程（jobs=1）」与「多线程（jobs=8）」真实跑一遍
//! `Engine::extract`，要求落盘结果**逐文件、逐字节一致**。
//! 这验证的是完整接线（并发写盘 + 目录缓存 + 断点台账互不干扰），
//! 而不只是 `parallel_extract` 这个纯函数。

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use stool::engines::{Ctx, Engine, GodotPlugin, KirikiriPlugin, RpgMakerRgssPlugin};
use stool::formats::{pck, rgss, xp3};

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("stool_parallel_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// 以指定并行度跑一次解包，返回 (成功, 结果消息)。
fn run_extract(e: &dyn Engine, root: &Path, out: &Path, jobs: usize) -> (bool, String) {
    let opts: HashMap<String, String> = [("jobs".to_string(), jobs.to_string())].into_iter().collect();
    let cancel = AtomicBool::new(false);
    let prog = |_: f32, _: &str| {};
    let ctx = Ctx { root, out_dir: out, options: &opts, progress: &prog, cancel: &cancel };
    let r = e.extract(&ctx);
    (r.success, r.message)
}

/// 收集目录树：相对路径（`/` 归一）→ 文件内容。
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut m = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        // 断点台账不是产物，忽略
        if rel.starts_with(".stool_resume") {
            continue;
        }
        m.insert(rel, fs::read(entry.path()).unwrap());
    }
    m
}

/// 生成一批体量各异、含空文件的条目。
fn sample(n: usize) -> BTreeMap<String, Vec<u8>> {
    let mut m = BTreeMap::new();
    for i in 0..n {
        let dir = format!("data{}", i % 7);
        let name = format!("{dir}/f{i}.dat");
        let len = (i % 97) + 1;
        let byte = (i % 251) as u8;
        let body = if i % 23 == 0 { Vec::new() } else { vec![byte; len] };
        m.insert(name, body);
    }
    m
}

fn assert_parallel_equals_serial(e: &dyn Engine, d: &Path, tag: &str, expect_files: usize) {
    let game = d.join(format!("game_{tag}"));
    fs::create_dir_all(&game).unwrap();
    let out1 = d.join(format!("out1_{tag}"));
    let out8 = d.join(format!("out8_{tag}"));

    let (ok1, msg1) = run_extract(e, &game, &out1, 1);
    let (ok8, msg8) = run_extract(e, &game, &out8, 8);
    assert!(ok1, "单线程解包应成功：{msg1}");
    assert!(ok8, "多线程解包应成功：{msg8}");

    let t1 = tree(&out1);
    let t8 = tree(&out8);
    assert_eq!(t1.len(), expect_files, "{tag}: 单线程产物条目数不符");
    assert_eq!(t8.len(), expect_files, "{tag}: 多线程产物条目数不符");
    assert_eq!(t1, t8, "{tag}: 单线程与多线程产物必须完全一致");
    // 台账在成功后应被清掉（无残留）
    for out in [&out1, &out8] {
        assert!(
            !out.join(".stool_resume_extract.json").exists(),
            "{tag}: 成功后台账应删除"
        );
    }
}

#[test]
fn kirikiri_extract_parallel_matches_serial() {
    let d = tmpdir("kk");
    let files = sample(300);
    // .xp3 真实名字用 `/` 分隔
    let game = d.join("game_kk");
    fs::create_dir_all(&game).unwrap();
    xp3::write(&game.join("data.xp3"), &files).unwrap();
    assert_parallel_equals_serial(&KirikiriPlugin, &d, "kk", files.len());
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn godot_pck_extract_parallel_matches_serial() {
    let d = tmpdir("pck");
    let files = sample(250);
    let game = d.join("game_pck");
    fs::create_dir_all(&game).unwrap();
    pck::write_v1(&game.join("game.pck"), &files).unwrap();
    assert_parallel_equals_serial(&GodotPlugin, &d, "pck", files.len());
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn rgss3a_extract_parallel_matches_serial() {
    let d = tmpdir("rgss");
    // RGSSAD 的名字用反斜杠
    let mut files = BTreeMap::new();
    for i in 0..400 {
        let name = format!("Data\\file{i}.rvdata");
        let body = if i % 31 == 0 { Vec::new() } else { vec![(i % 241) as u8; (i % 53) + 1] };
        files.insert(name, body);
    }
    let game = d.join("game_rgss");
    fs::create_dir_all(&game).unwrap();
    rgss::write_v3(&game.join("Game.rgss3a"), &files).unwrap();
    assert_parallel_equals_serial(&RpgMakerRgssPlugin, &d, "rgss", files.len());
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn rgssad_v1_extract_parallel_matches_serial() {
    let d = tmpdir("rgssv1");
    let mut files = BTreeMap::new();
    for i in 0..120 {
        files.insert(format!("Graphics\\g{i}.png"), vec![(i % 200) as u8; (i % 41) + 1]);
    }
    let game = d.join("game_rgssv1");
    fs::create_dir_all(&game).unwrap();
    rgss::write_v1(&game.join("Game.rgssad"), &files).unwrap();
    assert_parallel_equals_serial(&RpgMakerRgssPlugin, &d, "rgssv1", files.len());
    let _ = fs::remove_dir_all(&d);
}
