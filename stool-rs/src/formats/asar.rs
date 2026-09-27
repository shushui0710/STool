//! Electron asar 归档（解析/解包/打包）。

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct AsarNode {
    #[serde(default)]
    pub files: BTreeMap<String, AsarNode>,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub offset: String,
    #[serde(default)]
    pub unpacked: bool,
}

pub fn parse(path: &Path) -> Result<(BTreeMap<String, AsarNode>, u64), String> {
    let data = fs::read(path).map_err(|e| e.to_string())?;
    parse_bytes(&data)
}

pub fn parse_bytes(data: &[u8]) -> Result<(BTreeMap<String, AsarNode>, u64), String> {
    if data.len() < 16 || u32::from_le_bytes(data[0..4].try_into().unwrap()) != 4 {
        return Err("不是 asar 文件".into());
    }
    // Chromium pickle 布局：
    //   [u32=4][u32=headerBuf总长][u32=headerBuf总长][u32=JSON长度][JSON][补齐]
    let header_total = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let json_size = u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize;
    if 8 + header_total > data.len() || 16 + json_size > data.len() {
        return Err("asar 头部越界".into());
    }
    let header: AsarNode = serde_json::from_slice(&data[16..16 + json_size]).map_err(|e| e.to_string())?;
    let data_start = (8 + header_total) as u64;
    let mut files = BTreeMap::new();
    walk(&header, "", &mut files);
    Ok((files, data_start))
}

fn walk(node: &AsarNode, prefix: &str, out: &mut BTreeMap<String, AsarNode>) {
    for (name, child) in &node.files {
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if !child.files.is_empty() {
            walk(child, &rel, out);
        } else if !child.offset.is_empty() {
            out.insert(rel, child.clone());
        }
    }
}

// ---------- 流式 API（P2-1）：只读索引 + 按需读条目 ----------

/// 只读索引（不整包读入）：头部 16 字节 + JSON 头，数据区不动。
pub fn parse_index(src: &mut super::source::Source) -> Result<(BTreeMap<String, AsarNode>, u64), String> {
    let len = src.len();
    if len < 16 {
        return Err("不是 asar 文件".into());
    }
    let head = src.read_exact_at(0, 16)?;
    if u32::from_le_bytes(head[0..4].try_into().unwrap()) != 4 {
        return Err("不是 asar 文件".into());
    }
    // [u32=4][u32=headerBuf总长][u32=headerBuf总长][u32=JSON长度][JSON][补齐]
    let header_total = u32::from_le_bytes(head[4..8].try_into().unwrap()) as u64;
    let json_size = u32::from_le_bytes(head[12..16].try_into().unwrap()) as u64;
    if 8 + header_total > len || 16 + json_size > len {
        return Err("asar 头部越界".into());
    }
    let json = src.read_exact_at(16, json_size as usize)?;
    let header: AsarNode = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
    let data_start = 8 + header_total;
    let mut files = BTreeMap::new();
    walk(&header, "", &mut files);
    Ok((files, data_start))
}

/// 按需读取一个条目。
pub fn read_entry(src: &mut super::source::Source, data_start: u64, entry: &AsarNode) -> Result<Vec<u8>, String> {
    if entry.unpacked {
        return Err("该文件存储于 app.asar.unpacked 目录".into());
    }
    let off = data_start
        .checked_add(entry.offset.parse::<u64>().map_err(|e| e.to_string())?)
        .ok_or("asar 数据偏移溢出")?;
    src.read_at(off, entry.size as usize)
}

pub fn read_file(data: &[u8], data_start: u64, entry: &AsarNode) -> Result<Vec<u8>, String> {
    if entry.unpacked {
        return Err("该文件存储于 app.asar.unpacked 目录".into());
    }
    let off = data_start
        .checked_add(entry.offset.parse::<u64>().map_err(|e| e.to_string())?)
        .ok_or("asar 数据偏移溢出")?;
    // 饱和夹取，避免伪造 size 触发整数溢出 panic
    let start = (off as usize).min(data.len());
    let end = start.saturating_add(entry.size as usize).min(data.len());
    Ok(data[start..end].to_vec())
}

/// 打包目录为 asar。返回文件数。
pub fn pack(src_dir: &Path, out: &Path) -> Result<usize, String> {
    // 收集相对路径 → 绝对路径
    let mut files: BTreeMap<String, std::path::PathBuf> = BTreeMap::new();
    for entry in walkdir::WalkDir::new(src_dir).sort_by_file_name() {
        let p = entry.map_err(|e| e.to_string())?;
        if p.file_type().is_file() {
            let rel = p
                .path()
                .strip_prefix(src_dir)
                .map_err(|e| e.to_string())?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            files.insert(rel, p.path().to_path_buf());
        }
    }
    // 构建带 offset/size 的 header
    let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
    for (rel, p) in &files {
        sizes.insert(rel.clone(), p.metadata().map_err(|e| e.to_string())?.len());
    }
    let mut header = serde_json::json!({"files": {}});
    let mut off = 0u64;
    // 数据区必须**按头表分配 offset 的同一顺序**写入：头表先排当前层叶子、再递归子目录，
    // 与 `files.values()` 的全局字典序并不一致——两者不一致会让解包端按 offset 取到错位内容。
    let mut order: Vec<String> = Vec::new();
    build_header(header.get_mut("files").unwrap(), &sizes, "", &mut off, &mut order)?;

    fn build_header(
        node: &mut serde_json::Value,
        sizes: &BTreeMap<String, u64>,
        prefix: &str,
        off: &mut u64,
        order: &mut Vec<String>,
    ) -> Result<(), String> {
        // 收集当前层应包含的子项（目录 + 文件）
        let mut dirs: Vec<String> = Vec::new();
        let mut leaves: Vec<String> = Vec::new();
        let prefix_slash = if prefix.is_empty() { String::new() } else { format!("{prefix}/") };
        for rel in sizes.keys() {
            if !rel.starts_with(&prefix_slash) || (rel.len() <= prefix.len() && !prefix.is_empty()) {
                continue;
            }
            let rest = &rel[prefix_slash.len()..];
            match rest.find('/') {
                Some(i) => {
                    let dir = rest[..i].to_string();
                    if !dirs.contains(&dir) {
                        dirs.push(dir);
                    }
                }
                None => leaves.push(rest.to_string()),
            }
        }
        dirs.sort();
        leaves.sort();
        let obj = node.as_object_mut().ok_or("节点不是对象")?;
        for leaf in leaves {
            let rel = format!("{prefix_slash}{leaf}");
            let size = *sizes.get(&rel).ok_or("size 缺失")?;
            obj.insert(leaf, serde_json::json!({"size": size, "offset": off.to_string()}));
            *off += size;
            order.push(rel);
        }
        for dir in dirs {
            let mut child = serde_json::json!({"files": {}});
            // 注意：传入的是 child["files"]（build_header 的 node 参数是文件表本身）
            build_header(child.get_mut("files").unwrap(), sizes, &format!("{prefix_slash}{dir}"), off, order)?;
            obj.insert(dir, child);
        }
        Ok(())
    }

    let header_str = serde_json::to_string(&header).map_err(|e| e.to_string())?;
    let json_len = header_str.len();
    let pad = (4 - json_len % 4) % 4;
    // Chromium pickle 布局：[u32=4][u32=头部pickle总长][u32=头部pickle载荷长][u32=JSON长][JSON][补齐]
    // 第 3 个字段是**载荷长**（= 总长 - 4，即 [JSON长][JSON][补齐]），不是总长本身——
    // 早先这里误写成总长（多 4 字节），Electron 读字符串时容错所以没暴露，但对不上真实格式。
    let payload_len = json_len + pad + 4;
    let header_total = payload_len + 4; // [u32 JSON长度][JSON][补齐]
    let mut f = fs::File::create(out).map_err(|e| e.to_string())?;
    f.write_all(&4u32.to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&(header_total as u32).to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&(payload_len as u32).to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&(json_len as u32).to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(header_str.as_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&[0u8; 8][..pad]).map_err(|e| e.to_string())?;
    for rel in &order {
        let p = files.get(rel).ok_or("路径缺失")?;
        let content = fs::read(p).map_err(|e| e.to_string())?;
        f.write_all(&content).map_err(|e| e.to_string())?;
    }
    Ok(files.len())
}

// ---------- 原位改写：往已有档案里打补丁 / 追加条目（不解包、不整包进内存） ----------

/// 读取头部 JSON 原文（保留字段与结构，便于改写后回写），返回 (JSON, 数据区起点, 文件总长)。
pub fn read_header_value(f: &mut fs::File) -> Result<(serde_json::Value, u64, u64), String> {
    let file_len = f.metadata().map_err(|e| e.to_string())?.len();
    let mut head = [0u8; 16];
    f.read_exact(&mut head).map_err(|e| e.to_string())?;
    if u32::from_le_bytes(head[0..4].try_into().unwrap()) != 4 {
        return Err("不是 asar 文件".into());
    }
    let header_total = u32::from_le_bytes(head[4..8].try_into().unwrap()) as u64;
    let json_size = u32::from_le_bytes(head[12..16].try_into().unwrap()) as u64;
    if 8 + header_total > file_len || 16 + json_size > file_len {
        return Err("asar 头部越界".into());
    }
    let mut js = vec![0u8; json_size as usize];
    f.read_exact(&mut js).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_slice(&js).map_err(|e| e.to_string())?;
    Ok((v, 8 + header_total, file_len))
}

/// 取「根 files 下的嵌套节点」；`rel` 用 `/` 分隔、不带前导斜杠（与 `parse` 的 key 一致）。
fn node_mut<'a>(mut cur: &'a mut serde_json::Value, rel: &str) -> Option<&'a mut serde_json::Value> {
    for seg in rel.split('/') {
        cur = cur.get_mut("files")?.get_mut(seg)?;
    }
    Some(cur)
}

/// 收集所有**占用数据区**的条目的 (offset, size, 相对路径)。
fn collect_files(node: &serde_json::Value, prefix: &str, out: &mut Vec<(u64, u64, String)>) {
    let Some(files) = node.get("files").and_then(|v| v.as_object()) else { return };
    for (name, child) in files {
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if child.get("files").and_then(|v| v.as_object()).is_some() {
            collect_files(child, &rel, out);
        } else {
            if child.get("unpacked").and_then(|v| v.as_bool()).unwrap_or(false) {
                continue; // 存放于 app.asar.unpacked，不占数据区
            }
            let Some(off) = child.get("offset").and_then(|v| v.as_str()).and_then(|s| s.parse::<u64>().ok())
            else {
                continue;
            };
            let size = child.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            out.push((off, size, rel));
        }
    }
}

fn copy_n(f: &mut fs::File, out: &mut fs::File, mut n: u64, buf: &mut [u8]) -> Result<(), String> {
    while n > 0 {
        let want = n.min(buf.len() as u64) as usize;
        f.read_exact(&mut buf[..want]).map_err(|e| e.to_string())?;
        out.write_all(&buf[..want]).map_err(|e| e.to_string())?;
        n -= want as u64;
    }
    Ok(())
}

/// 在已有 asar 上打补丁：`patches` 覆盖**已存在**条目（根级相对路径，如 `index.html`），
/// `adds` 在数据区末尾**追加**新条目（根级相对路径，如 `stool_translate.js`）。
///
/// 原理：asar 的数据区是各条目按 offset 紧凑拼接的（无空洞），所以「某条目变大」等价于
/// 「它之后的所有条目整体平移」。于是只改头部里的 offset/size，数据区原样流式复制，
/// 不做解包、不整包进内存，写出的档案**除补丁外与原档案逐字节相同**。
///
/// 为安全起见，数据区不紧凑（有空洞或 size 合计对不上）时直接拒绝，避免改写错位。
/// 返回 (原条目数, 新增条目数)。
pub fn patch_archive(
    src_path: &Path,
    out_path: &Path,
    patches: &[(String, Vec<u8>)],
    adds: &[(String, Vec<u8>)],
) -> Result<(usize, usize), String> {
    let mut f = fs::File::open(src_path).map_err(|e| format!("打开 {} 失败: {e}", src_path.display()))?;
    let (mut root, data_start, file_len) = read_header_value(&mut f)?;
    let data_len = file_len - data_start;

    // 1) 收集原有条目并校验数据区紧凑
    let mut files: Vec<(u64, u64, String)> = Vec::new();
    collect_files(&root, "", &mut files);
    files.sort_by_key(|a| a.0);
    let mut expect = 0u64;
    for (off, size, rel) in &files {
        if *off != expect {
            return Err(format!(
                "asar 数据区不紧凑（{rel} 偏移 {off}，期望 {expect}）—— 为安全起见不做原位改写"
            ));
        }
        expect = off + size;
    }
    if expect != data_len {
        return Err(format!("asar 数据区长度不符（条目合计 {expect}，实际 {data_len}）"));
    }

    // 2) 定位补丁目标，算出各自的 (旧偏移, 旧长度)
    struct P { off: u64, old: u64, data: Vec<u8> }
    let mut ps: Vec<P> = Vec::new();
    for (rel, content) in patches {
        let rec = files.iter().find(|t| &t.2 == rel).ok_or_else(|| format!("asar 内没有条目 {rel}"))?;
        if ps.iter().any(|p| p.off == rec.0) {
            return Err(format!("条目 {rel} 被重复指定补丁"));
        }
        ps.push(P { off: rec.0, old: rec.1, data: content.clone() });
    }
    ps.sort_by_key(|a| a.off);

    // 3) 更新原有条目的 offset（补丁自身的起始位置不变，只改 size）
    let shift_of = |off: u64| -> i64 {
        ps.iter().filter(|p| p.off + p.old <= off).map(|p| p.data.len() as i64 - p.old as i64).sum()
    };
    for (off, _, rel) in &files {
        let new_off = (*off as i64 + shift_of(*off)) as u64;
        if let Some(n) = node_mut(&mut root, rel) {
            if let Some(o) = n.get_mut("offset") {
                *o = serde_json::Value::String(new_off.to_string());
            }
        }
    }
    for p in &ps {
        let rel = &files.iter().find(|t| t.0 == p.off).ok_or("补丁目标丢失")?.2;
        if let Some(n) = node_mut(&mut root, rel) {
            if let Some(s) = n.get_mut("size") {
                *s = serde_json::Value::from(p.data.len() as u64);
            }
        }
    }

    // 4) 追加新条目：紧跟在「平移后的数据区」之后
    let total_delta: i64 = ps.iter().map(|p| p.data.len() as i64 - p.old as i64).sum();
    let mut add_off = (data_len as i64 + total_delta) as u64;
    for (rel, content) in adds {
        if rel.contains('/') {
            return Err(format!("新增条目只支持放在 asar 根目录（收到 {rel}）"));
        }
        let Some(obj) = root.get_mut("files").and_then(|v| v.as_object_mut()) else {
            return Err("asar 头部缺少 files 对象".into());
        };
        obj.insert(
            rel.clone(),
            serde_json::json!({ "size": content.len() as u64, "offset": add_off.to_string() }),
        );
        add_off += content.len() as u64;
    }

    // 5) 写新档案：头部（原文结构 + 新 offset/size）+ 数据区流式复制（补丁位置替换）
    let header_str = serde_json::to_string(&root).map_err(|e| e.to_string())?;
    let json_len = header_str.len();
    let pad = (4 - json_len % 4) % 4;
    let payload_len = json_len + pad + 4;
    let header_total = payload_len + 4;
    let mut out = fs::File::create(out_path).map_err(|e| format!("创建 {} 失败: {e}", out_path.display()))?;
    out.write_all(&4u32.to_le_bytes()).map_err(|e| e.to_string())?;
    out.write_all(&(header_total as u32).to_le_bytes()).map_err(|e| e.to_string())?;
    out.write_all(&(payload_len as u32).to_le_bytes()).map_err(|e| e.to_string())?;
    out.write_all(&(json_len as u32).to_le_bytes()).map_err(|e| e.to_string())?;
    out.write_all(header_str.as_bytes()).map_err(|e| e.to_string())?;
    out.write_all(&[0u8; 8][..pad]).map_err(|e| e.to_string())?;

    f.seek(SeekFrom::Start(data_start)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 20];
    let mut cur = 0u64;
    for p in &ps {
        if p.off < cur {
            return Err("补丁区间重叠".into());
        }
        copy_n(&mut f, &mut out, p.off - cur, &mut buf)?;
        out.write_all(&p.data).map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Current(p.old as i64)).map_err(|e| e.to_string())?;
        cur = p.off + p.old;
    }
    copy_n(&mut f, &mut out, data_len - cur, &mut buf)?;
    for (_, content) in adds {
        out.write_all(content).map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())?;
    Ok((files.len(), adds.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("stool_asar_test_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// pack → patch_archive → 逐条目读回：未打补丁的必须逐字节一致。
    #[test]
    fn test_patch_archive_roundtrip() {
        let d = tmpdir("patch");
        let src = d.join("src");
        fs::create_dir_all(src.join("data")).unwrap();
        fs::write(src.join("a.txt"), b"AAAA").unwrap();
        fs::write(src.join("data/b.bin"), vec![7u8; 300]).unwrap();
        fs::write(src.join("index.html"), b"<html><body>hi</body></html>").unwrap();

        let p1 = d.join("one.asar");
        pack(&src, &p1).unwrap();

        let patched_html = b"<html><body>hi<script src=\"stool_translate.js\"></script></body></html>".to_vec();
        let js = b"/* hook */".to_vec();
        let json = br#"{"a":"b"}"#.to_vec();
        let p2 = d.join("two.asar");
        let (n_old, n_add) = patch_archive(
            &p1,
            &p2,
            &[("index.html".to_string(), patched_html.clone())],
            &[
                ("stool_translate.js".to_string(), js.clone()),
                ("stool_translate.json".to_string(), json.clone()),
            ],
        )
        .unwrap();
        assert_eq!(n_old, 3);
        assert_eq!(n_add, 2);

        // 新档案能被自家解析器读回来
        let data = fs::read(&p2).unwrap();
        let (files, data_start) = parse_bytes(&data).unwrap();
        assert_eq!(files.len(), 5);
        assert_eq!(read_file(&data, data_start, &files["index.html"]).unwrap(), patched_html);
        assert_eq!(read_file(&data, data_start, &files["stool_translate.js"]).unwrap(), js);
        assert_eq!(read_file(&data, data_start, &files["stool_translate.json"]).unwrap(), json);
        // 未打补丁的条目必须原样
        assert_eq!(read_file(&data, data_start, &files["a.txt"]).unwrap(), b"AAAA");
        assert_eq!(read_file(&data, data_start, &files["data/b.bin"]).unwrap(), vec![7u8; 300]);
        let _ = fs::remove_dir_all(&d);
    }

    /// 头部第 3 个字段必须是「载荷长 = 总长 - 4」（真实 pickle 布局），不是总长。
    #[test]
    fn test_header_pickle_layout() {
        let d = tmpdir("hdr");
        let src = d.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("x.txt"), b"hello").unwrap();
        let out = d.join("a.asar");
        pack(&src, &out).unwrap();
        let b = fs::read(&out).unwrap();
        let ht = u32::from_le_bytes(b[4..8].try_into().unwrap());
        let payload = u32::from_le_bytes(b[8..12].try_into().unwrap());
        let json_len = u32::from_le_bytes(b[12..16].try_into().unwrap());
        assert_eq!(payload, ht - 4, "第 3 字段应为载荷长");
        assert!(payload >= json_len + 4, "载荷须容得下 JSON 长度字段与 JSON");
        let _ = fs::remove_dir_all(&d);
    }
}
