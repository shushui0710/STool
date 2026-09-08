//! 存档服务：定位、自动识别、JSON 树编辑、搜索、回写。
//!
//! 支持格式：
//! - RPG Maker MV（.rpgsave，lz-string 压缩 base64）—— 可读可写
//! - RPG Maker MZ（.rmzsave / .rmmzsave，zlib）—— 可读可写
//! - JSON 明文存档 —— 可读可写
//! - RPG Maker XP/VX/VX Ace（.rxdata/.rvdata/.rvdata2，Ruby Marshal）—— 只读
//! - Ren'Py persistent（pickle）—— 只读
//! - 未知扩展名时按内容嗅探（JSON / zlib / MV base64）

use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// 存档目录定位
// ---------------------------------------------------------------------------

pub fn list_files(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_file() {
            if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
                if exts.contains(&ext.to_lowercase().as_str()) {
                    out.push(p.to_path_buf());
                }
            }
        }
    }
    out.sort();
    out
}

/// 返回 [(标签, 目录)]。
pub fn find_save_locations(game_root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for rel in ["save", "www/save", "Saves", "Save"] {
        let p = game_root.join(rel);
        if p.is_dir() {
            out.push((format!("游戏目录/{rel}"), p));
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let rp = Path::new(&appdata).join("RenPy");
        if rp.is_dir() {
            if let Ok(rd) = fs::read_dir(&rp) {
                for d in rd.flatten() {
                    if d.path().is_dir() {
                        out.push((format!("RenPy/{}", d.file_name().to_string_lossy()), d.path()));
                    }
                }
            }
        }
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let ll = Path::new(&profile).join("AppData").join("LocalLow");
        if ll.is_dir() {
            if let Ok(rd) = fs::read_dir(&ll) {
                for d in rd.flatten() {
                    if d.path().is_dir() {
                        out.push((format!("LocalLow/{}", d.file_name().to_string_lossy()), d.path()));
                    }
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 存档文档：加载 → JSON 树 → 搜索 → 编辑 → 回写
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFormat {
    JsonPlain,
    MvSave,
    MzSave,
    Marshal,
    RenPyPersistent,
}

impl SaveFormat {
    pub fn label(&self) -> &'static str {
        match self {
            SaveFormat::JsonPlain => "JSON 明文存档",
            SaveFormat::MvSave => "RPG Maker MV 存档（lz-string）",
            SaveFormat::MzSave => "RPG Maker MZ 存档（zlib）",
            SaveFormat::Marshal => "Ruby Marshal（XP/VX/VX Ace，只读）",
            SaveFormat::RenPyPersistent => "Ren'Py persistent（只读）",
        }
    }

    pub fn writable(&self) -> bool {
        matches!(self, SaveFormat::JsonPlain | SaveFormat::MvSave | SaveFormat::MzSave)
    }
}

/// 搜索范围：全部 / 只搜键名 / 只搜值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    All,
    Keys,
    Values,
}

impl SearchScope {
    pub const ALL: [SearchScope; 3] = [SearchScope::All, SearchScope::Keys, SearchScope::Values];
    pub fn label(&self) -> &'static str {
        match self {
            SearchScope::All => "键名+值",
            SearchScope::Keys => "只搜键名",
            SearchScope::Values => "只搜值",
        }
    }
}

/// 一个已加载的存档文档。
pub struct SaveDoc {
    pub path: PathBuf,
    pub format: SaveFormat,
    pub root: serde_json::Value,
}

impl SaveDoc {
    /// 加载存档，自动识别格式并解析为 JSON 树。
    pub fn load(path: &Path) -> Result<SaveDoc, String> {
        let raw = fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
        let fmt = detect_format(path, &raw)?;
        let root = match fmt {
            SaveFormat::JsonPlain => serde_json::from_slice(&raw).map_err(|e| format!("JSON 解析失败: {e}"))?,
            SaveFormat::MvSave => {
                let text = decode_mv(&raw)?;
                serde_json::from_str(&text).map_err(|e| format!("MV 存档 JSON 解析失败: {e}"))?
            }
            SaveFormat::MzSave => {
                let text = decode_mz(&raw)?;
                serde_json::from_str(&text).map_err(|e| format!("MZ 存档 JSON 解析失败: {e}"))?
            }
            SaveFormat::Marshal => {
                // Ruby Marshal → JSON（只读视图）
                let s = crate::formats::marshal::to_json_string(&raw).map_err(|e| format!("Marshal 解析失败: {e}"))?;
                serde_json::from_str(&s).map_err(|e| format!("Marshal 转 JSON 失败: {e}"))?
            }
            SaveFormat::RenPyPersistent => {
                let start = raw.windows(2).position(|w| w == [0x80, 0x02]).unwrap_or(0);
                let v = crate::formats::pickle::loads(&raw[start..]).map_err(|e| format!("persistent 解析失败: {e}"))?;
                pickle_value_to_json(&v, 0)
            }
        };
        Ok(SaveDoc { path: path.to_path_buf(), format: fmt, root })
    }

    /// 在树中搜索键名或值包含 `query` 的路径（不区分大小写），返回 JSON Pointer 列表。
    pub fn search(&self, query: &str, scope: SearchScope) -> Vec<String> {
        let q = query.to_lowercase();
        let mut out = Vec::new();
        if !q.is_empty() {
            search_node(&self.root, "", &q, scope, &mut out, 0);
        }
        out.truncate(500);
        out
    }

    /// 按 JSON Pointer 取值（如 /system/变量名 或 /items/3/count）。
    pub fn get(&self, pointer: &str) -> Option<&serde_json::Value> {
        self.root.pointer(pointer)
    }

    /// 按 JSON Pointer 设置值。pointer 指向对象时返回错误。
    pub fn set(&mut self, pointer: &str, new_val: serde_json::Value) -> Result<(), String> {
        if !self.format.writable() {
            return Err("该格式为只读视图，无法修改".into());
        }
        if pointer.is_empty() || pointer == "/" {
            self.root = new_val;
            return Ok(());
        }
        let path = pointer.trim_start_matches('/');
        let parts: Vec<&str> = path.split('/').collect();
        let mut node: &mut serde_json::Value = &mut self.root;
        for (i, raw_seg) in parts.iter().enumerate() {
            let seg = raw_seg.replace("~1", "/").replace("~0", "~");
            let last = i == parts.len() - 1;
            let cur = node;
            match cur {
                serde_json::Value::Object(obj) => {
                    if last {
                        obj.insert(seg, new_val);
                        return Ok(());
                    }
                    node = obj.entry(seg).or_insert(serde_json::Value::Null);
                }
                serde_json::Value::Array(arr) => {
                    let idx: usize = seg.parse().map_err(|_| format!("数组下标不合法: {seg}"))?;
                    if last {
                        if idx >= arr.len() {
                            return Err(format!("数组下标越界: {idx}（长度 {}）", arr.len()));
                        }
                        arr[idx] = new_val;
                        return Ok(());
                    }
                    node = arr.get_mut(idx).ok_or_else(|| format!("数组下标越界: {idx}"))?;
                }
                _ => return Err(format!("路径 {} 处不是对象或数组", parts[..i].join("/"))),
            }
        }
        Ok(())
    }

    /// 回写原文件（自动备份为 <原名>.stool.bak）。
    pub fn save(&self) -> Result<String, String> {
        if !self.format.writable() {
            return Err("该格式为只读视图，无法回写".into());
        }
        let bak = {
            let mut s = self.path.as_os_str().to_os_string();
            s.push(".stool.bak");
            PathBuf::from(s)
        };
        fs::copy(&self.path, &bak).map_err(|e| format!("备份失败: {e}"))?;
        let compact = serde_json::to_string(&self.root).map_err(|e| e.to_string())?;
        match self.format {
            SaveFormat::JsonPlain => {
                let pretty = serde_json::to_string_pretty(&self.root).map_err(|e| e.to_string())?;
                fs::write(&self.path, pretty).map_err(|e| e.to_string())?;
            }
            SaveFormat::MvSave => {
                let enc = crate::formats::lzstring::compress_to_base64(&compact)
                    .ok_or("lz-string 压缩失败")?;
                fs::write(&self.path, enc).map_err(|e| e.to_string())?;
            }
            SaveFormat::MzSave => {
                use std::io::Write;
                let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                z.write_all(compact.as_bytes()).map_err(|e| e.to_string())?;
                fs::write(&self.path, z.finish().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            }
            _ => unreachable!(),
        }
        Ok(format!("已写回 {}（备份 {}）", self.path.display(), bak.display()))
    }
}

/// 格式识别：先看扩展名，再看内容。
fn detect_format(path: &Path, raw: &[u8]) -> Result<SaveFormat, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name == "persistent" {
        return Ok(SaveFormat::RenPyPersistent);
    }
    match ext.as_str() {
        "rpgsave" => return Ok(SaveFormat::MvSave),
        "rmmzsave" | "rmzsave" => return Ok(SaveFormat::MzSave),
        "json" => return Ok(SaveFormat::JsonPlain),
        "rxdata" | "rvdata" | "rvdata2" => return Ok(SaveFormat::Marshal),
        _ => {}
    }
    // 内容嗅探
    let trimmed = trim_bom(raw);
    if trimmed.starts_with(b"{") || trimmed.starts_with(b"[") {
        return Ok(SaveFormat::JsonPlain);
    }
    if trimmed.starts_with(&[0x78]) {
        // zlib 头 0x78 01/9C/DA
        return Ok(SaveFormat::MzSave);
    }
    if trimmed.starts_with(&[0x80, 0x02]) {
        return Ok(SaveFormat::RenPyPersistent);
    }
    if looks_like_base64(trimmed) {
        return Ok(SaveFormat::MvSave);
    }
    if raw.first() == Some(&0x04) {
        return Ok(SaveFormat::Marshal);
    }
    Err(format!(
        "无法识别的存档格式（.{}）。\n提示：Flash .sol 请用 JPEGS Free（JPEXS）；注册表存档请用 regedit；Unity 可先解包看 PlayerPrefs。",
        ext
    ))
}

fn trim_bom(raw: &[u8]) -> &[u8] {
    raw.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(raw)
}

fn looks_like_base64(raw: &[u8]) -> bool {
    if raw.is_empty() {
        return false;
    }
    raw.iter().all(|&b| {
        b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=' || b == b'\r' || b == b'\n'
    }) && raw.len() >= 8
}

fn decode_mv(raw: &[u8]) -> Result<String, String> {
    let text = String::from_utf8_lossy(raw);
    crate::formats::lzstring::decompress_from_base64(text.trim())
        .ok_or_else(|| "lz-string 解码失败（不是有效的 MV 存档）".into())
}

fn decode_mz(raw: &[u8]) -> Result<String, String> {
    use std::io::Read;
    let mut z = flate2::read::ZlibDecoder::new(raw);
    let mut out = String::new();
    z.read_to_string(&mut out).map_err(|e| format!("zlib 解码失败: {e}"))?;
    Ok(out)
}

fn search_node(node: &serde_json::Value, prefix: &str, q: &str, scope: SearchScope, out: &mut Vec<String>, depth: usize) {
    if out.len() >= 500 || depth > 12 {
        return;
    }
    match node {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let ptr = format!("{prefix}/{}", k.replace('/', "~1"));
                let key_hit = scope != SearchScope::Values && k.to_lowercase().contains(q);
                let val_hit = scope != SearchScope::Keys
                    && value_text(v).map(|t| t.to_lowercase().contains(q)).unwrap_or(false);
                if key_hit || val_hit {
                    out.push(ptr.clone());
                }
                search_node(v, &ptr, q, scope, out, depth + 1);
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                let ptr = format!("{prefix}/{i}");
                let val_hit = scope != SearchScope::Keys
                    && value_text(v).map(|t| t.to_lowercase().contains(q)).unwrap_or(false);
                if val_hit {
                    out.push(ptr.clone());
                }
                search_node(v, &ptr, q, scope, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// 标量值的文本表示（用于搜索命中），容器返回 None。
fn value_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn pickle_value_to_json(v: &crate::formats::pickle::Value, depth: usize) -> serde_json::Value {
    use crate::formats::pickle::Value;
    use serde_json::json;
    if depth > 12 {
        return json!("...");
    }
    match v {
        Value::None => serde_json::Value::Null,
        Value::Bool(b) => json!(b),
        Value::Int(i) => json!(i),
        Value::Float(f) => json!(f),
        Value::Str(s) => json!(s),
        Value::Bytes(b) => json!(String::from_utf8_lossy(b)),
        Value::List(l) => serde_json::Value::Array(l.iter().map(|x| pickle_value_to_json(x, depth + 1)).collect()),
        Value::Tuple(t) => serde_json::Value::Array(t.iter().map(|x| pickle_value_to_json(x, depth + 1)).collect()),
        Value::Dict(d) => {
            let mut m = serde_json::Map::new();
            for (i, (k, val)) in d.iter().enumerate() {
                let key = match k {
                    Value::Str(s) => s.clone(),
                    Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                    other => format!("#{i}:{other:?}"),
                };
                m.insert(key, pickle_value_to_json(val, depth + 1));
            }
            serde_json::Value::Object(m)
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp_json(root: &Path, name: &str) -> PathBuf {
        let p = root.join(name);
        fs::write(&p, serde_json::to_string_pretty(&json!({
            "system": { "gold": 1000, "playerName": "Alice" },
            "items": [ {"id": 1, "count": 3}, {"id": 2, "count": 9} ],
            "flags": [ true, false, true ]
        })).unwrap()).unwrap();
        p
    }

    #[test]
    fn json_load_search_edit_save() {
        let dir = std::env::temp_dir().join(format!("stool_saves_test_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = tmp_json(&dir, "a.json");

        let mut doc = SaveDoc::load(&p).unwrap();
        assert_eq!(doc.format, SaveFormat::JsonPlain);
        assert!(doc.format.writable());

        // 搜索：键名 gold / 值 Alice / 数值 9 都能命中
        let gold = doc.search("gold", SearchScope::All);
        assert_eq!(gold, vec!["/system/gold"]);
        let alice = doc.search("alice", SearchScope::Values);
        assert!(alice.iter().any(|x| x == "/system/playerName"));
        let nine = doc.search("9", SearchScope::Values);
        assert!(nine.iter().any(|x| x == "/items/1/count"));

        // 范围过滤：只搜键名时，值 "alice" 不应命中（但键名 gold 命中）
        assert!(doc.search("alice", SearchScope::Keys).is_empty());
        assert!(!doc.search("gold", SearchScope::Keys).is_empty());

        // 修改并回写
        doc.set("/system/gold", json!(99999)).unwrap();
        doc.set("/items/1/count", json!(0)).unwrap();
        doc.save().unwrap();

        // 重新加载验证 + 备份存在
        let doc2 = SaveDoc::load(&p).unwrap();
        assert_eq!(doc2.get("/system/gold"), Some(&json!(99999)));
        assert_eq!(doc2.get("/items/1/count"), Some(&json!(0)));
        assert!(p.with_extension("json.stool.bak").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_errors() {
        let mut doc = SaveDoc {
            path: PathBuf::from("x.json"),
            format: SaveFormat::JsonPlain,
            root: json!({"arr": [1, 2], "obj": {"a": 1}, "scalar": 5}),
        };
        assert!(doc.set("/arr/5", json!(0)).is_err()); // 越界
        assert!(doc.set("/arr/x", json!(0)).is_err()); // 非数字下标
        assert!(doc.set("/scalar/a", json!(0)).is_err()); // 标量中间节点
        assert!(doc.set("/obj/a", json!(2)).is_ok()); // 正常路径
        assert_eq!(doc.root.pointer("/obj/a"), Some(&json!(2)));

        // 只读格式拒绝修改
        let mut ro = SaveDoc { path: PathBuf::from("x"), format: SaveFormat::Marshal, root: json!({}) };
        assert!(ro.set("/a", json!(1)).is_err());
        assert!(ro.save().is_err());
    }

    #[test]
    fn sniff_unknown_extension() {
        let dir = std::env::temp_dir().join(format!("stool_saves_sniff_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // JSON 明文但扩展名怪异 → 嗅探为 JsonPlain
        let pj = dir.join("slot1.dat");
        fs::write(&pj, b"{\"a\":1}").unwrap();
        assert_eq!(SaveDoc::load(&pj).unwrap().format, SaveFormat::JsonPlain);
        // zlib 内容 → 嗅探为 MzSave
        use std::io::Write;
        let pz = dir.join("slot2.dat");
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(b"{}").unwrap();
        fs::write(&pz, z.finish().unwrap()).unwrap();
        assert_eq!(SaveDoc::load(&pz).unwrap().format, SaveFormat::MzSave);
        let _ = fs::remove_dir_all(&dir);
    }
}
