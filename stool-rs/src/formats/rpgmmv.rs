//! RPG Maker MV/MZ 加密素材（假头 XOR 解密/加密）与存档编解码。

use std::fs;
use std::path::Path;

pub const MV_FAKE: [u8; 16] = [0x52, 0x50, 0x47, 0x4D, 0x56, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
pub const MZ_FAKE: [u8; 16] = [0x52, 0x50, 0x47, 0x4D, 0x5A, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

pub fn is_encrypted(data: &[u8]) -> bool {
    data.len() >= 32 && data.starts_with(b"RPGM")
}

/// MV/MZ 素材解密：16 字节假头 + 16 字节被 XOR 的真实文件头。
pub fn decrypt(data: &[u8]) -> Vec<u8> {
    if !is_encrypted(data) {
        return data.to_vec();
    }
    let fake = &data[..16];
    let mut out = vec![0u8; data.len() - 16];
    for i in 0..16 {
        out[i] = data[16 + i] ^ fake[i];
    }
    out[16..].copy_from_slice(&data[32..]);
    out
}

/// 加密（mz=true 使用 RPGMZ 假头）。
pub fn encrypt(data: &[u8], mz: bool) -> Vec<u8> {
    let fake = if mz { &MZ_FAKE } else { &MV_FAKE };
    let mut out = Vec::with_capacity(data.len() + 16);
    out.extend_from_slice(fake);
    for i in 0..16.min(data.len()) {
        out.push(data[i] ^ fake[i]);
    }
    if data.len() > 16 {
        out.extend_from_slice(&data[16..]);
    }
    out
}

/// 解密目录中的加密素材到 out_dir（返回处理数量）。
pub fn decrypt_dir(root: &Path, out_dir: &Path) -> Result<usize, String> {
    let map: &[(&str, &str)] = &[
        ("rpgmvp", "png"), ("rpgmvo", "ogg"), ("rpgmvm", "m4a"),
        ("png_", "png"), ("ogg_", "ogg"), ("m4a_", "m4a"),
    ];
    let mut n = 0;
    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        for (enc_ext, real_ext) in map {
            if ext == **enc_ext {
                let rel = p.strip_prefix(root).map_err(|e| e.to_string())?;
                let dst = out_dir.join(rel).with_extension(real_ext);
                fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
                let data = fs::read(p).map_err(|e| e.to_string())?;
                fs::write(&dst, decrypt(&data)).map_err(|e| e.to_string())?;
                n += 1;
                break;
            }
        }
    }
    Ok(n)
}

/// 加密 src_dir 下的明文素材回写游戏目录（覆盖前自动 .stool.bak 备份）。
pub fn encrypt_into(root: &Path, src_dir: &Path, mz: bool) -> Result<usize, String> {
    let map: &[(&str, &str)] = if mz {
        &[("png", "png_"), ("ogg", "ogg_"), ("m4a", "m4a_")]
    } else {
        &[("png", "rpgmvp"), ("ogg", "rpgmvo"), ("m4a", "rpgmvm")]
    };
    let mut n = 0;
    for entry in walkdir::WalkDir::new(src_dir).into_iter().filter_map(|e| e.ok()) {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        for (real_ext, enc_ext) in map {
            if ext == **real_ext {
                let rel = p.strip_prefix(src_dir).map_err(|e| e.to_string())?;
                let dst = root.join(rel).with_extension(enc_ext);
                if dst.exists() {
                    let bak = dst.with_extension(format!("{enc_ext}.stool.bak"));
                    fs::copy(&dst, &bak).map_err(|e| e.to_string())?;
                }
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let data = fs::read(p).map_err(|e| e.to_string())?;
                fs::write(&dst, encrypt(&data, mz)).map_err(|e| e.to_string())?;
                n += 1;
                break;
            }
        }
    }
    Ok(n)
}

/// MV 存档（lz-string base64）→ JSON 字符串。
pub fn mv_save_decode(data: &[u8]) -> Result<String, String> {
    let text = String::from_utf8_lossy(data);
    let cleaned = text.trim().trim_start_matches("lzd").trim();
    super::lzstring::decompress_from_base64(cleaned)
        .ok_or_else(|| "lz-string 解压失败".to_string())
}

/// MZ 存档（zlib）→ JSON 字符串。
pub fn mz_save_decode(data: &[u8]) -> Result<String, String> {
    if data.first() == Some(&0x78) {
        let mut z = flate2::read::ZlibDecoder::new(data);
        let mut out = String::new();
        std::io::Read::read_to_string(&mut z, &mut out).map_err(|e| e.to_string())?;
        Ok(out)
    } else {
        String::from_utf8(data.to_vec()).map_err(|_| "不是 MZ zlib 存档".to_string())
    }
}
