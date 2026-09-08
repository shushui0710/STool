//! 本地化 CSV 读写：列 = id, file, context, source, translation。

use std::path::Path;

pub const HEADERS: [&str; 5] = ["id", "file", "context", "source", "translation"];

pub fn write_csv(out: &Path, rows: &[[String; 4]]) -> Result<(), String> {
    // rows 内部列：id, context, source, translation
    crate::settings::ensure_parent(out);
    let mut w = csv::Writer::from_path(out).map_err(|e| e.to_string())?;
    w.write_record(HEADERS).map_err(|e| e.to_string())?;
    for r in rows {
        w.write_record(&[&r[0], &r[0], &r[1], &r[2], &r[3]])
            .map_err(|e| e.to_string())?;
    }
    w.flush().map_err(|e| e.to_string())?;
    Ok(())
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
