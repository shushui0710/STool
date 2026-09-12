//! 设置：外部工具路径与用户配置（~/.stool/config.json）。

use std::fs;
use std::path::{Path, PathBuf};

/// 当前配置结构版本。**字段语义变化时 +1**，并在 [`migrate`] 里补迁移分支。
/// 新增字段（纯追加）不必升版本 —— `#[serde(default)]` 已能兼容。
pub const CONFIG_VERSION: u32 = 1;

fn default_config_version() -> u32 {
    CONFIG_VERSION
}

fn default_proxy() -> String {
    "http://127.0.0.1:7892".into()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    /// 配置结构版本（旧配置缺该字段时按 0 处理，见 [`migrate`]）
    #[serde(default = "default_config_version")]
    pub config_version: u32,
    // 以下字段一律 `#[serde(default)]`：**局部缺失的配置也能加载**，
    // 避免"少一个字段 → 整体解析失败 → 设置全丢"。
    #[serde(default)]
    pub output_dir: String,
    #[serde(default)]
    pub garbro: String,
    #[serde(default)]
    pub asset_ripper: String,
    #[serde(default)]
    pub il2cpp_dumper: String,
    #[serde(default)]
    pub wolfdec: String,
    #[serde(default)]
    pub gdre_tools: String,
    #[serde(default)]
    pub unrpyc: String,
    #[serde(default)]
    pub python: String,
    #[serde(default = "default_proxy")]
    pub proxy: String,
    /// 机翻引擎（OpenAI 兼容）配置
    #[serde(default)]
    pub mtl_base_url: String,
    #[serde(default)]
    pub mtl_key: String,
    #[serde(default)]
    pub mtl_model: String,
    #[serde(default = "default_mtl_batch")]
    pub mtl_batch: u32,
    /// 机翻并发批数（网络型任务，默认 4；1 = 串行）
    #[serde(default = "default_mtl_jobs")]
    pub mtl_jobs: u32,
    /// 术语表：每行一条 `原文=译文`，注入机翻 system prompt 保证译名一致
    #[serde(default)]
    pub mtl_glossary: String,
}

fn default_mtl_batch() -> u32 {
    20
}

fn default_mtl_jobs() -> u32 {
    4
}

impl Default for Config {
    fn default() -> Self {
        Config {
            config_version: CONFIG_VERSION,
            output_dir: String::new(),
            garbro: String::new(),
            asset_ripper: String::new(),
            il2cpp_dumper: String::new(),
            wolfdec: String::new(),
            gdre_tools: String::new(),
            unrpyc: String::new(),
            python: String::new(),
            proxy: default_proxy(),
            mtl_base_url: String::new(),
            mtl_key: String::new(),
            mtl_model: String::new(),
            mtl_batch: default_mtl_batch(),
            mtl_jobs: default_mtl_jobs(),
            mtl_glossary: String::new(),
        }
    }
}

pub fn config_path() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|h| PathBuf::from(h).join(".stool").join("config.json"))
        .unwrap_or_else(|| PathBuf::from(".stool").join("config.json"))
}

/// 密钥字段的密文前缀：`enc:v1:<hex(DPAPI blob)>`。没有该前缀 = 旧配置里的明文。
pub const KEY_PREFIX: &str = "enc:v1:";

/// Windows DPAPI 加解密（`CryptProtectData` / `CryptUnprotectData`，无第三方依赖）。
///
/// 选它的理由：密钥由 Windows 按**当前登录用户**派生，本工具不需要自己管理主密钥，
/// 也不会有"主密钥和密文放在一起"的伪加密问题。
///
/// 代价：密文**绑定当前 Windows 用户**，把 `config.json` 拷到别的机器/账户会解不开 ——
/// 这种情况按"密钥失效"处理（告警 + 清空该字段），不影响其它设置加载。
#[cfg(windows)]
mod dpapi {
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };

    fn blob_of(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    fn empty() -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: 0, pbData: ptr::null_mut() }
    }

    /// 把内存块拷出来并释放 API 分配的内存（避免句柄泄漏）。
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as *mut core::ffi::c_void);
        v
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let input = blob_of(plain);
            let mut out = empty();
            let ok = CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            );
            if ok == 0 {
                return Err(format!("CryptProtectData 失败: {}", std::io::Error::last_os_error()));
            }
            Ok(take(out))
        }
    }

    pub fn unprotect(blob: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let input = blob_of(blob);
            let mut out = empty();
            let mut descr: *mut u16 = ptr::null_mut();
            let ok = CryptUnprotectData(
                &input,
                &mut descr,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            );
            if !descr.is_null() {
                LocalFree(descr as *mut core::ffi::c_void);
            }
            if ok == 0 {
                return Err(format!("CryptUnprotectData 失败: {}", std::io::Error::last_os_error()));
            }
            Ok(take(out))
        }
    }
}

#[cfg(not(windows))]
mod dpapi {
    pub fn protect(_plain: &[u8]) -> Result<Vec<u8>, String> {
        Err("DPAPI 仅在 Windows 上可用".into())
    }
    pub fn unprotect(_blob: &[u8]) -> Result<Vec<u8>, String> {
        Err("DPAPI 仅在 Windows 上可用".into())
    }
}

/// 明文密钥 → 落盘字符串（空串保持空串，不产生无意义的密文）。
pub fn encrypt_key(plain: &str) -> Result<String, String> {
    if plain.is_empty() {
        return Ok(String::new());
    }
    let blob = dpapi::protect(plain.as_bytes())?;
    Ok(format!("{KEY_PREFIX}{}", crate::hash::hex(&blob)))
}

/// 落盘字符串 → 明文密钥。无前缀的（旧配置）按明文原样返回。
pub fn decrypt_key(stored: &str) -> Result<String, String> {
    let Some(hexpart) = stored.strip_prefix(KEY_PREFIX) else {
        return Ok(stored.to_string());
    };
    let raw = hex_decode(hexpart).ok_or("密钥字段不是合法的十六进制")?;
    let bytes = dpapi::unprotect(&raw)?;
    String::from_utf8(bytes).map_err(|e| format!("解密结果不是 UTF-8: {e}"))
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..b.len()).step_by(2) {
        let hi = (b[i] as char).to_digit(16)?;
        let lo = (b[i + 1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

/// 版本迁移：把任意版本的配置提升到 [`CONFIG_VERSION`]。
///
/// - `0`（无版本字段的旧配置）→ 目前结构兼容，只补版本号；
///   将来若字段改名 / 语义变化，在这里加 `if cfg.config_version < N { … }` 分支。
/// - 版本**高于**本程序支持时只告警不报错（前向兼容，未知字段被忽略）。
fn migrate(mut cfg: Config) -> Config {
    if cfg.config_version == 0 {
        cfg.config_version = CONFIG_VERSION;
    }
    if cfg.config_version > CONFIG_VERSION {
        crate::diag::log(
            "WARN",
            &format!(
                "配置版本 {} 高于程序支持的 {}，未知字段会被忽略",
                cfg.config_version, CONFIG_VERSION
            ),
        );
    }
    cfg
}

/// 解析配置文本（含版本迁移）。独立成函数以便单测，**不触碰真实配置文件**。
pub fn parse(text: &str) -> Result<Config, String> {
    let cfg: Config = serde_json::from_str(text).map_err(|e| e.to_string())?;
    Ok(migrate(cfg))
}

/// 读取配置。
///
/// - 文件不存在 = 首次运行 → 默认值（正常路径，不报错）；
/// - 解析失败 → **备份损坏文件**（`.stool.bak`，不覆盖既有备份）→ 落日志 + stderr 告警 →
///   返回默认值。相比旧的"静默回退默认值"，损坏不再无声无息（旧行为会让下一次
///   `save` 把损坏内容彻底覆盖掉）。
/// - 密钥字段：带 [`KEY_PREFIX`] 的用 DPAPI 解回明文；解不开（换机器/账户）→ 告警 + 清空该字段。
pub fn load() -> Config {
    let p = config_path();
    let text = match fs::read_to_string(&p) {
        Ok(t) => t,
        Err(_) => return Config::default(),
    };
    match parse(&text) {
        Ok(mut cfg) => {
            match decrypt_key(&cfg.mtl_key) {
                Ok(k) => cfg.mtl_key = k,
                Err(e) => {
                    let msg = format!(
                        "机翻 API Key 解密失败（{e}）。DPAPI 密文绑定当前 Windows 用户，\
                         把配置拷到别的机器/账户会解不开；该字段已清空，请重新填写（其它设置不受影响）。"
                    );
                    crate::diag::log("WARN", &msg);
                    eprintln!("⚠ {msg}");
                    cfg.mtl_key = String::new();
                }
            }
            cfg
        }
        Err(e) => {
            let where_ = match backup_once(&p) {
                Ok(b) => b.display().to_string(),
                Err(be) => format!("（备份失败：{be}）"),
            };
            let msg = format!(
                "配置文件解析失败：{e}。已备份原文件到 {where_}，本次使用默认配置（请检查后重新保存设置）"
            );
            crate::diag::log("WARN", &msg);
            eprintln!("⚠ {msg}");
            Config::default()
        }
    }
}

/// 保存配置。**密钥不明文落盘**：先加密再写；加密失败也不阻断保存（退回明文并落日志）。
pub fn save(cfg: &Config) -> Result<(), String> {
    let p = config_path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut out = cfg.clone();
    match encrypt_key(&cfg.mtl_key) {
        Ok(enc) => out.mtl_key = enc,
        Err(e) => {
            crate::diag::log("WARN", &format!("API Key 加密失败（{e}），本次以明文写入配置"));
        }
    }
    fs::write(&p, serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// 外部工具安装根目录（与 `features::tools_dl` 一致：`~/.stool/tools/`）。
pub fn tools_root() -> PathBuf {
    userprofile().join(".stool").join("tools")
}

/// unrpyc.py 路径：配置优先 → 环境变量 → 本工具下载目录 → skill 捆绑副本 → 兜底。
///
/// 注意：**不能写死本机绝对路径**（换机器即失效，且失败会静默回退到系统 `python`）。
/// 这里只按「跨机器成立」的位置顺序探测。
pub fn unrpyc_path() -> PathBuf {
    let cfg = load();
    if !cfg.unrpyc.is_empty() {
        let p = PathBuf::from(&cfg.unrpyc);
        if p.exists() {
            return p;
        }
    }
    let candidates = [
        // ① 设置页「一键下载」的落地位置
        tools_root().join("unrpyc").join("unrpyc.py"),
        // ② skill 捆绑副本
        userprofile()
            .join(".workbuddy")
            .join("skills")
            .join("game-unpacker")
            .join("scripts")
            .join("unrpyc")
            .join("unrpyc.py"),
    ];
    if let Ok(v) = std::env::var("STOOL_UNRPYC") {
        let p = PathBuf::from(v);
        if p.exists() {
            return p;
        }
    }
    let found = candidates.iter().find(|p| p.exists()).cloned();
    found.unwrap_or_else(|| candidates[0].clone())
}

/// Python 解释器：配置优先 → 环境变量 → 受管运行时（扫 `versions/*`，取最新）→ PATH。
///
/// 受管目录里可能同时存在 3.12/3.13 等多个版本，硬编码某个版本号换机器就会失效；
/// 这里改成**扫描目录取版本号最大者**。
pub fn python_path() -> String {
    let cfg = load();
    if !cfg.python.is_empty() {
        return cfg.python.clone();
    }
    if let Ok(v) = std::env::var("STOOL_PYTHON") {
        return v;
    }
    if let Some(p) = managed_python() {
        return p.to_string_lossy().into_owned();
    }
    "python".to_string()
}

/// 在受管运行目录里找最新版的 `python.exe`（`~/.workbuddy/binaries/python/versions/<ver>/python.exe`）。
fn managed_python() -> Option<PathBuf> {
    let versions = userprofile()
        .join(".workbuddy")
        .join("binaries")
        .join("python")
        .join("versions");
    let rd = fs::read_dir(&versions).ok()?;
    let mut best: Option<(Vec<u32>, PathBuf)> = None;
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let exe = dir.join("python.exe");
        if !exe.exists() {
            continue;
        }
        let ver = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .split('.')
            .map(|x| x.parse::<u32>().unwrap_or(0))
            .collect::<Vec<u32>>();
        if best.as_ref().map(|(b, _)| ver > *b).unwrap_or(true) {
            best = Some((ver, exe));
        }
    }
    best.map(|(_, p)| p)
}

pub fn userprofile() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

pub fn external_tool(cfg: &Config, key: &str) -> Option<PathBuf> {
    let v = match key {
        "garbro" => &cfg.garbro,
        "asset_ripper" => &cfg.asset_ripper,
        "il2cpp_dumper" => &cfg.il2cpp_dumper,
        "wolfdec" => &cfg.wolfdec,
        "gdre_tools" => &cfg.gdre_tools,
        _ => return None,
    };
    if v.is_empty() {
        return None;
    }
    let p = PathBuf::from(v);
    p.exists().then_some(p)
}

/// 确保文件父目录存在。
pub fn ensure_parent(p: &Path) {
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
}

/// 原文件 → 备份路径：在**完整文件名**后追加 `.stool.bak`。
///
/// 等价于历史上的 `with_extension("xp3.stool.bak")` 写法，但对多级扩展名更安全
/// （`save.dat.gz` → `save.dat.gz.stool.bak`，不会把 `.gz` 吃掉），
/// 也与 `features::restore` 的反向推导（剥掉 `.stool.bak` 即原文件）严格一致。
pub fn backup_path_for(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".stool.bak");
    PathBuf::from(s)
}

/// 备份一次；**备份已存在则绝不覆盖**，返回备份路径。
///
/// 为什么必须"不覆盖"：回填 / 重封包 / 注入是可以在同一目录上**反复执行**的操作。
/// 若第二次无条件 `fs::copy(原文件, 备份)`，备份内容就会从"原始文件"变成"上一次改过的文件"，
/// 原始文件随之永久丢失（`restore` 也救不回来）。
/// 所以这里的语义是"**首次写操作前的原始状态存档**"——一旦存在，就认定原始状态已存档。
pub fn backup_once(path: &Path) -> Result<PathBuf, String> {
    let bak = backup_path_for(path);
    if bak.exists() {
        return Ok(bak);
    }
    fs::copy(path, &bak).map_err(|e| format!("备份 {} 失败: {e}", path.display()))?;
    Ok(bak)
}

/// 写操作前的强制备份：失败即返回 Err，调用方**必须中止**而不能继续覆盖原文件。
pub fn backup_or_abort(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("原文件不存在: {}", path.display()));
    }
    backup_once(path).map_err(|e| format!("{e}（为安全起见已中止，未修改任何文件）"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir()
            .join(format!("stool_settings_{}", std::process::id()))
            .join(name);
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn backup_path_keeps_multi_extension() {
        // 多级扩展名不应被 with_extension 吃掉尾巴
        assert_eq!(
            backup_path_for(Path::new("/a/save.dat.gz")),
            Path::new("/a/save.dat.gz.stool.bak")
        );
        assert_eq!(
            backup_path_for(Path::new("/a/Game.rgss3a")),
            Path::new("/a/Game.rgss3a.stool.bak")
        );
    }

    /// 核心数据安全回归：反复执行写操作时，备份必须始终保留**首次**的原始内容，
    /// 绝不能被第二次调用覆盖成"改过一次的中间态"。
    #[test]
    fn backup_once_never_overwrites() {
        let dir = tmp("never_overwrite");
        let f = dir.join("Game.rgss3a");
        fs::write(&f, b"ORIGINAL").unwrap();

        let bak = backup_once(&f).unwrap();
        assert_eq!(fs::read(&bak).unwrap(), b"ORIGINAL");

        // 第一次修改原文件
        fs::write(&f, b"MODIFIED-1").unwrap();
        // 第二次备份：不得覆盖，仍是 ORIGINAL
        let bak2 = backup_once(&f).unwrap();
        assert_eq!(bak2, bak);
        assert_eq!(fs::read(&bak).unwrap(), b"ORIGINAL", "第二次备份覆盖了原始存档！");

        // 第三次依旧不变
        fs::write(&f, b"MODIFIED-2").unwrap();
        let _ = backup_once(&f).unwrap();
        assert_eq!(fs::read(&bak).unwrap(), b"ORIGINAL");
    }

    #[test]
    fn backup_or_abort_missing_original() {
        let dir = tmp("missing");
        let missing = dir.join("nope.pck");
        assert!(backup_or_abort(&missing).is_err());
        // 不应凭空生成备份
        assert!(!backup_path_for(&missing).exists());
    }

    #[test]
    fn backup_or_abort_creates_first_backup() {
        let dir = tmp("first");
        let f = dir.join("data.xp3");
        fs::write(&f, b"X").unwrap();
        let bak = backup_or_abort(&f).unwrap();
        assert_eq!(fs::read(&bak).unwrap(), b"X");
    }

    #[test]
    fn config_legacy_without_version_migrates() {
        // 旧配置：无 config_version 字段 —— 迁移后应补上当前版本且不丢字段
        let legacy = r#"{"output_dir":"D:/out","proxy":"http://127.0.0.1:7892"}"#;
        let cfg = parse(legacy).unwrap();
        assert_eq!(cfg.config_version, CONFIG_VERSION);
        assert_eq!(cfg.output_dir, "D:/out");
        // 缺失字段走默认值（局部缺失不再导致"设置全丢"）
        assert_eq!(cfg.mtl_batch, default_mtl_batch());
    }

    #[test]
    fn config_empty_object_uses_defaults() {
        let cfg = parse("{}").unwrap();
        assert_eq!(cfg.config_version, CONFIG_VERSION);
        assert_eq!(cfg.proxy, default_proxy());
        assert!(cfg.garbro.is_empty());
    }

    #[test]
    fn config_roundtrip_keeps_version() {
        let text = serde_json::to_string(&Config::default()).unwrap();
        let back = parse(&text).unwrap();
        assert_eq!(back.config_version, CONFIG_VERSION);
        assert_eq!(back.proxy, default_proxy());
    }

    #[test]
    fn config_parse_error_surfaces() {
        // 损坏内容必须返回 Err（调用方据此备份 + 告警），而不是静默吞掉
        assert!(parse("{ not json").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn legacy_plaintext_key_passes_through() {
        // 旧配置里的明文 Key 没有前缀：必须原样返回（保证老配置能用）
        assert_eq!(decrypt_key("sk-plain-123").unwrap(), "sk-plain-123");
        assert_eq!(decrypt_key("").unwrap(), "");
    }

    #[test]
    fn hex_decode_roundtrip() {
        let bytes: Vec<u8> = (0..=255u8).collect();
        assert_eq!(hex_decode(&crate::hash::hex(&bytes)).unwrap(), bytes);
        assert!(hex_decode("abc").is_none(), "奇数长度必须拒绝");
        assert!(hex_decode("zz").is_none(), "非十六进制必须拒绝");
    }

    #[test]
    fn empty_key_stays_empty() {
        // 没填 Key 时不应写入无意义的密文
        assert_eq!(encrypt_key("").unwrap(), "");
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_and_no_plaintext_leak() {
        let secret = "sk-test-中文-key-0123456789";
        let enc = encrypt_key(secret).unwrap();
        assert!(enc.starts_with(KEY_PREFIX), "应带 enc:v1: 前缀");
        assert!(!enc.contains(secret), "密文里不能出现明文");
        assert_eq!(decrypt_key(&enc).unwrap(), secret, "加解密必须回环");
        // 篡改密文（保持十六进制合法）→ 必须报错（而不是解出垃圾）
        let idx = KEY_PREFIX.len() + 4;
        let mut bad = enc.clone();
        let ch = if bad.as_bytes()[idx] == b'0' { "1" } else { "0" };
        bad.replace_range(idx..idx + 1, ch);
        assert!(decrypt_key(&bad).is_err(), "被篡改的密文应解密失败");
    }
}
