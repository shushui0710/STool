//! NScripter / ONScripter 加密脚本（nscript.dat）解码。
//!
//! 常见加密：前两字节为大端密钥 key，第 i 字节 = 原字节 XOR ((key + i) & 0xFF)。
//! 自动尝试多个变体，选择可打印率最高者。

pub fn printable_ratio(data: &[u8]) -> f32 {
    if data.is_empty() {
        return 0.0;
    }
    let n = data.len().min(8192);
    let ok = data[..n]
        .iter()
        .filter(|&&c| (0x20..0x7f).contains(&c) || c == b'\t' || c == b'\n' || c == b'\r' || c >= 0x80)
        .count();
    ok as f32 / n as f32
}

pub fn decode(data: &[u8]) -> Vec<u8> {
    if data.len() < 3 {
        return data.to_vec();
    }
    // 前 2 字节为大端密钥；正文第 i 字节 = 密文 XOR ((key + i) & 0xFF)
    let key = ((data[0] as u32) << 8) | data[1] as u32;
    let body = &data[2..];
    let candidates: Vec<Vec<u8>> = vec![
        // 标准变体：正文索引从 0 起
        body.iter()
            .enumerate()
            .map(|(i, c)| c ^ ((key.wrapping_add(i as u32)) & 0xFF) as u8)
            .collect(),
        // 兼容变体：固定低 8 位密钥
        body.iter().map(|c| c ^ (key & 0xFF) as u8).collect(),
        body.to_vec(),
    ];
    candidates
        .into_iter()
        .max_by(|a, b| printable_ratio(a).partial_cmp(&printable_ratio(b)).unwrap())
        .unwrap()
}
