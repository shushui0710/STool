//! 合成样本回环测试：各格式 writer → parser → 逐字节比对。
//! 与 Python 原型 tests/test_core.py 的样本生成逻辑一致，可交叉验证。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use stool::formats::source::{Source, MAX_ARCHIVE};
use stool::formats::{asar, nscript, pck, pfs, pickle, rpa, rgss, rpgmmv, xp3};

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("stool_test_{}", std::process::id())).join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

// ---------- XP3 writer（仅测试用） ----------

fn chunk(tag: &[u8], body: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

fn make_xp3(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut toc_entries: Vec<u8> = Vec::new();
    let mut data_blob: Vec<u8> = Vec::new();
    for (name, content) in files {
        let comp = {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(content).unwrap();
            z.finish().unwrap()
        };
        // 数据区紧随 20 字节头，偏移为文件内绝对偏移
        let aoff = (20 + data_blob.len()) as u64;
        data_blob.extend_from_slice(&comp);
        let nb: Vec<u8> = name.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        let mut info = Vec::new();
        info.extend_from_slice(&0u32.to_le_bytes());
        info.extend_from_slice(&(content.len() as u64).to_le_bytes());
        info.extend_from_slice(&(comp.len() as u64).to_le_bytes());
        info.extend_from_slice(&(name.chars().count() as u16).to_le_bytes());
        info.extend_from_slice(&nb);
        let mut segm = (1u32).to_le_bytes().to_vec();
        segm.extend_from_slice(&aoff.to_le_bytes());
        segm.extend_from_slice(&(content.len() as u64).to_le_bytes());
        segm.extend_from_slice(&(comp.len() as u64).to_le_bytes());
        let mut adlr = Vec::new();
        adlr.extend_from_slice(&adler32(content).to_le_bytes());
        let entry_body = chunk(b"info", &info);
        let entry_body = [entry_body, chunk(b"segm", &segm), chunk(b"adlr", &adlr)].concat();
        toc_entries.extend_from_slice(&(entry_body.len() as u32).to_le_bytes());
        toc_entries.extend_from_slice(&entry_body);
    }
    let mut toc = (files.len() as u32).to_le_bytes().to_vec();
    toc.extend_from_slice(&toc_entries);
    let comp_toc = {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&toc).unwrap();
        z.finish().unwrap()
    };
    // 布局：20 字节头 + 数据区 + [0x01 + zlib TOC]，索引偏移指向 0x01 标志字节
    let index_pos = (20 + data_blob.len()) as u64;
    let mut out = stool::formats::xp3::MAGIC.to_vec();
    out.push(0x80);
    out.extend_from_slice(&index_pos.to_le_bytes());
    out.extend_from_slice(&data_blob);
    out.push(0x01);
    out.extend_from_slice(&comp_toc);
    out
}

fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

// ---------- NScripter 加密（仅测试用） ----------

fn make_nscript(text: &str) -> Vec<u8> {
    let data = text.as_bytes(); // ASCII 测试即可
    let key: u32 = 0x1234;
    let mut enc = vec![(key >> 8) as u8, (key & 0xFF) as u8];
    for (i, b) in data.iter().enumerate() {
        enc.push(b ^ ((key + i as u32) & 0xFF) as u8);
    }
    enc
}

// ---------- 测试 ----------

#[test]
fn rpa_roundtrip() {
    let dir = tmp_dir("rpa");
    let arc = dir.join("archive.rpa");
    let mut files = BTreeMap::new();
    files.insert("images/bg.png".to_string(), vec![0x89, b'P', b'N', b'G', 0, 0, 0, 0]);
    files.insert("scripts/story.rpyc".to_string(), b"FAKE_RPYC_DATA".repeat(10));
    rpa::write_archive(&arc, &files, 0xDEADBEEF).unwrap();
    let index = rpa::read_index(&arc).unwrap();
    assert_eq!(index.entries.len(), 2);
    for (name, data) in &files {
        let got = rpa::read_file(&arc, &index.entries[name]).unwrap();
        assert_eq!(&got, data, "RPA 内容不一致: {name}");
    }
}

#[test]
fn rgss_v1_roundtrip() {
    let dir = tmp_dir("rgss1");
    let arc = dir.join("Game.rgssad");
    let mut files = BTreeMap::new();
    files.insert("Graphics/Battlesets/hero.png".to_string(), b"\x89PNGDATA\x00\x01".repeat(3));
    files.insert("Data/Actors.rxdata".to_string(), b"marshal\x00\x01\x02".to_vec());
    rgss::write_v1(&arc, &files).unwrap();
    let index = rgss::parse_v1(&arc).unwrap();
    let data = fs::read(&arc).unwrap();
    for (name, (off, size, key)) in &index {
        let got = rgss::extract_v1_file(&data, *off, *size, *key);
        assert_eq!(&got, files.get(name).unwrap(), "RGSS1 内容不一致: {name}");
    }
}

#[test]
fn rgss_v3_roundtrip() {
    let dir = tmp_dir("rgss3");
    let arc = dir.join("Game.rgss3a");
    let mut files = BTreeMap::new();
    files.insert("Graphics/Animetion/foo.png".to_string(), b"PNGDATA99".repeat(7));
    files.insert("Data/Items.rxdata".to_string(), b"ace-data\x01\x02".to_vec());
    rgss::write_v3(&arc, &files).unwrap();
    let entries = rgss::parse_v3(&arc).unwrap();
    let data = fs::read(&arc).unwrap();
    assert_eq!(entries.len(), files.len());
    for e in &entries {
        let got = rgss::extract_v3_file(&data, e.offset, e.size, e.filekey);
        assert_eq!(&got, files.get(&e.name).unwrap(), "RGSS3 内容不一致: {}", e.name);
    }
}

#[test]
fn xp3_roundtrip() {
    let dir = tmp_dir("xp3");
    let arc = dir.join("data.xp3");
    let mut files = BTreeMap::new();
    files.insert("scene1.ks".to_string(), b"*start\nhello world\r\n".to_vec());
    files.insert("bg001.jpg".to_string(), b"\xff\xd8\xff\xe0JFIF".repeat(5));
    fs::write(&arc, make_xp3(&files)).unwrap();
    let data = fs::read(&arc).unwrap();
    let parsed = xp3::parse_bytes(&data).unwrap();
    assert_eq!(parsed.len(), 2);
    for (name, content) in &files {
        let entry = &parsed[name];
        let got = xp3::read_file(&data, entry).unwrap();
        assert_eq!(&got, content, "XP3 内容不一致: {name}");
    }
}

#[test]
fn pck_roundtrip() {
    let dir = tmp_dir("pck");
    let arc = dir.join("data.pck");
    let mut files = BTreeMap::new();
    files.insert("res://scripts/main.gd".to_string(), b"extends Node\n".to_vec());
    files.insert("res://assets/logo.png".to_string(), b"\x89PNGlogo".repeat(3));
    pck::write_v1(&arc, &files).unwrap();
    let data = fs::read(&arc).unwrap();
    let entries = pck::parse_bytes(&data).unwrap();
    assert_eq!(entries.len(), 2);
    for e in &entries {
        let got = pck::read_file(&data, e);
        assert_eq!(&got, files.get(&e.path).unwrap(), "PCK 内容不一致: {}", e.path);
    }
}

#[test]
fn asar_roundtrip() {
    let dir = tmp_dir("asar");
    let src = dir.join("app");
    fs::create_dir_all(src.join("js")).unwrap();
    fs::write(src.join("index.html"), b"<html><body>hi</body></html>").unwrap();
    fs::write(src.join("js").join("main.js"), b"console.log('hello')").unwrap();
    let arc = dir.join("app.asar");
    asar::pack(&src, &arc).unwrap();
    let data = fs::read(&arc).unwrap();
    let (files, data_start) = asar::parse_bytes(&data).unwrap();
    assert_eq!(files.len(), 2);
    let html = asar::read_file(&data, data_start, &files["index.html"]).unwrap();
    assert_eq!(html, b"<html><body>hi</body></html>");
    let js = asar::read_file(&data, data_start, &files["js/main.js"]).unwrap();
    assert_eq!(js, b"console.log('hello')");
}

#[test]
fn pfs_roundtrip() {
    let dir = tmp_dir("pfs");
    let files: Vec<(String, Vec<u8>)> = vec![
        ("system.ini".to_string(), b"; Artemis config\n".to_vec()),
        ("font\\SourceHanSerif-Bold.otf".to_string(), b"OTTO\x00\x10\x01\x00 fake font".to_vec()),
        ("pc\\bg_cn.png".to_string(), b"\x89PNG\r\n\x1a\n fake png".to_vec()),
        ("script\\deep\\nested\\main.ast".to_string(), b"ast={{}}\n".to_vec()),
        ("empty.bin".to_string(), Vec::new()),
    ];
    for ver in *b"86" {
        let arc = dir.join(format!("root_pf{}.pfs", ver as char));
        pfs::write_archive(&arc, &files, ver).unwrap();
        let mut src = Source::open(&arc, MAX_ARCHIVE).unwrap();
        let ix = pfs::parse_index(&mut src).unwrap();
        assert_eq!(ix.entries.len(), files.len(), "pf{}", ver as char);
        let by_name: BTreeMap<&str, &Vec<u8>> =
            files.iter().map(|(n, b)| (n.as_str(), b)).collect();
        for e in &ix.entries {
            assert_eq!(e.size, by_name[e.name.as_str()].len() as u64, "{} 大小不符", e.name);
            let got = pfs::read_entry(&mut src, &ix, e).unwrap();
            assert_eq!(&got, by_name[e.name.as_str()], "条目 {} 内容不一致", e.name);
        }
        // pf8 的数据区必须是密文（XOR 生效），读出来才是明文
        if ver == b'8' {
            let raw = {
                let mut s2 = Source::open(&arc, MAX_ARCHIVE).unwrap();
                s2.read_at(ix.entries[1].offset, 4).unwrap()
            };
            assert_ne!(raw, b"OTTO".to_vec(), "pf8 落盘的数据区应是密文");
            let plain = pfs::read_entry(&mut src, &ix, &ix.entries[1]).unwrap();
            assert_eq!(&plain[..4], b"OTTO", "解出的内容应是明文");
        }
    }
}

#[test]
fn rpgmmv_decrypt_roundtrip() {
    let png = b"\x89PNG\r\n\x1a\nREALPNGDATA-0123456789".to_vec();
    let enc = rpgmmv::encrypt(&png, false);
    assert_eq!(rpgmmv::decrypt(&enc), png);
    let enc_mz = rpgmmv::encrypt(&png, true);
    assert_eq!(rpgmmv::decrypt(&enc_mz), png);
    assert!(!rpgmmv::is_encrypted(&png));
    assert!(rpgmmv::is_encrypted(&enc));
}

#[test]
fn nscript_decode() {
    let text = "*define\r\ncaption \"Test\"\r\n*start\r\nHello World\r\n";
    let enc = make_nscript(text);
    let dec = nscript::decode(&enc);
    assert_eq!(String::from_utf8_lossy(&dec), text, "nscript 解密应还原明文");
}

#[test]
fn pickle_index_like() {
    // 模拟 Ren'Py 索引 pickle：{b"img/a.png": [(10,20)]}
    let mut data = vec![0x80u8, 2, b'}'];
    let name = b"img/a.png";
    data.push(b'X');
    data.extend_from_slice(&(name.len() as u32).to_le_bytes());
    data.extend_from_slice(name);
    data.push(b']');
    for v in [10u32, 20] {
        data.push(b'J');
        data.extend_from_slice(&v.to_le_bytes());
    }
    data.push(0x86); // TUPLE2
    data.push(b'a'); // APPEND
    data.push(b's'); // SETITEM
    data.push(b'.');
    let v = pickle::loads(&data).unwrap();
    let d = v.as_dict().unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].0.as_bytes().unwrap(), b"img/a.png");
    let chunks = d[0].1.as_list().unwrap();
    let t = chunks[0].as_list().unwrap();
    assert_eq!((t[0].as_int(), t[1].as_int()), (Some(10), Some(20)));
}

#[test]
fn pickle_persistent_like() {
    // {'seen': True, 'count': 3, 'names': ['a','b']}
    let mut data = vec![0x80u8, 2, b'}'];
    let put = |data: &mut Vec<u8>, k: &str| {
        data.push(b'X');
        data.extend_from_slice(&(k.len() as u32).to_le_bytes());
        data.extend_from_slice(k.as_bytes());
    };
    put(&mut data, "seen");
    data.push(0x88); // NEWTRUE
    data.push(b's'); // SETITEM
    put(&mut data, "count");
    data.push(b'K');
    data.push(3);
    data.push(b's');
    put(&mut data, "names");
    data.push(b']');
    for n in ["a", "b"] {
        data.push(b'X');
        data.extend_from_slice(&1u32.to_le_bytes());
        data.push(n.as_bytes()[0]);
        data.push(b'a');
    }
    data.push(b's');
    data.push(b'.');
    let v = pickle::loads(&data).unwrap();
    let d = v.as_dict().unwrap();
    assert_eq!(d[0].1, pickle::Value::Bool(true));
    assert_eq!(d[1].1, pickle::Value::Int(3));
    assert_eq!(d[2].1.as_list().unwrap().len(), 2);
}

#[test]
fn lzstring_roundtrip() {
    // 样本由 Python lzstring 包 compressToBase64 生成（交叉验证）
    assert_eq!(
        stool::formats::lzstring::decompress_from_base64("BYUwNmD2AEDukCcwBMg=").as_deref(),
        Some("hello world")
    );
    assert_eq!(
        stool::formats::lzstring::decompress_from_base64("BYUwNmD2AEhocoMq5A==").as_deref(),
        Some("hello \u{4e16}\u{754c}")
    );
    assert_eq!(
        stool::formats::lzstring::decompress_from_base64("lNykpNGFBtFg5RMVMIAZQjeYpyA=").as_deref(),
        Some("\u{4eca}\u{5929}\u{5929}\u{6c14}\u{4e0d}\u{9519}\u{3002}".repeat(3).as_str())
    );
}

// ---------- B0/B1 新功能回环测试 ----------

use std::sync::atomic::AtomicBool;

#[test]
fn xp3_writer_roundtrip() {
    let dir = tmp_dir("xp3w");
    let arc = dir.join("data.xp3");
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    files.insert("scene1.ks".into(), "*start\nようこそ、世界へ。\r\n".as_bytes().to_vec());
    files.insert("bg001.jpg".into(), b"\xff\xd8\xff\xe0JFIF-IMG".repeat(50));
    files.insert("tiny.bin".into(), b"a".to_vec()); // 压缩收益为负 → 明文存储
    // 内存版 writer
    let arc2 = dir.join("data2.xp3");
    xp3::write(&arc2, &files).unwrap();
    let data = fs::read(&arc2).unwrap();
    let parsed = xp3::parse_bytes(&data).unwrap();
    assert_eq!(parsed.len(), 3);
    for (name, content) in &files {
        let got = xp3::read_file(&data, &parsed[name]).unwrap();
        assert_eq!(&got, content, "XP3 writer 内容不一致: {name}");
    }
    // 路径版 writer
    let mut paths: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (name, content) in &files {
        let p = dir.join(format!("f_{}", xp3_hash(name)));
        fs::write(&p, content).unwrap();
        paths.insert(name.clone(), p);
    }
    xp3::write_paths(&arc, &paths).unwrap();
    let data = fs::read(&arc).unwrap();
    let parsed = xp3::parse_bytes(&data).unwrap();
    for (name, content) in &files {
        assert_eq!(&xp3::read_file(&data, &parsed[name]).unwrap(), content, "XP3 paths 内容不一致: {name}");
    }
}

fn xp3_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[test]
fn rgss_paths_roundtrip() {
    let dir = tmp_dir("rgssp");
    let mut paths: BTreeMap<String, PathBuf> = BTreeMap::new();
    let payloads: Vec<(&str, Vec<u8>)> = vec![
        ("Graphics/Battlesets/hero.png", b"\x89PNG-HERO".repeat(40)),
        ("Data/Actors.rxdata", b"marshal-data\x00\x01".repeat(9)),
    ];
    for (i, (name, content)) in payloads.iter().enumerate() {
        let p = dir.join(format!("src_{i}"));
        fs::write(&p, content).unwrap();
        paths.insert(name.to_string(), p);
    }
    // v1 流式
    let arc1 = dir.join("Game.rgssad");
    rgss::write_v1_paths(&arc1, &paths).unwrap();
    let data = fs::read(&arc1).unwrap();
    let index = rgss::parse_v1(&arc1).unwrap();
    for (name, (off, size, key)) in &index {
        let got = rgss::extract_v1_file(&data, *off, *size, *key);
        assert_eq!(&got, payloads.iter().find(|(n, _)| rgss_name_eq(n, name)).unwrap().1.as_slice(), "v1 不一致: {name}");
    }
    // v3 流式
    let arc3 = dir.join("Game.rgss3a");
    rgss::write_v3_paths(&arc3, &paths).unwrap();
    let data = fs::read(&arc3).unwrap();
    let entries = rgss::parse_v3(&arc3).unwrap();
    assert_eq!(entries.len(), paths.len());
    for e in &entries {
        let got = rgss::extract_v3_file(&data, e.offset, e.size, e.filekey);
        assert_eq!(&got, payloads.iter().find(|(n, _)| rgss_name_eq(n, &e.name)).unwrap().1.as_slice(), "v3 不一致: {}", e.name);
    }
}

fn rgss_name_eq(a: &str, b: &str) -> bool {
    a.replace('/', "\\") == b.replace('/', "\\")
}

#[test]
fn pck_paths_roundtrip() {
    let dir = tmp_dir("pckp");
    let mut paths: BTreeMap<String, PathBuf> = BTreeMap::new();
    let payloads: Vec<(&str, Vec<u8>)> = vec![
        ("res://scripts/main.gd", b"extends Node\nfunc _ready(): pass\n".to_vec()),
        ("res://assets/logo.png", b"\x89PNG-logo".repeat(30)),
    ];
    for (i, (name, content)) in payloads.iter().enumerate() {
        let p = dir.join(format!("src_{i}"));
        fs::write(&p, content).unwrap();
        paths.insert(name.to_string(), p);
    }
    let arc = dir.join("game.pck");
    pck::write_v1_paths(&arc, &paths).unwrap();
    let data = fs::read(&arc).unwrap();
    let entries = pck::parse_bytes(&data).unwrap();
    assert_eq!(entries.len(), 2);
    for e in &entries {
        assert_eq!(&pck::read_file(&data, e), payloads.iter().find(|(n, _)| n == &e.path).unwrap().1.as_slice(), "PCK 不一致: {}", e.path);
    }
}

#[test]
fn nscript_text_extract_import() {
    use stool::engines::{Engine, NscripterPlugin, Op};
    let dir = tmp_dir("nsc");
    // 模拟 Shift-JIS 脚本：命令行 + 对白行
    let script = "*define\r\ngamekey 4654\r\n*start\r\nクリックで進みます\r\n「こんにちは、世界」\r\nclick\r\n二行目のテキストです\\\r\nend\r\n";
    let (enc, _, had_err) = encoding_rs::SHIFT_JIS.encode(script);
    assert!(!had_err);
    let key: u32 = 0x1234;
    let mut dat = vec![(key >> 8) as u8, (key & 0xFF) as u8];
    for (i, b) in enc.iter().enumerate() {
        dat.push(b ^ ((key + i as u32) & 0xFF) as u8);
    }
    fs::write(dir.join("nscript.dat"), &dat).unwrap();

    let plugin = NscripterPlugin;
    assert!(plugin.capabilities().contains(&Op::TextExtract));
    let cancel = AtomicBool::new(false);
    let out_dir = dir.join("out");
    fs::create_dir_all(&out_dir).unwrap();
    let opts = std::collections::HashMap::new();
    let ctx = stool::engines::Ctx {
        root: &dir,
        out_dir: &out_dir,
        options: &opts,
        progress: &|_, _| {},
        cancel: &cancel,
    };

    // 提取
    let csv = dir.join("text.csv");
    let r = plugin.text_extract(&ctx, &csv);
    assert!(r.success, "text_extract 失败: {}", r.message);
    let rows = stool::features::text::read_csv(&csv).unwrap();
    assert!(rows.len() >= 3, "应至少提取 3 行对白，实际 {}", rows.len());
    assert!(rows.iter().any(|r| r[3].contains("こんにちは、世界")));

    // 回填：改第一行对白（译文必须全部为 Shift-JIS 可编码字符）
    let mut modified = rows.clone();
    for row in modified.iter_mut() {
        if row[3].contains("こんにちは、世界") {
            row[4] = "好美的世界".into(); // 好/美/世/界 均在 JIS X 0208 内
        }
    }
    let csv2 = dir.join("text2.csv");
    let rows4: Vec<[String; 4]> = modified
        .iter()
        .map(|r| [r[0].clone(), r[2].clone(), r[3].clone(), r[4].clone()])
        .collect();
    stool::features::text::write_csv(&csv2, &rows4).unwrap();
    let r = plugin.text_import(&ctx, &csv2);
    assert!(r.success, "text_import 失败: {}", r.message);

    // 验证：重新解密应包含译文，且加密结构仍有效（前缀密钥可解）
    let dat2 = fs::read(dir.join("nscript.dat")).unwrap();
    let dec = nscript::decode(&dat2);
    let (text, _, _) = encoding_rs::SHIFT_JIS.decode(&dec);
    assert!(text.contains("好美的世界"), "回填后应包含译文");
    assert!(text.contains("*define"), "脚本结构应保留");
    // 备份存在
    assert!(dir.join("nscript.dat.stool.bak").exists());
}
