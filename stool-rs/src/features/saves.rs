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
    /// MZ 专用：磁盘上是不是「UTF-8 包装」形态（见 [`mz_unwrap`]）。
    /// **回写必须照原样式**，否则游戏读不了。其它格式恒为 `false`。
    pub mz_wrapped: bool,
    pub root: serde_json::Value,
}

impl SaveDoc {
    /// 加载存档，自动识别格式并解析为 JSON 树。
    pub fn load(path: &Path) -> Result<SaveDoc, String> {
        let raw = fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
        let fmt = detect_format(path, &raw)?;
        let mut mz_wrapped = false;
        let root = match fmt {
            SaveFormat::JsonPlain => serde_json::from_slice(&raw).map_err(|e| format!("JSON 解析失败: {e}"))?,
            SaveFormat::MvSave => {
                let text = decode_mv(&raw)?;
                serde_json::from_str(&text).map_err(|e| format!("MV 存档 JSON 解析失败: {e}"))?
            }
            SaveFormat::MzSave => {
                let (v, wrapped) = decode_mz(&raw)?;
                mz_wrapped = wrapped;
                v
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
        Ok(SaveDoc { path: path.to_path_buf(), format: fmt, mz_wrapped, root })
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

    /// 回写原文件（自动备份为 `<原名>.stool.bak`）。
    ///
    /// 注意备份语义：这里**每次都覆盖**备份（= 「上一次写之前」的状态），
    /// 与 [`crate::settings::backup_once`]（已存在就不覆盖 = 「原件」）不同。
    /// 路径拼接复用 [`crate::settings::backup_path_for`]，别在这里再拼一遍。
    pub fn save(&self) -> Result<String, String> {
        if !self.format.writable() {
            return Err("该格式为只读视图，无法回写".into());
        }
        let bak = crate::settings::backup_path_for(&self.path);
        fs::copy(&self.path, &bak).map_err(|e| format!("备份失败: {e}"))?;
        let bytes = self.encode()?;
        fs::write(&self.path, bytes).map_err(|e| e.to_string())?;
        Ok(format!("已写回 {}（备份 {}）", self.path.display(), bak.display()))
    }

    /// 把当前 JSON 树按本格式编码成**磁盘字节**（不写盘）。
    ///
    /// 与 [`SaveDoc::save`] 分开是为了**备份语义**：`save` 的备份是
    /// 「上一次写之前」（无条件覆盖，配界面的「还原到改之前」），
    /// 而解锁路线那种**可反复执行**的批量写要的是「首次写之前的原件」
    /// （`settings::backup_once`，已存在就不动）。所以那边先自己备份，再用这里拿字节。
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        if !self.format.writable() {
            return Err("该格式为只读视图，无法编码回写".into());
        }
        match self.format {
            SaveFormat::JsonPlain => serde_json::to_string_pretty(&self.root)
                .map(String::into_bytes)
                .map_err(|e| e.to_string()),
            SaveFormat::MvSave => {
                let compact = serde_json::to_string(&self.root).map_err(|e| e.to_string())?;
                crate::formats::lzstring::compress_to_base64(&compact)
                    .map(String::into_bytes)
                    .ok_or_else(|| "lz-string 压缩失败".to_string())
            }
            SaveFormat::MzSave => {
                use std::io::Write;
                let compact = serde_json::to_string(&self.root).map_err(|e| e.to_string())?;
                let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                z.write_all(compact.as_bytes()).map_err(|e| e.to_string())?;
                let zlib = z.finish().map_err(|e| e.to_string())?;
                // 照原样式回写：进来的文件是「UTF-8 包装」形态就再包一次（MZ 在 NW.js 下默认如此），
                // 是裸 zlib 就保持裸 zlib。**包错方向 = 游戏直接读不了这个存档。**
                Ok(if self.mz_wrapped { mz_wrap(&zlib) } else { zlib })
            }
            _ => Err("该格式为只读视图，无法编码回写".into()),
        }
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
    Err(unrecognized_hint(path, raw, &ext))
}

/// 认不出来时给「原因 + 出路」，而不是一句干巴巴的报错。
///
/// 分两档，因为**对策完全不同**：
///
/// - **疑似加密 / 整体压缩**：说清本工具**不破解他人加密方案**
///   （`docs/ARCHITECTURE.md` §7），只给能做的（游戏自带导入导出、备份、
///   Unity 注册表路线）。不要在这里给出任何"我可以试着解一下"的暗示。
/// - 其它未知格式：给通用提示。
///
/// 起因（2026-09-27）：用户报「无法修改存档 <Unity 游戏>」，查实是
/// `MarsSDK` 的 `AESCryptography` 把存档包住了 —— 这不是 bug，是**保护类加密**。
/// 旧文案只有一句「无法识别的存档格式（.sav）」+ 一条误导性提示
/// （建议去看 PlayerPrefs，而该作的 PlayerPrefs 里一个进度键都没有）。
fn unrecognized_hint(path: &Path, raw: &[u8], ext: &str) -> String {
    let what = if ext.is_empty() { "无扩展名".to_string() } else { format!(".{ext}") };
    if looks_encrypted(raw) {
        let unity = if is_unity_game_near(path) {
            "旁边就是一款 Unity 游戏 —— 进度多半就锁在这份文件里，注册表里通常没有。\n"
        } else {
            ""
        };
        return format!(
            "无法识别的存档格式（{what}）：内容疑似被加密或整体压过（长度是 16 的倍数、熵接近满值）。\n\
             {unity}本工具不破解他人的加密方案：这类存档只给提示，不尝试解密、也不注入游戏进程。\n\
             可以做的：\n\
             · 游戏若自带「导入 / 导出存档」或调试菜单，走它自己的出口（唯一不改格式的路）；\n\
             · 若是 Unity 游戏，先试「全CG解锁」页的注册表路线（部分作品把 CG / 进度放在 PlayerPrefs）；\n\
             · 改之前自己再留一份原文件 —— 本工具写文件前也会自动备份成 .stool.bak。"
        );
    }
    format!(
        "无法识别的存档格式（{what}）。\n\
         提示：Flash .sol 用 JPEXS Free；注册表存档用 regedit；若是 Unity 游戏，\
         先看「全CG解锁」页的注册表路线。"
    )
}

/// 「疑似被加密或整体压缩」的**保守**粗判：长度是 16 的倍数 + 熵接近满值。
///
/// ⚠️ 用途**只有一个**：换一条更准确的提示语。它**不是格式判据**，绝不能
/// 拿它去决定「要不要动手改这个文件」—— 真正的写入口径仍由 `SaveFormat` 决定。
///
/// 实测标定（2026-09-27，见 `tests/fixtures/unity_encrypted_sav/`）：
///
/// | 样本 | 长度 | 前 4096 熵 |
/// |---|---|---|
/// | Unity 加密存档（真机） | 117200 | 7.955 |
/// | 同上，截前 1024 | 1024 | 7.831 |
/// | 真随机（`os.urandom`） | 4096 | 7.952 |
/// | MZ 存档 `shared.rmmzsave` | 147 | 5.467 |
/// | `System.json` | 9823 | 5.409 |
/// | `README.md` | 44316 | 6.371 |
///
/// 所以 `7.5` 这个阈值两头都有 ~1.1 bit 的余量。**故意留的两个盲区**（宁可漏判）：
///
/// - **小于 1024 字节的文件不判**：`system.sav`(192 B) 与
///   `settings.conf`(144 B) 其实同样是密文，但小样本的熵估计天然偏低
///   （实测只有 6.85 / 6.75），要收进来就得把阈值降到 6.5 —— 那时
///   `README.md`(6.371) 只差 0.13 就误判了。不值当。
/// - **长度不是 16 的倍数就不判**：真加密（PKCS7 填充）必然是 16 的倍数；
///   这条能挡掉一批「高熵但其实是压缩」的误判。代价是流密码 / 无填充的
///   方案认不出（那也只是退回通用提示，不会出错）。
fn looks_encrypted(raw: &[u8]) -> bool {
    // 最小长度：1024 字节时随机数据实测熵 7.831，离 7.5 有 0.33 的余量；
    // 再短（如 256 B 的 ~7.28）就会掉到阈值以下、判不准。
    const ENCRYPT_MIN_LEN: usize = 1024;
    // 只看开头这一段：密文的熵不需要整个文件来证明，而大存档读全量是浪费。
    const WINDOW: usize = 4096;
    // 阈值：见上方标定表。
    const ENTROPY_LINE: f64 = 7.5;

    // 长度不是 16 的倍数 → 不判（真加密有 PKCS7 填充，长度必然是 16 的倍数）。
    // 用 `is_multiple_of` 而不是 `% 16 == 0`：clippy 的 `manual_is_multiple_of`
    // 在 `-D warnings` 下会直接失败（2026-09-27 踩）。
    if raw.len() < ENCRYPT_MIN_LEN || !raw.len().is_multiple_of(16) {
        return false;
    }
    shannon_entropy(&raw[..raw.len().min(WINDOW)]) > ENTROPY_LINE
}

/// 香农熵（bits/byte，0..=8）。8.0 附近 ≈ 加密或已压缩。
fn shannon_entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut hist = [0usize; 256];
    for &b in data {
        hist[b as usize] += 1;
    }
    let n = data.len() as f64;
    let mut ent = 0.0f64;
    for &c in hist.iter() {
        if c > 0 {
            let p = c as f64 / n;
            ent -= p * p.log2();
        }
    }
    ent
}

/// 存档附近是不是 Unity 游戏：往上找 `UnityPlayer.dll` 或 `*_Data` 目录。
///
/// 复用 [`super::gallery::find_data_dir`]（`_Data` 目录的唯一判据就是它，
/// 别再写第二份）。纯粹是**提示语增强**：任何 IO 失败都当作「不是」，
/// **绝不**因为这个判断改变行为。
fn is_unity_game_near(path: &Path) -> bool {
    /// 只往上找 3 层：`<root>/SaveData/x.sav` 是 2 层，`<root>/a/b/x.sav` 也够。
    /// 存档在 `AppData\LocalLow\<公司>\<产品>\` 时找不到，这是已知的漏判。
    const UP: usize = 3;
    path.ancestors()
        .skip(1)
        .take(UP)
        .any(|dir| dir.join("UnityPlayer.dll").is_file() || super::gallery::find_data_dir(dir).is_some())
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

/// RPG Maker MZ 的存档在磁盘上比「裸 zlib」多一层 **UTF-8 包装** ——
/// 这是 MZ 的**原生行为，不是加密**。原版 `js/rmmz_managers.js` 就长这样：
///
/// ```text
/// pako.deflate(json, { to: "string", level: 1 })   // 「二进制字符串」，1 字符 = 1 字节
///   ↓ StorageManager.saveToLocalFile → fs.writeFile(path, zip)   // 字符串按 UTF-8 写盘
/// ```
///
/// 于是每个 ≥ 0x80 的字节被展开成 **2 个**字节（`0xAD` → `C2 AD`）。
/// 游戏自己读写能 round-trip（读回来 utf8 解码仍是同一个二进制字符串），
/// 但**外部工具直接 `zlib.inflate` 一定失败** —— 报的就是
/// `corrupt deflate stream` / `invalid distance too far back`。
///
/// 实测（2026-09-27，`save/*.rmmzsave` 共 5 个文件）：直接 inflate **5/5 全挂**，
/// 先过这一层还原后 **5/5 解开**。所以这一步不是可选项。
///
/// 返回 `None` = 不是这层包装（含 >U+00FF 的字符，或压根不是合法 UTF-8）。
pub fn mz_unwrap(raw: &[u8]) -> Option<Vec<u8>> {
    let s = std::str::from_utf8(raw).ok()?;
    if s.chars().any(|c| c as u32 > 0xFF) {
        return None;
    }
    Some(s.chars().map(|c| c as u8).collect())
}

/// [`mz_unwrap`] 的逆：把裸 zlib 字节包成 MZ 在磁盘上的样子。
///
/// **回写必须照原样式**，否则游戏按 utf8 读回来拿到的是错的字节，
/// 直接读不了这个存档（比不写还糟）。
pub fn mz_wrap(zlib: &[u8]) -> Vec<u8> {
    let mut s = String::with_capacity(zlib.len() * 2);
    for &b in zlib {
        s.push(b as char);
    }
    s.into_bytes()
}

/// 把一段字节当 zlib 流解成字符串。
fn inflate_to_string(data: &[u8]) -> Result<String, String> {
    use std::io::Read;
    let mut z = flate2::read::ZlibDecoder::new(data);
    let mut out = String::new();
    z.read_to_string(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// MZ 解码：返回 (JSON 树, 磁盘上是否为 UTF-8 包装形态)。
///
/// **两种形态都试，谁能解成合法 JSON 就用谁** —— 这样"挑选"和"校验"是同一步，
/// 不存在「蒙对了一个形态、后面 JSON 才炸」的空档。
/// 顺序上先裸流后包装：少数工具导出的确实是裸 zlib，先命中最省事。
fn decode_mz(raw: &[u8]) -> Result<(serde_json::Value, bool), String> {
    let unwrapped = mz_unwrap(raw);
    let variants: [(&str, bool, &[u8]); 2] = [
        ("裸 zlib", false, raw),
        ("MZ 的 UTF-8 包装", true, unwrapped.as_deref().unwrap_or(&[])),
    ];
    let mut errs: Vec<String> = Vec::new();
    for (label, wrapped, data) in variants {
        if data.is_empty() {
            continue;
        }
        let parsed = inflate_to_string(data).and_then(|t| {
            serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string())
        });
        match parsed {
            Ok(v) => return Ok((v, wrapped)),
            Err(e) => errs.push(format!("[{label}] {e}")),
        }
    }
    Err(format!("MZ 存档解码失败（两种形态都试过）—— {}", errs.join("；")))
}

// ---------------------------------------------------------------------------
// MZ 的「开关」读写（全 CG 解锁路线要用）
// ---------------------------------------------------------------------------

/// MZ 存档里「开关」的两种存放形态 —— **实测都真实存在，必须都支持**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MzSwitchLayout {
    /// 进度存档 `fileN.rmmzsave`：`switches._data` 是**数组**，**下标 = 开关 ID**
    /// （`_data[0]` 恒为 `null` 占位，与 `Game_Switches` 一一对应）。
    Indexed,
    /// 跨存档共享（插件 `DarkPlasma_SharedSwitchVariable` 等写的 `shared.rmmzsave`）：
    /// `switches` 是 `[{"id":65,"value":true}, …]` —— **只列插件关心的那几个**。
    Entries,
}

impl MzSwitchLayout {
    pub fn label(&self) -> &'static str {
        match self {
            MzSwitchLayout::Indexed => "进度存档（下标即开关 ID）",
            MzSwitchLayout::Entries => "跨存档共享表（id/value 项）",
        }
    }
}

/// 判定这份 MZ 存档的开关表是哪种形态；`None` = 这份存档里没有开关表。
pub fn mz_switch_layout(root: &serde_json::Value) -> Option<MzSwitchLayout> {
    let sw = root.get("switches")?;
    if sw.get("_data").map(|d| d.is_array()).unwrap_or(false) {
        return Some(MzSwitchLayout::Indexed);
    }
    if sw.is_array() {
        return Some(MzSwitchLayout::Entries);
    }
    None
}

/// 读出这份存档里**已为真**的开关 ID（升序）。
pub fn mz_switch_ids_on(root: &serde_json::Value, layout: MzSwitchLayout) -> Vec<u32> {
    let mut out: Vec<u32> = match layout {
        MzSwitchLayout::Indexed => root
            .pointer("/switches/_data")
            .and_then(|d| d.as_array())
            .map(|a| {
                a.iter()
                    .enumerate()
                    .filter(|(_, v)| v.as_bool() == Some(true))
                    .map(|(i, _)| i as u32)
                    .collect()
            })
            .unwrap_or_default(),
        MzSwitchLayout::Entries => root
            .pointer("/switches")
            .and_then(|s| s.as_array())
            .map(|a| {
                a.iter()
                    .filter(|e| e.get("value").and_then(|v| v.as_bool()) == Some(true))
                    .filter_map(|e| e.get("id").and_then(|i| i.as_u64()))
                    .map(|i| i as u32)
                    .collect()
            })
            .unwrap_or_default(),
    };
    out.sort_unstable();
    out
}

/// 把这份存档里的指定开关置为 `on`，返回**真正发生改动**的 ID。
///
/// 语义上刻意保守：
/// - `Indexed` 形态下标越界时**补 `null` 扩到该下标**（不静默丢弃 —— 丢了就成了"假装解锁成功"）；
/// - `Entries` 形态**只改已存在的项**，不往里塞游戏没登记过的 id
///   （那张表是插件按配置过滤的，塞进去也未必生效，不如老老实实报"未覆盖"）。
pub fn mz_set_switches(
    root: &mut serde_json::Value,
    layout: MzSwitchLayout,
    ids: &[u32],
    on: bool,
) -> Vec<u32> {
    let mut done: Vec<u32> = Vec::new();
    match layout {
        MzSwitchLayout::Indexed => {
            let Some(arr) = root
                .pointer_mut("/switches/_data")
                .and_then(|d| d.as_array_mut())
            else {
                return done;
            };
            for &id in ids {
                let i = id as usize;
                if arr.len() <= i {
                    arr.resize(i + 1, serde_json::Value::Null);
                }
                if arr[i].as_bool() != Some(on) {
                    arr[i] = serde_json::Value::Bool(on);
                    done.push(id);
                }
            }
        }
        MzSwitchLayout::Entries => {
            let Some(arr) = root.pointer_mut("/switches").and_then(|s| s.as_array_mut()) else {
                return done;
            };
            for &id in ids {
                let hit = arr
                    .iter_mut()
                    .find(|e| e.get("id").and_then(|v| v.as_u64()) == Some(id as u64));
                if let Some(e) = hit {
                    if e.get("value").and_then(|v| v.as_bool()) != Some(on) {
                        e["value"] = serde_json::Value::Bool(on);
                        done.push(id);
                    }
                }
            }
        }
    }
    done
}

/// 收集一棵游戏目录里所有 MZ 存档文件（`save/`、`www/save/` 下，含递归一层）。
///
/// 只认 `.rmmzsave` / `.rmzsave` —— 与 [`detect_format`] 的扩展名规则一致，
/// 别在这里另立一套。
pub fn collect_mz_saves(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for rel in ["save", "www/save", "www/saves", "saves"] {
        let d = root.join(rel);
        if d.is_dir() {
            dirs.push(d);
        }
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let ext = p
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_lowercase();
            if ext == "rmmzsave" || ext == "rmzsave" {
                out.push(p);
            }
        }
    }
    out.sort();
    out
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
pub fn value_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 值 <-> 编辑文本：**唯一**口径
//
// 这三个原先是旧 egui 界面的私有工具。Tauri 侧要用就得抄一份，而「同一个意思、
// 两处实现」正是日后行为漂移的来源（两个界面改同一字段却解析规则不同）。
// 所以搬到这里做单一实现，界面侧只做转发。
// ---------------------------------------------------------------------------

/// 值 → 编辑框文本：字符串去引号，其余用 JSON 形式。
pub fn value_edit_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 编辑框文本 → 值：true/false/null → 对应类型，数字 → 数值，其余 → 字符串。
pub fn parse_edit_text(s: &str) -> serde_json::Value {
    let t = s.trim();
    match t {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        "null" => serde_json::Value::Null,
        _ => {
            if let Ok(i) = t.parse::<i64>() {
                serde_json::json!(i)
            } else if let Ok(f) = t.parse::<f64>() {
                serde_json::json!(f)
            } else {
                serde_json::Value::String(s.to_string())
            }
        }
    }
}

/// 值的类型提示（中文，供 UI 直接显示）；容器返回空串。
pub fn type_hint(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::String(_) => "文本",
        serde_json::Value::Number(_) => "数值",
        serde_json::Value::Bool(_) => "布尔",
        serde_json::Value::Null => "空",
        _ => "",
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
            mz_wrapped: false,
            root: json!({"arr": [1, 2], "obj": {"a": 1}, "scalar": 5}),
        };
        assert!(doc.set("/arr/5", json!(0)).is_err()); // 越界
        assert!(doc.set("/arr/x", json!(0)).is_err()); // 非数字下标
        assert!(doc.set("/scalar/a", json!(0)).is_err()); // 标量中间节点
        assert!(doc.set("/obj/a", json!(2)).is_ok()); // 正常路径
        assert_eq!(doc.root.pointer("/obj/a"), Some(&json!(2)));

        // 只读格式拒绝修改
        let mut ro = SaveDoc {
            path: PathBuf::from("x"),
            format: SaveFormat::Marshal,
            mz_wrapped: false,
            root: json!({}),
        };
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

    // ---- RPG Maker MZ 的 UTF-8 包装（2026-09-27 修）--------------------------
    //
    // 这批用例的固件**全是真机存档**（`tests/fixtures/mz_save/`，来源见其 README）。
    // 起因：真实 MZ 存档一律读不出来（`corrupt deflate stream`），而当时这里的单测
    // 用的是**自己造的裸 zlib 流** —— 测试全绿、真机全挂。所以刻意钉死三件事：
    //   ① 直接 inflate 必须失败（证明「包装」这层真实存在，别当它可选）；
    //   ② `mz_unwrap` 之后必须解开；
    //   ③ 改完回写必须**仍是**包装形态（写反了游戏直接读不了这个存档）。

    fn mz_fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("mz_save")
            .join(name)
    }

    #[test]
    fn mz_real_save_needs_utf8_unwrap() {
        for name in ["shared.rmmzsave", "global.rmmzsave"] {
            let raw = fs::read(mz_fixture(name)).unwrap();
            // ①直接 inflate 必须失败 —— 就是用户当初看到的那条报错
            assert!(
                inflate_to_string(&raw).is_err(),
                "{name} 居然能直接 inflate？固件被动过了（它必须保持「包装」形态）"
            );
            // ②还原后必须能解成合法 JSON
            let un = mz_unwrap(&raw).unwrap_or_else(|| panic!("{name} 应是 UTF-8 包装形态"));
            let text = inflate_to_string(&un).unwrap_or_else(|e| panic!("{name} 还原后仍解不开: {e}"));
            let v: serde_json::Value = serde_json::from_str(&text).expect("应是合法 JSON");
            assert!(!v.is_null(), "{name} 解出来是 null");
            // ③再包装回去要逐字节回原样（回写方向不能反）
            assert_eq!(mz_wrap(&un), raw, "{name} 包装/还原不是无损往返");
        }
    }

    #[test]
    fn mz_load_edit_write_keeps_wrapper() {
        // 复制到临时目录再改，别动固件本身
        let dir = std::env::temp_dir().join(format!("stool_mz_wrap_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("shared.rmmzsave");
        fs::copy(mz_fixture("shared.rmmzsave"), &p).unwrap();

        let mut doc = SaveDoc::load(&p).unwrap();
        assert_eq!(doc.format, SaveFormat::MzSave);
        assert!(doc.mz_wrapped, "从真机存档读出来应记为「包装形态」");
        assert!(doc.format.writable());

        // 改一个真值（shared 的第 0 个开关 = 65 ギャラリー解放）
        assert_eq!(doc.get("/switches/0/id"), Some(&json!(65)));
        doc.set("/switches/0/value", json!(false)).unwrap();
        doc.save().unwrap();
        assert!(crate::settings::backup_path_for(&p).exists(), "写前必须留备份");

        // 回写后的文件仍须是「包装」形态 —— 游戏是按 utf8 读回来再 inflate 的
        let back = fs::read(&p).unwrap();
        assert!(
            inflate_to_string(&back).is_err(),
            "回写成了裸 zlib：游戏读不了这个存档（比不写还糟）"
        );
        let un = mz_unwrap(&back).expect("回写后应仍是包装形态");
        let v: serde_json::Value = serde_json::from_str(&inflate_to_string(&un).unwrap()).unwrap();
        assert_eq!(v.pointer("/switches/0/value"), Some(&json!(false)));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mz_clean_zlib_still_reads_and_stays_clean() {
        // 少数工具导出的确实是裸 zlib：仍要能读，且回写**保持裸流**（别给它套上包装）
        let dir = std::env::temp_dir().join(format!("stool_mz_clean_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("slot3.rmmzsave");
        use std::io::Write;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(br#"{"switches":{"_data":[null,false,true]}}"#).unwrap();
        fs::write(&p, z.finish().unwrap()).unwrap();

        let mut doc = SaveDoc::load(&p).unwrap();
        assert_eq!(doc.format, SaveFormat::MzSave);
        assert!(!doc.mz_wrapped, "裸流不该被记成包装形态");
        doc.set("/switches/_data/1", json!(true)).unwrap();
        doc.save().unwrap();

        let back = fs::read(&p).unwrap();
        assert!(mz_unwrap(&back).is_none(), "裸流回写被套上包装了");
        let v: serde_json::Value = serde_json::from_str(&inflate_to_string(&back).unwrap()).unwrap();
        assert_eq!(v.pointer("/switches/_data/1"), Some(&json!(true)));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mz_switch_ops_cover_both_layouts() {
        // 形态 A：进度存档 —— `switches._data` 是数组、下标即开关 ID
        let mut progress = json!({
            "switches": { "_data": [null, false, true, false], "@": "Game_Switches" }
        });
        assert_eq!(mz_switch_layout(&progress), Some(MzSwitchLayout::Indexed));
        assert_eq!(mz_switch_ids_on(&progress, MzSwitchLayout::Indexed), vec![2]);

        // 越界必须**补 null 扩到该下标**，不能静默丢弃 —— 丢了就成了"假装解锁成功"
        let flipped = mz_set_switches(&mut progress, MzSwitchLayout::Indexed, &[2, 65], true);
        assert_eq!(flipped, vec![65], "2 本来就是 true，只有 65 才算改动");
        assert_eq!(progress.pointer("/switches/_data/65"), Some(&json!(true)));
        assert_eq!(
            progress.pointer("/switches/_data").unwrap().as_array().unwrap().len(),
            66,
            "应扩到下标 65（长度 66）"
        );

        // 形态 B：跨存档共享表 —— 直接拿真机 `shared.rmmzsave` 验
        let raw = fs::read(mz_fixture("shared.rmmzsave")).unwrap();
        let un = mz_unwrap(&raw).unwrap();
        let mut shared: serde_json::Value =
            serde_json::from_str(&inflate_to_string(&un).unwrap()).unwrap();
        assert_eq!(mz_switch_layout(&shared), Some(MzSwitchLayout::Entries));
        assert!(
            mz_switch_ids_on(&shared, MzSwitchLayout::Entries).contains(&65),
            "固件里 65（ギャラリー解放）本就是 true"
        );
        let flipped = mz_set_switches(&mut shared, MzSwitchLayout::Entries, &[66, 999], true);
        assert_eq!(flipped, vec![66], "66 在表里且原为 false → 翻；999 不在表里 → 不塞进去");
        assert_eq!(shared.pointer("/switches/1/value"), Some(&json!(true)));
    }

    // ---- 认不出来的存档：报错必须「分档 + 有出路」（2026-09-27 加）----------
    //
    // 起因：用户报「无法修改存档（Unity 游戏）」，查实是第三方 SDK 的 AES 把
    // 存档包住了 —— 不是 bug，是保护类加密。旧文案只有一句「无法识别的存档格式」
    // 加一条误导性提示（让去看 PlayerPrefs，而该作 PlayerPrefs 里一个进度键都没有）。
    //
    // 固件是**真机存档**的前 4096 字节（来源 / sha256 见其 README）。
    // 这批用例钉死三件事：
    //   ① 高熵密文要认出来，并**明说不破解**（ARCHITECTURE §7）；
    //   ② 小文件（<1024 B）与长度不合规的宁可退回通用提示，也不许瞎定性；
    //   ③ Unity 上下文只从路径推、且只影响提示语，不改变任何行为。

    /// 固件路径的**唯一**取法（改名就只改这里）。
    fn enc_fixture_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("unity_encrypted_sav")
            .join("gamesave_01_head4096.sav")
    }

    fn enc_fixture() -> Vec<u8> {
        fs::read(enc_fixture_path()).unwrap()
    }

    /// 每个用例一个干净的临时目录（同名先清掉，避免上一轮残留影响断言）。
    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_saves_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// 断言加载**必须失败**并返回内核文案。
    ///
    /// `SaveDoc` 没实现 `Debug`，用不了 `unwrap_err()`；而且成功时也不该把
    /// 整棵 JSON 树打到测试输出里 —— 这两个理由都指向这个助手，别改回 `unwrap_err`。
    fn load_err(p: &Path) -> String {
        match SaveDoc::load(p) {
            Ok(_) => panic!("{} 不该被识别成功", p.display()),
            Err(e) => e,
        }
    }

    #[test]
    fn looks_encrypted_only_fires_where_it_should() {
        let real = enc_fixture();
        assert!(looks_encrypted(&real), "真机密文必须判为疑似加密（熵 7.955）");
        assert!(looks_encrypted(&real[..1024]), "1024 字节窗口仍判得出（熵 7.831）");
        assert!(!looks_encrypted(&real[..512]), "低于 1024 字节不判（小样本熵估不准）");
        assert!(!looks_encrypted(&real[..4095]), "长度不是 16 的倍数不判");
        assert!(!looks_encrypted(&vec![0u8; 4096]), "全 0：长度合规但熵为 0");

        // 明文对照：长度凑成 16 的倍数的 JSON 也不能误判（熵 ~4.5）
        let mut plain = Vec::new();
        while plain.len() < 4096 {
            plain.extend_from_slice(b"{\"gold\":1000,\"name\":\"alice\"}  ");
        }
        plain.truncate(4096);
        assert!(!looks_encrypted(&plain), "明文 JSON 绝不能判成密文");
    }

    #[test]
    fn encrypted_save_says_so_and_that_we_wont_crack_it() {
        let dir = tmp_dir("enc_hint");
        let p = dir.join("gamesave_01.sav");
        fs::write(&p, enc_fixture()).unwrap();

        let err = load_err(&p);
        assert!(err.contains("疑似被加密"), "要说清是加密，而不是泛泛的『无法识别』：{err}");
        assert!(err.contains("不破解"), "必须写明不破解他人加密方案（ARCHITECTURE §7）：{err}");
        assert!(err.contains(".stool.bak"), "要给「先备份」这条真能做的出路：{err}");
        assert!(!err.contains("JPEXS"), "走了加密那一档就不该再给 Flash 提示：{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unity_context_is_read_from_the_path_only() {
        let dir = tmp_dir("enc_unity");
        // 独特措辞：通用提示里也含「Unity」二字，不能拿它当判据。
        let marker = "旁边就是一款 Unity 游戏";

        // 正向：`<game>/SaveData/x.sav` + 兄弟目录 `<game>/<game>_Data`（Unity 的目录特征）
        let game = dir.join("FakeGame");
        fs::create_dir_all(game.join("FakeGame_Data")).unwrap();
        let sd = game.join("SaveData");
        fs::create_dir_all(&sd).unwrap();
        let p = sd.join("gamesave_01.sav");
        fs::write(&p, enc_fixture()).unwrap();
        let err = load_err(&p);
        assert!(err.contains(marker), "存档就在 Unity 游戏目录里，应当点名：{err}");

        // 反向对照：同样的字节换个位置就不许点名。
        // 刻意埋深两层 —— `is_unity_game_near` 只往上找 3 层，这样它的候选
        // 目录全在我们自己的临时目录内，断言不会被 %TEMP% 里别人的 `*_Data` 干扰。
        let plain = dir.join("NoGame").join("deep");
        fs::create_dir_all(&plain).unwrap();
        let q = plain.join("gamesave_01.sav");
        fs::write(&q, enc_fixture()).unwrap();
        let err2 = load_err(&q);
        assert!(!err2.contains(marker), "没有 Unity 特征就不许乱点名：{err2}");
        assert!(err2.contains("疑似被加密"), "但加密判定本身不能跟着丢：{err2}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn small_or_unaligned_ciphertext_falls_back_to_generic_hint() {
        let dir = tmp_dir("enc_small");
        let real = enc_fixture();

        // 小加密文件：真机上 `system.sav`(192 B) / `settings.conf`(144 B) 正是这种形态。
        // 熵估不准 → 宁可退回通用提示，也不许瞎定性。
        let ps = dir.join("system.sav");
        fs::write(&ps, &real[..512]).unwrap();
        let err = load_err(&ps);
        assert!(err.contains("JPEXS"), "小文件应走通用提示：{err}");
        assert!(!err.contains("疑似被加密"), "小文件不许被定性成加密：{err}");

        // 高熵、但长度不是 16 的倍数（PKCS7 之后不可能出现这种长度）
        let pu = dir.join("odd.sav");
        fs::write(&pu, &real[..4095]).unwrap();
        let err = load_err(&pu);
        assert!(err.contains("JPEXS"), "长度不合规应走通用提示：{err}");
        assert!(!err.contains("疑似被加密"), "长度不合规不许被定性成加密：{err}");
        let _ = fs::remove_dir_all(&dir);
    }
}
