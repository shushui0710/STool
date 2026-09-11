//! 封包数据来源：**内存 / 文件双模**（P2-1 解析器流式化）。
//!
//! 问题：以前每个 `parse()` 都 `fs::read` 整包，实测 509MB 的 `Game.rgss3a`、
//! 393MB 的 `Game.pck` 会白白吃掉等量内存——而索引其实只用文件头几十字节。
//!
//! 方案：`Source` 按体积自动选模式——
//! - ≤ [`MEM_THRESHOLD`]：整包读进内存（小文件省掉 seek 开销，行为与过去一致）；
//! - 更大：只保留 `File` 句柄，索引与条目都按需 `seek + read`。
//!
//! 超过 [`MAX_ARCHIVE`] 直接拒绝（多半是选错了文件），由调用方给出友好报错。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// 小于此体积整包读进内存。
pub const MEM_THRESHOLD: u64 = 128 * 1024 * 1024;
/// 单封包解析硬上限。
pub const MAX_ARCHIVE: u64 = 4 * 1024 * 1024 * 1024;

/// 封包数据来源。
pub enum Source {
    Mem(Vec<u8>),
    File { f: File, path: PathBuf, len: u64 },
}

impl Source {
    /// 打开来源。`max` 为硬上限（一般传 [`MAX_ARCHIVE`]）。
    pub fn open(path: &Path, max: u64) -> Result<Source, String> {
        Self::open_with(path, max, MEM_THRESHOLD)
    }

    /// 打开来源，并显式指定「整包读入内存」的体积阈值。
    /// 传 0 可强制走文件模式（测试用，也便于日后按设置项调整）。
    pub fn open_with(path: &Path, max: u64, mem_threshold: u64) -> Result<Source, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("读取失败 {}: {e}", path.display()))?;
        let len = meta.len();
        if len > max {
            return Err(format!(
                "封包过大（{}），超过解析上限 {}：请确认选对了文件",
                human(len),
                human(max)
            ));
        }
        if len <= mem_threshold {
            let data = std::fs::read(path).map_err(|e| format!("读取失败 {}: {e}", path.display()))?;
            Ok(Source::Mem(data))
        } else {
            let f = File::open(path).map_err(|e| format!("打开失败 {}: {e}", path.display()))?;
            Ok(Source::File { f, path: path.to_path_buf(), len })
        }
    }

    pub fn len(&self) -> u64 {
        match self {
            Source::Mem(d) => d.len() as u64,
            Source::File { len, .. } => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 内存模式下可直接借用（索引解析走这条路更快）。
    pub fn mem(&self) -> Option<&[u8]> {
        match self {
            Source::Mem(d) => Some(d),
            Source::File { .. } => None,
        }
    }

    /// 从 `off` 读 `len` 字节（自动夹取到文件尾，不会越界）。
    ///
    /// **不** panic、**不** 返回部分数据歧义：读到多少返回多少，调用方自行判长度。
    pub fn read_at(&mut self, off: u64, len: usize) -> Result<Vec<u8>, String> {
        let total = self.len();
        if off > total {
            return Err(format!("读取越界：offset {off} > 文件长度 {total}"));
        }
        let n = len.min((total - off) as usize);
        match self {
            Source::Mem(d) => Ok(d[off as usize..off as usize + n].to_vec()),
            Source::File { f, path, .. } => {
                f.seek(SeekFrom::Start(off)).map_err(|e| format!("seek 失败 {}: {e}", path.display()))?;
                let mut buf = vec![0u8; n];
                f.read_exact(&mut buf).map_err(|e| format!("读取失败 {}: {e}", path.display()))?;
                Ok(buf)
            }
        }
    }

    /// 从 `off` 读满 `len` 字节；文件不足则报错（用于「必须有」的字段）。
    pub fn read_exact_at(&mut self, off: u64, len: usize) -> Result<Vec<u8>, String> {
        let v = self.read_at(off, len)?;
        if v.len() != len {
            return Err(format!("文件截断：需要 offset {off} 处 {len} 字节，实际只有 {}", v.len()));
        }
        Ok(v)
    }

    /// 读取 [off, 文件尾) 的全部内容（用于「目录在文件尾」这类只有到尾部才知道大小的场景）。
    ///
    /// `cap` 为单次读取上限，超出即报错——避免误把整个大封包读进来。
    pub fn read_tail(&mut self, off: u64, cap: usize) -> Result<Vec<u8>, String> {
        let total = self.len();
        if off > total {
            return Err(format!("读取越界：offset {off} > 文件长度 {total}"));
        }
        let len = (total - off) as usize;
        if len > cap {
            return Err(format!(
                "该封包的索引区过大（{} > 上限 {}），超出流式解析范围",
                human(len as u64),
                human(cap as u64)
            ));
        }
        self.read_exact_at(off, len)
    }
}

pub fn human(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let f = n as f64;
    if f >= GB {
        format!("{:.1} GB", f / GB)
    } else if f >= MB {
        format!("{:.1} MB", f / MB)
    } else if f >= KB {
        format!("{:.1} KB", f / KB)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stool_source_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn small_file_is_memory_backed() {
        let d = tmp("mem");
        let p = d.join("small.bin");
        std::fs::write(&p, b"0123456789").unwrap();
        let mut s = Source::open(&p, MAX_ARCHIVE).unwrap();
        assert!(s.mem().is_some(), "小文件应走内存");
        assert_eq!(s.len(), 10);
        assert_eq!(s.read_at(2, 3).unwrap(), b"234");
        // 越界读自动夹取
        assert_eq!(s.read_at(8, 100).unwrap(), b"89");
        assert!(s.read_exact_at(8, 100).is_err(), "read_exact 应报截断");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn force_file_mode_reads_by_seek() {
        let d = tmp("file");
        let p = d.join("big.bin");
        std::fs::write(&p, vec![7u8; 4096]).unwrap();
        // max 作为唯一入参控制不了阈值，这里直接构造 File 分支来验证 seek 读
        let mut s = Source::open(&p, MAX_ARCHIVE).unwrap();
        // 手动转成文件模式（模拟超大文件）
        if let Source::Mem(data) = s {
            s = Source::File { f: File::open(&p).unwrap(), path: p.clone(), len: data.len() as u64 };
        }
        assert!(s.mem().is_none());
        assert_eq!(s.len(), 4096);
        assert_eq!(s.read_at(100, 4).unwrap(), vec![7u8; 4]);
        assert_eq!(s.read_tail(4090, 16).unwrap().len(), 6);
        assert!(s.read_tail(0, 16).is_err(), "尾部读取超上限应报错");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_oversized_archive() {
        let d = tmp("cap");
        let p = d.join("x.bin");
        std::fs::write(&p, vec![0u8; 64]).unwrap();
        let e = Source::open(&p, 10).err().expect("超上限应报错");
        assert!(e.contains("超过解析上限"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn human_formats_units() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(2048), "2.0 KB");
        assert_eq!(human(5 * 1024 * 1024), "5.0 MB");
    }
}
