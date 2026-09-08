//! 本地化 CSV 读写：列 = id, file, context, source, translation。

use std::path::Path;

pub const HEADERS: [&str; 5] = ["id", "file", "context", "source", "translation"];

/// 从内部 id 推导 CSV 的 file 列：
/// - MV/RGSS 型 id（`Map001.json|12|3|45|s`）→ 取第一个 `|` 前的文件名；
/// - Ren'Py/NScripter 型 id（`script.rpy:42`）→ 取最后一个 `:` 前的文件名；
/// - 其它 → 原样。
fn file_col_from_id(id: &str) -> &str {
    if let Some(f) = id.split('|').next() {
        if f.len() < id.len() {
            return f;
        }
    }
    if let Some((f, _)) = id.rsplit_once(':') {
        if !f.is_empty() {
            return f;
        }
    }
    id
}

pub fn write_csv(out: &Path, rows: &[[String; 4]]) -> Result<(), String> {
    // rows 内部列：id, context, source, translation
    crate::settings::ensure_parent(out);
    let mut w = csv::Writer::from_path(out).map_err(|e| e.to_string())?;
    w.write_record(HEADERS).map_err(|e| e.to_string())?;
    for r in rows {
        // file 列必须是真实文件名（text_import 用它直接打开文件），而不是 id 的副本
        w.write_record(&[&r[0], file_col_from_id(&r[0]), &r[1], &r[2], &r[3]])
            .map_err(|e| e.to_string())?;
    }
    w.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// 写入完整 5 列行（id, file, context, source, translation），原样保留 file 列。
pub fn write_csv_rows(out: &Path, rows: &[[String; 5]]) -> Result<(), String> {
    crate::settings::ensure_parent(out);
    let mut w = csv::Writer::from_path(out).map_err(|e| e.to_string())?;
    w.write_record(HEADERS).map_err(|e| e.to_string())?;
    for r in rows {
        w.write_record(r.as_slice()).map_err(|e| e.to_string())?;
    }
    w.flush().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归测试：file 列必须是真实文件名（text_import 用它直接打开文件），
    /// 之前误写成 id 的副本导致 MV/RGSS 导入全部跳过。
    #[test]
    fn test_file_col_derived_from_id() {
        let dir = std::env::temp_dir().join(format!("stool_text_fc_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("out.csv");
        write_csv(
            &p,
            &[
                ["Map001.json|12|3|45|s".into(), "对白".into(), "こんにちは".into(), String::new()],
                ["Items.json|5|name".into(), "词条".into(), "薬草".into(), String::new()],
                ["script.rpy:42".into(), "dialogue".into(), "hi".into(), String::new()],
                ["nscript.dat:12".into(), "nscript".into(), "text".into(), String::new()],
            ],
        )
        .unwrap();
        let rows = read_csv(&p).unwrap();
        assert_eq!(rows[0][1], "Map001.json");
        assert_eq!(rows[1][1], "Items.json");
        assert_eq!(rows[2][1], "script.rpy");
        assert_eq!(rows[3][1], "nscript.dat");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

pub fn read_csv(path: &Path) -> Result<Vec<[String; 5]>, String> {
    let mut r = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for rec in r.records() {
        let rec = rec.map_err(|e| e.to_string())?;
        let mut row: [String; 5] = Default::default();
        for (i, cell) in rec.iter().enumerate().take(5) {
            row[i] = cell.to_string();
        }
        out.push(row);
    }
    Ok(out)
}
