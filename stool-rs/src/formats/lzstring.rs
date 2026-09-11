//! lz-string（compressToBase64）解压的 Rust 移植，用于 RPG Maker MV 存档。
//!
//! 严格对照官方 pieroxy/lz-string `src/_decompress.ts` 移植：
//! - base64 通道 resetValue = 32（每字符 6 个有效位，掩码 32→1）
//! - position 递减到 0 时重载下一个字符
//! - 前导 2 位：0 → 8 位字面量，1 → 16 位字面量，2 → 结束
//! - 字典槽 0~2 为占位（"0"/"1"/"2"），槽 3 = 首字符，dictSize 从 4 起
//!
//! 使用 UTF-16 码元（Vec<u16>）保持与 JS String 的语义一致（含增补平面代理对）。

fn key_index(c: u8) -> i32 {
    match c {
        b'A'..=b'Z' => (c - b'A') as i32,
        b'a'..=b'z' => (c - b'a') as i32 + 26,
        b'0'..=b'9' => (c - b'0') as i32 + 52,
        b'+' | b'-' => 62,
        b'/' | b'_' => 63,
        _ => -1,
    }
}

const RESET_VALUE: i32 = 32; // decompressFromBase64 的 resetValue

struct BitReader<'a> {
    input: &'a [u8],
    val: i32,
    position: i32,
    index: usize,
}

impl<'a> BitReader<'a> {
    fn new(input: &'a [u8]) -> Self {
        let first = input.first().map_or(0, |&b| key_index(b).max(0));
        BitReader { input, val: first, position: RESET_VALUE, index: 1 }
    }

    /// JS: getNextValue(index++)，未知字符/越界视为 0（JS 中 undefined & mask == 0）
    fn next_val(&mut self) -> i32 {
        let v = self
            .input
            .get(self.index)
            .map_or(0, |&b| key_index(b).max(0));
        self.index += 1;
        v
    }

    /// 官方 readBits：从高位掩码 32 起逐位读取
    fn read_bits(&mut self, num_bits: u32) -> u32 {
        let maxpower = 1u32 << num_bits;
        let mut bits = 0u32;
        let mut power = 1u32;
        while power != maxpower {
            let resb = self.val & self.position;
            self.position >>= 1;
            if self.position == 0 {
                self.position = RESET_VALUE;
                self.val = self.next_val();
            }
            if resb > 0 {
                bits |= power;
            }
            power <<= 1;
        }
        bits
    }
}

/// 解压 lz-string base64 文本。失败返回 None。
pub fn decompress_from_base64(input: &str) -> Option<String> {
    let input: Vec<u8> = input
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();
    if input.is_empty() {
        return Some(String::new());
    }
    let length = input.len();
    let mut r = BitReader::new(&input);

    // 字典槽 0~2 占位（JS 中为字符串 "0"/"1"/"2"，永远 truthy），槽 3 为首字符
    let mut dictionary: Vec<Vec<u16>> = vec![vec![b'0' as u16], vec![b'1' as u16], vec![b'2' as u16]];
    let mut num_bits = 3u32;
    let mut enlarge_in = 4usize;

    let mut result: Vec<u16> = Vec::new();
    let mut w: Vec<u16>;

    // ---- 前导：2 位选择首字符宽度 ----
    let first: Vec<u16> = match r.read_bits(2) {
        0 => vec![r.read_bits(8) as u16],
        1 => vec![r.read_bits(16) as u16],
        2 => return Some(String::new()),
        _ => return None,
    };
    dictionary.push(first.clone()); // dictionary[3]
    let mut dict_size = 4usize;
    result.extend_from_slice(&first);
    w = first;

    // ---- 主循环 ----
    loop {
        if r.index > length {
            return Some(String::new());
        }
        let c = r.read_bits(num_bits);
        
        match c {
            0 => {
                let ch = r.read_bits(8) as u16;
                dictionary.push(vec![ch]);
                dict_size += 1;
                enlarge_in -= 1;
            }
            1 => {
                let ch = r.read_bits(16) as u16;
                dictionary.push(vec![ch]);
                dict_size += 1;
                enlarge_in -= 1;
            }
            2 => {
                return Some(String::from_utf16_lossy(&result));
            }
            _ => {}
        }
        // JS: c 此时或是字面量（走 0/1 分支后变为 dictSize-1）或字典索引
        let c_idx = if c == 0 || c == 1 { dict_size - 1 } else { c as usize };

        if enlarge_in == 0 {
            enlarge_in = 1usize << num_bits;
            num_bits += 1;
        }
        let entry: Vec<u16> = if c_idx < dictionary.len() && !dictionary[c_idx].is_empty() {
            dictionary[c_idx].clone()
        } else if c_idx == dict_size {
            // 特殊情形：引用刚要创建的词条 w + w[0]
            [w.as_slice(), &[w[0]]].concat()
        } else {
            return None;
        };
        result.extend_from_slice(&entry);
        // 新词条：w + entry[0]
        dictionary.push([w.as_slice(), &[entry[0]]].concat());
        dict_size += 1;
        enlarge_in -= 1;
        w = entry;
        if enlarge_in == 0 {
            enlarge_in = 1usize << num_bits;
            num_bits += 1;
        }
    }
}

/// compressToBase64 压缩（用于 MV 存档回写）。
///
/// 严格对照官方 pieroxy/lz-string `src/_compress.ts` 移植（bitsPerChar = 6 的 base64 通道），
/// 输出与 JS 版逐字节一致（已与 Python lzstring 包交叉验证）。
#[allow(unused_assignments)] // 末尾的 enlarge_in 赋值与官方算法一致（值不再被读取）
pub fn compress_to_base64(input: &str) -> Option<String> {
    // JS 的 charAt 按 UTF-16 码元遍历
    let units: Vec<u16> = input.encode_utf16().collect();
    if units.is_empty() {
        return Some(String::new());
    }

    const BITS_PER_CHAR: u32 = 6;
    let mut dictionary: std::collections::HashMap<Vec<u16>, u32> = Default::default();
    let mut to_create: std::collections::HashSet<Vec<u16>> = Default::default();
    let mut enlarge_in: u32 = 2; // 补偿第一个不计入的条目
    let mut dict_size: u32 = 3;
    let mut num_bits: u32 = 2;
    let mut out: Vec<u8> = Vec::new();
    let mut val: u32 = 0;
    let mut position: u32 = 0;

    // 在当前输出字符里再放一位（低位先出）；凑满 6 位就输出一个 base64 字符
    macro_rules! write_bit {
        ($bit:expr) => {{
            val = (val << 1) | (($bit) & 1);
            if position == BITS_PER_CHAR - 1 {
                out.push(KEY_BASE[(val & 0x3f) as usize]);
                val = 0;
                position = 0;
            } else {
                position += 1;
            }
        }};
    }

    let mut w: Vec<u16> = Vec::new();
    for &c in &units {
        if !dictionary.contains_key(&[c][..]) {
            dictionary.insert(vec![c], dict_size);
            dict_size += 1;
            to_create.insert(vec![c]);
        }
        let mut wc = w.clone();
        wc.push(c);
        if dictionary.contains_key(&wc) {
            w = wc;
        } else {
            if to_create.contains(&w) {
                if w[0] < 256 {
                    for _ in 0..num_bits {
                        write_bit!(0);
                    }
                    let mut value = w[0] as u32;
                    for _ in 0..8 {
                        write_bit!(value & 1);
                        value >>= 1;
                    }
                } else {
                    // 首位写 1（16 位字面量标记），其余补 0
                    write_bit!(1);
                    for _ in 1..num_bits {
                        write_bit!(0);
                    }
                    let mut value = w[0] as u32;
                    for _ in 0..16 {
                        write_bit!(value & 1);
                        value >>= 1;
                    }
                }
                enlarge_in -= 1;
                if enlarge_in == 0 {
                    enlarge_in = 1 << num_bits;
                    num_bits += 1;
                }
                to_create.remove(&w);
            } else {
                let mut value = dictionary[&w];
                for _ in 0..num_bits {
                    write_bit!(value & 1);
                    value >>= 1;
                }
            }
            enlarge_in -= 1;
            if enlarge_in == 0 {
                enlarge_in = 1 << num_bits;
                num_bits += 1;
            }
            // 把 wc 加入字典
            dictionary.insert(wc, dict_size);
            dict_size += 1;
            w = vec![c];
        }
    }
    // 输出最后一个 w
    if !w.is_empty() {
        if to_create.contains(&w) {
            if w[0] < 256 {
                for _ in 0..num_bits {
                    write_bit!(0);
                }
                let mut value = w[0] as u32;
                for _ in 0..8 {
                    write_bit!(value & 1);
                    value >>= 1;
                }
            } else {
                write_bit!(1);
                for _ in 1..num_bits {
                    write_bit!(0);
                }
                let mut value = w[0] as u32;
                for _ in 0..16 {
                    write_bit!(value & 1);
                    value >>= 1;
                }
            }
            enlarge_in -= 1;
            if enlarge_in == 0 {
                enlarge_in = 1 << num_bits;
                num_bits += 1;
            }
            to_create.remove(&w);
        } else {
            let mut value = dictionary[&w];
            for _ in 0..num_bits {
                write_bit!(value & 1);
                value >>= 1;
            }
        }
        enlarge_in -= 1;
        if enlarge_in == 0 {
            enlarge_in = 1 << num_bits;
            num_bits += 1;
        }
    }
    // 结束标记（值 2，低位先出）
    let mut value = 2u32;
    for _ in 0..num_bits {
        write_bit!(value & 1);
        value >>= 1;
    }
    // 补齐最后一个字符
    loop {
        val <<= 1;
        if position == BITS_PER_CHAR - 1 {
            out.push(KEY_BASE[(val & 0x3f) as usize]);
            break;
        }
        position += 1;
    }
    // base64 标准 4 字符对齐补 '='
    let pad = match out.len() % 4 {
        1 => "===",
        2 => "==",
        3 => "=",
        _ => "",
    };
    let mut s: String = out.iter().map(|&b| b as char).collect();
    s.push_str(pad);
    Some(s)
}

const KEY_BASE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[cfg(test)]
mod tests {
    use super::*;

    /// 压缩输出与 Python lzstring 包（官方算法移植）逐字节一致
    #[test]
    fn compress_matches_reference() {
        let cases = [
            ("hello world", "BYUwNmD2AEDukCcwBMg="),
            ("hello 世界", "BYUwNmD2AEhocoMq5A=="),
            (
                "{\"variables\":{\"1\":5,\"2\":99},\"switches\":[true,false,true],\"actors\":{}}",
                "N4IgbghgTglhBGAbApgZxALlARkwVgBoQAmTATjIF8jUB3GAFwGMALNTAbQagFdkCAZhESp+3PgF0iEJgwD2UdFkqUgA",
            ),
            (
                &"a".repeat(100)[..],
                "IY18ZXTt/0g=",
            ),
            (
                "ABCDEF 中文测试 mixed 123 カタカナ",
                "IIIQwgIgogYgBIWjlDhpoaVtCr0XAtgSwB4FMATOARgCYBmOQaoZB+hmsCmGIA==",
            ),
        ];
        for (input, expect) in cases {
            assert_eq!(compress_to_base64(input).as_deref(), Some(expect), "输入: {input}");
        }
    }

    #[test]
    fn compress_decompress_roundtrip() {
        let samples: Vec<String> = vec![
            "".to_string(),
            "hello world".to_string(),
            "hello 世界！中文カタカナ mixed 123 🎮".to_string(),
            r#"{"variables":{"1":5,"2":99},"switches":[true,false,true],"actors":{}}"#.to_string(),
            "a".repeat(1000),
            (0..200).map(|i| format!("条目{i}:value{i}\n")).collect(),
        ];
        for s in &samples {
            let c = compress_to_base64(s).unwrap();
            let d = decompress_from_base64(&c).unwrap();
            assert_eq!(&d, s, "回环失败（长度 {}）", s.chars().count());
        }
    }
}
