//! 翻译包：把一个游戏的汉化成果（文本 CSV + 注入 JSON + 清单）打包成单个 zip，
//! 便于备份 / 换机重装 / 分享给玩同一款游戏的人；导入时自动归位各文件并可直接注入。
//!
//! 包结构：
//! ```text
//! manifest.json   {"format":1,"engine":"rpgmaker_mv","game":"目录名","entries":N,"json_filled":M}
//! text.csv        文本提取 CSV（可选，存在才打包）
//! translation.json 注入 JSON（可选，存在才打包）
//! ```

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "manifest.json";
pub const CSV_NAME: &str = "text.csv";
pub const JSON_NAME: &str = "translation.json";
pub const FORMAT: u32 = 1;

/// 导出翻译包。csv / json 至少提供一个存在的文件；返回 (打包文件数, json 已填译文条数)。
pub fn export_pack(
    csv: Option<&Path>,
    json: Option<&Path>,
    out_zip: &Path,
    engine: &str,
    game: &str,
) -> Result<(usize, usize), String> {
    let mut files: Vec<(&str, Vec<u8>)> = Vec::new();
    let mut filled = 0usize;
    if let Some(jp) = json {
        if jp.exists() {
            let txt = std::fs::read_to_string(jp).map_err(|e| format!("读取 JSON 失败: {e}"))?;
            // 统计已填译文条数（值非空的键），写入清单便于导入端预览
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                if let Some(obj) = v.as_object() {
                    filled = obj.values().filter(|x| x.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)).count();
                }
            }
            files.push((JSON_NAME, txt.into_bytes()));
        }
    }
    if let Some(cp) = csv {
        if cp.exists() {
            let bytes = std::fs::read(cp).map_err(|e| format!("读取 CSV 失败: {e}"))?;
            files.push((CSV_NAME, bytes));
        }
    }
    if files.is_empty() {
        return Err("没有可打包的内容（CSV 与 注入 JSON 都不存在）".into());
    }
    let manifest = serde_json::json!({
        "format": FORMAT,
        "engine": engine,
        "game": game,
        "json_filled": filled,
    });
    let n = files.len();
    crate::settings::ensure_parent(out_zip);
    let fh = std::fs::File::create(out_zip).map_err(|e| format!("创建翻译包失败: {e}"))?;
    let mut zw = zip::ZipWriter::new(fh);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
    zw.start_file(MANIFEST, opts).map_err(|e| e.to_string())?;
    zw.write_all(serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?.as_bytes())
        .map_err(|e| e.to_string())?;
    for (name, bytes) in &files {
        zw.start_file(*name, opts).map_err(|e| e.to_string())?;
        zw.write_all(bytes).map_err(|e| e.to_string())?;
    }
    zw.finish().map_err(|e| e.to_string())?;
    Ok((n, filled))
}

/// 导入结果。
pub struct Imported {
    pub engine: String,
    pub game: String,
    /// 注入 JSON 归位到游戏目录后的路径（包内没有则为 None）
    pub json_path: Option<PathBuf>,
    /// CSV 归位到输出目录后的路径（包内没有则为 None）
    pub csv_path: Option<PathBuf>,
    pub json_filled: usize,
}

/// 导入翻译包：JSON 落到游戏目录（可注入），CSV 落到输出目录（可回填/再机翻）。
pub fn import_pack(zip_path: &Path, game_root: &Path, out_dir: &Path) -> Result<Imported, String> {
    let fh = std::fs::File::open(zip_path).map_err(|e| format!("打开翻译包失败: {e}"))?;
    let mut ar = zip::ZipArchive::new(fh).map_err(|e| format!("翻译包不是有效 zip: {e}"))?;
    // 读清单
    let manifest: serde_json::Value = {
        let mut mf = ar
            .by_name(MANIFEST)
            .map_err(|_| "翻译包缺少 manifest.json（不是 STool 翻译包）")?;
        let mut s = String::new();
        mf.read_to_string(&mut s).map_err(|e| e.to_string())?;
        serde_json::from_str(&s).map_err(|e| format!("manifest.json 损坏: {e}"))?
    };
    if manifest["format"].as_u64() != Some(FORMAT as u64) {
        return Err(format!("翻译包格式版本不支持: {:?}", manifest["format"]));
    }
    let engine = manifest["engine"].as_str().unwrap_or_default().to_string();
    let game = manifest["game"].as_str().unwrap_or_default().to_string();
    let filled = manifest["json_filled"].as_u64().unwrap_or(0) as usize;

    let mut imp = Imported { engine, game, json_path: None, csv_path: None, json_filled: filled };
    // 注入 JSON → 游戏目录
    if let Ok(mut f) = ar.by_name(JSON_NAME) {
        let mut s = String::new();
        f.read_to_string(&mut s).map_err(|e| e.to_string())?;
        // 校验是合法 JSON 对象
        let v: serde_json::Value = serde_json::from_str(&s).map_err(|e| format!("包内 JSON 损坏: {e}"))?;
        if !v.is_object() {
            return Err("包内 JSON 顶层不是对象".into());
        }
        let dst = game_root.join(JSON_NAME);
        crate::settings::ensure_parent(&dst);
        std::fs::write(&dst, s).map_err(|e| format!("写入 {} 失败: {e}", dst.display()))?;
        imp.json_path = Some(dst);
    }
    // CSV → 输出目录
    if let Ok(mut f) = ar.by_name(CSV_NAME) {
        let mut bytes = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        let dst = out_dir.join(CSV_NAME);
        crate::settings::ensure_parent(&dst);
        std::fs::write(&dst, bytes).map_err(|e| format!("写入 {} 失败: {e}", dst.display()))?;
        imp.csv_path = Some(dst);
    }
    if imp.json_path.is_none() && imp.csv_path.is_none() {
        return Err("翻译包里既没有 JSON 也没有 CSV".into());
    }
    Ok(imp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pack_roundtrip() {
        let dir = std::env::temp_dir().join(format!("stool_tpack_{}", std::process::id()));
        let game = dir.join("game");
        let out = dir.join("out");
        let _ = std::fs::create_dir_all(&game);
        let _ = std::fs::create_dir_all(&out);

        // 源文件
        let csv = dir.join("src.csv");
        let json = dir.join("src.json");
        crate::features::text::write_csv_rows(
            &csv,
            &[["1".into(), "f".into(), "ctx".into(), "こんにちは".into(), "你好".into()]],
        )
        .unwrap();
        std::fs::write(&json, "{\"こんにちは\":\"你好\",\"選択肢\":\"\"}").unwrap();

        // 导出
        let pack = dir.join("mypack.zip");
        let (n, filled) = export_pack(Some(&csv), Some(&json), &pack, "rpgmaker_mv", "mv_text_game").unwrap();
        assert_eq!(n, 2); // csv + json
        assert_eq!(filled, 1); // 選択肢 为空，只有 こんにちは 有译文

        // 导入到全新目录
        let game2 = dir.join("game2");
        let out2 = dir.join("out2");
        let _ = std::fs::create_dir_all(&game2);
        let _ = std::fs::create_dir_all(&out2);
        let imp = import_pack(&pack, &game2, &out2).unwrap();
        assert_eq!(imp.engine, "rpgmaker_mv");
        assert_eq!(imp.game, "mv_text_game");
        assert_eq!(imp.json_path.as_ref().unwrap(), &game2.join(JSON_NAME));
        assert_eq!(imp.csv_path.as_ref().unwrap(), &out2.join(CSV_NAME));
        assert_eq!(imp.json_filled, 1);
        // 内容一致且可直接注入（顶层是对象）
        let back = std::fs::read_to_string(game2.join(JSON_NAME)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&back).unwrap();
        assert_eq!(v["こんにちは"], "你好");
        assert_eq!(v["選択肢"], "");
        let rows = crate::features::text::read_csv(&out2.join(CSV_NAME)).unwrap();
        assert_eq!(rows[0][4], "你好");

        // 只有 CSV 也行
        let pack2 = dir.join("csvonly.zip");
        let (n2, _) = export_pack(Some(&csv), None, &pack2, "renpy", "g").unwrap();
        assert_eq!(n2, 1);
        let imp2 = import_pack(&pack2, &game2, &out2).unwrap();
        assert!(imp2.json_path.is_none() && imp2.csv_path.is_some());

        // 空 包报错
        assert!(export_pack(Some(&dir.join("nope.csv")), None, &dir.join("x.zip"), "e", "g").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
