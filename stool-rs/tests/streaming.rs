//! P2-1 流式解析等价性测试。
//!
//! 每个格式都写一份合成封包，然后**分别**用「内存模式」与「强制文件模式」的
//! `Source` 走一遍 索引解析 + 条目读取，要求与原始内容逐字节一致。
//! 这样流式路径（seek + 按需读）与旧的整包读入路径就有了等价的回归保障。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use stool::formats::source::{Source, MAX_ARCHIVE};
use stool::formats::{asar, pck, rgss, xp3};

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("stool_streaming_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// 内存模式与文件模式都跑一遍闭包。
fn both_modes<T>(path: &Path, f: impl Fn(&mut Source) -> T) {
    let mut mem = Source::open_with(path, MAX_ARCHIVE, u64::MAX).expect("内存模式打开");
    assert!(mem.mem().is_some(), "应走内存模式");
    let _ = f(&mut mem);
    let mut file = Source::open_with(path, MAX_ARCHIVE, 0).expect("文件模式打开");
    assert!(file.mem().is_none(), "应走文件模式");
    let _ = f(&mut file);
}

fn sample() -> BTreeMap<String, Vec<u8>> {
    let mut m = BTreeMap::new();
    m.insert("readme.txt".to_string(), b"hello stool".to_vec());
    m.insert("Data/big.bin".to_string(), vec![0xA5u8; 8192]);
    m.insert("Data/nested/deep/x.dat".to_string(), (0..=255u8).collect());
    m.insert("空文件.bin".to_string(), Vec::new());
    m
}

#[test]
fn xp3_stream_equals_in_memory() {
    let d = tmpdir("xp3");
    let arc = d.join("data.xp3");
    let files = sample();
    xp3::write(&arc, &files).unwrap();

    both_modes(&arc, |src| {
        let idx = xp3::parse_index(src).expect("XP3 索引");
        assert_eq!(idx.len(), files.len());
        for (name, entry) in &idx {
            let got = xp3::read_entry(src, entry).expect("读条目");
            assert_eq!(&got, files.get(name).unwrap(), "条目 {name} 内容不一致");
        }
    });
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn pck_stream_equals_in_memory() {
    let d = tmpdir("pck");
    let arc = d.join("game.pck");
    let mut files = BTreeMap::new();
    files.insert("res://icon.png".to_string(), vec![1u8, 2, 3, 4]);
    files.insert("res://scripts/main.gd".to_string(), b"extends Node\n".to_vec());
    files.insert("res://a/b/c/deep.bin".to_string(), vec![9u8; 3000]);
    pck::write_v1(&arc, &files).unwrap();

    both_modes(&arc, |src| {
        let idx = pck::parse_index(src).expect("PCK 索引");
        assert_eq!(idx.len(), files.len());
        for e in &idx {
            let got = pck::read_entry(src, e).expect("读条目");
            assert_eq!(&got, files.get(&e.path).unwrap(), "条目 {} 内容不一致", e.path);
        }
    });
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn rgss_v1_and_v3_stream_equal_in_memory() {
    let d = tmpdir("rgss");
    // 真实 RGSSAD 的名字用反斜杠
    let mut files = BTreeMap::new();
    files.insert("Data\\System.rvdata".to_string(), b"sys".to_vec());
    files.insert("Graphics\\Title.png".to_string(), vec![0x5Au8; 5000]);
    files.insert("Data\\Map001.rvdata".to_string(), (0..1000u32).flat_map(|v| v.to_le_bytes()).collect());

    let v1 = d.join("Game.rgssad");
    rgss::write_v1(&v1, &files).unwrap();
    both_modes(&v1, |src| {
        let idx = rgss::parse_index_v1(src).expect("v1 索引");
        assert_eq!(idx.len(), files.len());
        for (name, (off, size, key)) in &idx {
            let got = rgss::read_entry_v1(src, *off, *size, *key).expect("读 v1 条目");
            assert_eq!(&got, files.get(name).unwrap(), "v1 条目 {name} 不一致");
        }
    });

    let v3 = d.join("Game.rgss3a");
    rgss::write_v3(&v3, &files).unwrap();
    both_modes(&v3, |src| {
        let idx = rgss::parse_index_v3(src).expect("v3 索引");
        assert_eq!(idx.len(), files.len());
        for e in &idx {
            let got = rgss::read_entry_v3(src, e.offset, e.size, e.filekey).expect("读 v3 条目");
            assert_eq!(&got, files.get(&e.name).unwrap(), "v3 条目 {} 不一致", e.name);
        }
    });
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn asar_stream_equals_in_memory() {
    let d = tmpdir("asar");
    let src_dir = d.join("app");
    fs::create_dir_all(src_dir.join("dist/assets")).unwrap();
    fs::write(src_dir.join("package.json"), b"{\"name\":\"x\"}").unwrap();
    fs::write(src_dir.join("dist/index.html"), b"<html></html>").unwrap();
    fs::write(src_dir.join("dist/assets/app.js"), vec![7u8; 4096]).unwrap();
    let arc = d.join("app.asar");
    asar::pack(&src_dir, &arc).unwrap();

    let expect: BTreeMap<String, Vec<u8>> = [
        ("package.json".to_string(), b"{\"name\":\"x\"}".to_vec()),
        ("dist/index.html".to_string(), b"<html></html>".to_vec()),
        ("dist/assets/app.js".to_string(), vec![7u8; 4096]),
    ]
    .into_iter()
    .collect();

    both_modes(&arc, |src| {
        let (files, data_start) = asar::parse_index(src).expect("asar 索引");
        assert_eq!(files.len(), expect.len());
        for (rel, node) in &files {
            let got = asar::read_entry(src, data_start, node).expect("读条目");
            assert_eq!(&got, expect.get(rel).unwrap(), "条目 {rel} 不一致");
        }
    });
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn open_rejects_oversized() {
    let d = tmpdir("cap");
    let p = d.join("x.bin");
    fs::write(&p, vec![0u8; 128]).unwrap();
    assert!(Source::open(&p, 16).is_err());
    let _ = fs::remove_dir_all(&d);
}
