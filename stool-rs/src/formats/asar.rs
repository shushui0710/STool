//! Electron asar 归档（解析/解包/打包）。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
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

pub fn read_file(data: &[u8], data_start: u64, entry: &AsarNode) -> Result<Vec<u8>, String> {
    if entry.unpacked {
        return Err("该文件存储于 app.asar.unpacked 目录".into());
    }
    let off = data_start + entry.offset.parse::<u64>().map_err(|e| e.to_string())?;
    let start = off as usize;
    let end = (start + entry.size as usize).min(data.len());
    Ok(data[start.min(data.len())..end].to_vec())
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
    build_header(header.get_mut("files").unwrap(), &sizes, "", &mut off)?;

    fn build_header(
        node: &mut serde_json::Value,
        sizes: &BTreeMap<String, u64>,
        prefix: &str,
        off: &mut u64,
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
        }
        for dir in dirs {
            let mut child = serde_json::json!({"files": {}});
            // 注意：传入的是 child["files"]（build_header 的 node 参数是文件表本身）
            build_header(child.get_mut("files").unwrap(), sizes, &format!("{prefix_slash}{dir}"), off)?;
            obj.insert(dir, child);
        }
        Ok(())
    }

    let header_str = serde_json::to_string(&header).map_err(|e| e.to_string())?;
    let json_len = header_str.len();
    let pad = (4 - json_len % 4) % 4;
    let header_total = json_len + pad + 8; // [u32 JSON长度][JSON][补齐]
    let mut f = fs::File::create(out).map_err(|e| e.to_string())?;
    f.write_all(&4u32.to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&(header_total as u32).to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&(header_total as u32).to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&(json_len as u32).to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(header_str.as_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&[0u8; 8][..pad]).map_err(|e| e.to_string())?;
    for (_, p) in &files {
        let content = fs::read(p).map_err(|e| e.to_string())?;
        f.write_all(&content).map_err(|e| e.to_string())?;
    }
    Ok(files.len())
}
