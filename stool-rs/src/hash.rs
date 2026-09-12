//! 纯 Rust SHA-256（零第三方依赖）。
//!
//! 用途：校验外部工具下载内容的完整性 —— 比对本工具算出的摘要与 GitHub Release
//! 资产 API 提供的 `sha256:` 摘要。项目里已有手写的 Adler-32 / FNV-1a / djb2，
//! 这里沿用同样风格，避免为一个哈希引入整棵依赖树。
//!
//! 支持流式：大文件边下边喂，不必先把整个文件读进内存。

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// 流式 SHA-256 计算器。
pub struct Sha256 {
    h: [u32; 8],
    buf: Vec<u8>,
    len: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self { h: H0, buf: Vec::with_capacity(64), len: 0 }
    }

    /// 喂入一段数据（可多次调用）。
    pub fn update(&mut self, mut data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u64);
        // 先补齐上一次残留的不足 64 字节
        if !self.buf.is_empty() {
            let need = 64 - self.buf.len();
            let take = need.min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == 64 {
                let block = std::mem::take(&mut self.buf);
                self.compress(&block);
            }
        }
        // 整块直接压缩
        let (full, rest) = data.as_chunks::<64>();
        for c in full {
            self.compress(c);
        }
        self.buf.extend_from_slice(rest);
    }

    /// 收尾并返回 32 字节摘要。
    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.len.wrapping_mul(8);
        self.buf.push(0x80);
        while self.buf.len() % 64 != 56 {
            self.buf.push(0);
        }
        self.buf.extend_from_slice(&bit_len.to_be_bytes());
        let blocks = std::mem::take(&mut self.buf);
        let (full, rest) = blocks.as_chunks::<64>();
        debug_assert!(rest.is_empty(), "补齐后长度必然是 64 的整数倍");
        for c in full {
            self.compress(c);
        }
        let mut out = [0u8; 32];
        for (i, v) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, chunk: &[u8]) {
        debug_assert_eq!(chunk.len(), 64);
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
        self.h[5] = self.h[5].wrapping_add(f);
        self.h[6] = self.h[6].wrapping_add(g);
        self.h[7] = self.h[7].wrapping_add(h);
    }
}

/// 一次性计算 SHA-256。
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize()
}

/// 字节 → 小写十六进制。
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ---------------------------------------------------------------------------
// SHA-1
// ---------------------------------------------------------------------------

const SHA1_H0: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];

/// 流式 SHA-1 计算器。
///
/// 用途：Artemis 引擎 `.pfs` 封包（`pf8`）用 `SHA1(index)` 当 XOR 密钥流。
/// 该算法是引擎自带的**固定**混淆（密钥完全由封包索引推导，无每游戏私钥），
/// 与 RGSS/MV 里已实现的固定密钥 XOR 同性质，不涉及破解他人加密方案。
pub struct Sha1 {
    h: [u32; 5],
    buf: Vec<u8>,
    len: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    pub fn new() -> Self {
        Self { h: SHA1_H0, buf: Vec::with_capacity(64), len: 0 }
    }

    /// 喂入一段数据（可多次调用）。
    pub fn update(&mut self, mut data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u64);
        if !self.buf.is_empty() {
            let need = 64 - self.buf.len();
            let take = need.min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == 64 {
                let block = std::mem::take(&mut self.buf);
                self.compress(&block);
            }
        }
        let (full, rest) = data.as_chunks::<64>();
        for c in full {
            self.compress(c);
        }
        self.buf.extend_from_slice(rest);
    }

    /// 收尾并返回 20 字节摘要。
    pub fn finalize(mut self) -> [u8; 20] {
        let bit_len = self.len.wrapping_mul(8);
        self.buf.push(0x80);
        while self.buf.len() % 64 != 56 {
            self.buf.push(0);
        }
        self.buf.extend_from_slice(&bit_len.to_be_bytes());
        let blocks = std::mem::take(&mut self.buf);
        let (full, rest) = blocks.as_chunks::<64>();
        debug_assert!(rest.is_empty(), "补齐后长度必然是 64 的整数倍");
        for c in full {
            self.compress(c);
        }
        let mut out = [0u8; 20];
        for (i, v) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, chunk: &[u8]) {
        debug_assert_eq!(chunk.len(), 64);
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
    }
}

/// 一次性计算 SHA-1。
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(data);
    h.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // 官方向量：空串 / "abc" / "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn streaming_equals_oneshot() {
        // 跨 64 字节边界的多种切分方式，结果必须与一次性计算一致
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let want = sha256(&data);
        for chunk in [1usize, 7, 63, 64, 65, 128, 999] {
            let mut h = Sha256::new();
            for part in data.chunks(chunk) {
                h.update(part);
            }
            assert_eq!(h.finalize(), want, "chunk={chunk} 与一次性结果不一致");
        }
    }

    #[test]
    fn million_a_vector() {
        // 官方 "一百万个 a" 向量，验证长输入的多块压缩路径
        let mut h = Sha256::new();
        let block = vec![b'a'; 10_000];
        for _ in 0..100 {
            h.update(&block);
        }
        assert_eq!(
            hex(&h.finalize()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn sha1_known_vectors() {
        // 官方向量：空串 / "abc" / 56 字节长句
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            hex(&sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn sha1_streaming_equals_oneshot_and_million_a() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let want = sha1(&data);
        for chunk in [1usize, 7, 63, 64, 65, 128, 999] {
            let mut h = Sha1::new();
            for part in data.chunks(chunk) {
                h.update(part);
            }
            assert_eq!(h.finalize(), want, "chunk={chunk} 与一次性结果不一致");
        }
        let mut h = Sha1::new();
        let block = vec![b'a'; 10_000];
        for _ in 0..100 {
            h.update(&block);
        }
        assert_eq!(hex(&h.finalize()), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    }
}
