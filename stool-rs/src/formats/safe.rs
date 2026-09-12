//! 边界安全的二进制读取助手。
//!
//! 目的：解析器面对损坏 / 截断 / 伪造的封包时绝不 panic。
//! 统一用这些函数替代 `data[a..b].try_into().unwrap()`——后者在
//! 索引越界时直接 panic（可被用户随意构造的文件触发，属于 DoS 面）。
//!
//! 约定：所有函数在越界时返回 `None`，调用方用 `?` 或 `match` 转成 `Err`。

/// 读取 `data[off..off+len]`，越界返回 `None`。
#[inline]
pub fn slice(data: &[u8], off: usize, len: usize) -> Option<&[u8]> {
    let end = off.checked_add(len)?;
    data.get(off..end)
}

/// 从 `off` 起取 `len` 字节并拷贝为定长数组，越界返回 `None`。
#[inline]
pub fn arr<const N: usize>(data: &[u8], off: usize) -> Option<[u8; N]> {
    slice(data, off, N)?.try_into().ok()
}

#[inline]
pub fn u8_at(data: &[u8], off: usize) -> Option<u8> {
    data.get(off).copied()
}

#[inline]
pub fn u16_le(data: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(arr::<2>(data, off)?))
}

#[inline]
pub fn u32_le(data: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(arr::<4>(data, off)?))
}

#[inline]
pub fn u64_le(data: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(arr::<8>(data, off)?))
}

#[inline]
pub fn i32_le(data: &[u8], off: usize) -> Option<i32> {
    Some(i32::from_le_bytes(arr::<4>(data, off)?))
}

#[inline]
pub fn i64_le(data: &[u8], off: usize) -> Option<i64> {
    Some(i64::from_le_bytes(arr::<8>(data, off)?))
}

#[inline]
pub fn u32_be(data: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_be_bytes(arr::<4>(data, off)?))
}

#[inline]
pub fn u16_be(data: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_be_bytes(arr::<2>(data, off)?))
}

// ── 带游标的便捷读取（用于顺序解析：读完自动前进） ──

/// 一个只前进、永不 panic 的顺序读取游标。
pub struct Cursor<'a> {
    data: &'a [u8],
    pub pos: usize,
}

impl<'a> Cursor<'a> {
    #[inline]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[inline]
    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    #[inline]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// 取 `len` 字节并前进；越界返回 `None` 且不改变 `pos`。
    #[inline]
    pub fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let s = slice(self.data, self.pos, len)?;
        self.pos += len;
        Some(s)
    }

    #[inline]
    pub fn u8(&mut self) -> Option<u8> {
        let v = u8_at(self.data, self.pos)?;
        self.pos += 1;
        Some(v)
    }

    #[inline]
    pub fn u16_le(&mut self) -> Option<u16> {
        let v = u16_le(self.data, self.pos)?;
        self.pos += 2;
        Some(v)
    }

    #[inline]
    pub fn u32_le(&mut self) -> Option<u32> {
        let v = u32_le(self.data, self.pos)?;
        self.pos += 4;
        Some(v)
    }

    #[inline]
    pub fn i32_le(&mut self) -> Option<i32> {
        let v = i32_le(self.data, self.pos)?;
        self.pos += 4;
        Some(v)
    }

    #[inline]
    pub fn u64_le(&mut self) -> Option<u64> {
        let v = u64_le(self.data, self.pos)?;
        self.pos += 8;
        Some(v)
    }

    /// 跳过 `len` 字节；越界返回 `false`（pos 不变）。
    #[inline]
    pub fn skip(&mut self, len: usize) -> bool {
        match self.pos.checked_add(len).filter(|&e| e <= self.data.len()) {
            Some(e) => {
                self.pos = e;
                true
            }
            None => false,
        }
    }
}
