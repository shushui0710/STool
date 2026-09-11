//! Ruby Marshal 最小解析器（子集），用于 RPG Maker 的 Data/Scripts.rxdata|rvdata|rvdata2。
//!
//! 支持：nil/true/false/fixnum/float/string/symbol/array/hash + 对象（取 ivars 值列表兜底）。

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Rb {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Vec<u8>),
    Sym(String),
    Array(Vec<Rb>),
    Hash(Vec<(Rb, Rb)>),
    Object(String, Vec<Rb>), // 类名 + ivar 值
    Other,
}

pub fn load(data: &[u8]) -> Result<Rb, String> {
    if data.len() < 4 || &data[..2] != b"\x04\x08" {
        return Err("不是 Ruby Marshal 数据".into());
    }
    let mut p = Marshal { data, pos: 2, symbols: Vec::new(), objects: Vec::new() };
    p.value()
}

struct Marshal<'a> {
    data: &'a [u8],
    pos: usize,
    symbols: Vec<String>,
    objects: Vec<Rb>,
}

impl<'a> Marshal<'a> {
    fn u8(&mut self) -> Result<u8, String> {
        let b = *self.data.get(self.pos).ok_or("marshal 越界")?;
        self.pos += 1;
        Ok(b)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err("marshal 越界".into());
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    /// Ruby Marshal long 解码（含 ±5 偏移规则）：
    /// - 0x00 → 0
    /// - 0x01..0x04 → 后跟 n 字节小端正数
    /// - 0x05..0x7A → 直接值 = 字节 - 5（0..117）
    /// - 0xFC..0xFF(-4..-1) → 后跟 n 字节小端负数
    /// - 0x85..0xFB(-123..-5) → 直接值 = 字节 + 5（-118..0）
    fn long(&mut self) -> Result<i64, String> {
        let c = self.u8()? as i8;
        if c == 0 {
            return Ok(0);
        }
        if c > 0 {
            if c <= 4 {
                let n = c as usize;
                let bytes = self.take(n)?;
                let mut v: i64 = 0;
                for (i, b) in bytes.iter().enumerate() {
                    v |= (*b as i64) << (8 * i);
                }
                return Ok(v);
            }
            // i8 上限即 127（0x7F），到此处必然是直接值区间 0x05..=0x7F
            return Ok(c as i64 - 5);
        }
        // c < 0
        let mag = -(c as i16);
        if mag <= 4 {
            let n = mag as usize;
            let bytes = self.take(n)?;
            let mut v: i64 = -1;
            for (i, b) in bytes.iter().enumerate() {
                v &= !(0xFFi64 << (8 * i));
                v |= (*b as i64) << (8 * i);
            }
            return Ok(v);
        }
        if mag <= 128 {
            return Ok(c as i64 + 5);
        }
        Err(format!("marshal long 非法字节 0x{c:02x} @{}", self.pos))
    }

    fn value(&mut self) -> Result<Rb, String> {
        let op = self.u8()?;
        match op {
            b'0' => Ok(Rb::Nil),
            b'T' => Ok(Rb::Bool(true)),
            b'F' => Ok(Rb::Bool(false)),
            b'i' => Ok(Rb::Int(self.long()?)),
            b'f' => {
                // Float：ASCII 十进制字符串
                let len = self.long()? as usize;
                let s = String::from_utf8_lossy(self.take(len)?).into_owned();
                Ok(Rb::Float(s.parse().unwrap_or(0.0)))
            }
            b'l' => {
                // Bignum：符号字节 + u16 字数 + 数据（这里只需跳过，值为 0 占位）
                let _sign = self.u8()?;
                let words = u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as usize;
                self.take(words * 2)?;
                Ok(Rb::Int(0))
            }
            b'"' => {
                let len = self.long()? as usize;
                let raw = self.take(len)?.to_vec();
                let r = Ok(Rb::Str(raw));
                self.objects.push(r.clone()?);
                r
            }
            b':' => {
                let len = self.long()? as usize;
                let s = String::from_utf8_lossy(self.take(len)?).into_owned();
                self.symbols.push(s.clone());
                Ok(Rb::Sym(s))
            }
            b';' => {
                let idx = self.long()? as usize;
                Ok(Rb::Sym(self.symbols.get(idx).cloned().unwrap_or_default()))
            }
            b'[' => {
                let len = self.long()? as usize;
                let mut items = Vec::with_capacity(len.min(4096));
                for _ in 0..len {
                    items.push(self.value()?);
                }
                let r = Ok(Rb::Array(items));
                self.objects.push(r.clone()?);
                r
            }
            b'{' => {
                let len = self.long()? as usize;
                let mut pairs = Vec::with_capacity(len.min(4096));
                for _ in 0..len {
                    let k = self.value()?;
                    let v = self.value()?;
                    pairs.push((k, v));
                }
                let r = Ok(Rb::Hash(pairs));
                self.objects.push(r.clone()?);
                r
            }
            b'}' => {
                // hash with default
                let len = self.long()? as usize;
                let mut pairs = Vec::with_capacity(len.min(4096));
                for _ in 0..len {
                    let k = self.value()?;
                    let v = self.value()?;
                    pairs.push((k, v));
                }
                let _default = self.value()?;
                let r = Ok(Rb::Hash(pairs));
                self.objects.push(r.clone()?);
                r
            }
            b'o' => {
                // object: 类名 + ivars
                let class_name = match self.value()? {
                    Rb::Sym(s) => s,
                    Rb::Str(b) => String::from_utf8_lossy(&b).into_owned(),
                    _ => String::new(),
                };
                let len = self.long()? as usize;
                let mut vals = Vec::with_capacity(len);
                for _ in 0..len {
                    let _k = self.value()?; // ivar 名（symbol）
                    vals.push(self.value()?);
                }
                let r = Ok(Rb::Object(class_name, vals));
                self.objects.push(r.clone()?);
                r
            }
            b'@' => {
                let idx = self.long()? as usize;
                Ok(self.objects.get(idx).cloned().unwrap_or(Rb::Other))
            }
            b'I' => {
                // ivar 包裹（如字符串带编码）：先值，再跳过 ivar 对
                let v = self.value()?;
                let len = self.long()? as usize;
                for _ in 0..len {
                    let _k = self.value()?;
                    let _x = self.value()?;
                }
                Ok(v)
            }
            b'u' => {
                // user-marshal：类名 + 原始串
                let _cls = self.value()?;
                let len = self.long()? as usize;
                let r = Ok(Rb::Str(self.take(len)?.to_vec()));
                self.objects.push(r.clone()?);
                r
            }
            b'U' => {
                // userdef：类名 + 原始字节（rgss 的 Table/Color/Tone）
                let cls = match self.value()? {
                    Rb::Sym(s) => s,
                    Rb::Str(b) => String::from_utf8_lossy(&b).into_owned(),
                    _ => String::new(),
                };
                let len = self.long()? as usize;
                let raw = self.take(len)?.to_vec();
                let r = Ok(Rb::Object(format!("<{cls}>"), vec![Rb::Str(raw)]));
                self.objects.push(r.clone()?);
                r
            }
            _ => Err(format!("未支持的 marshal 类型码: 0x{op:02x}")),
        }
    }
}

/// 提取 Scripts.rxdata 中的 zlib 压缩 Ruby 源码。
pub fn extract_scripts(data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let v = load(data)?;
    let items = match v {
        Rb::Array(a) => a,
        Rb::Object(_, ivars) => match ivars.into_iter().find(|x| matches!(x, Rb::Array(_))) {
            Some(Rb::Array(a)) => a,
            _ => return Err("Scripts 数据结构异常".into()),
        },
        _ => return Err("Scripts 数据不是数组".into()),
    };
    let mut out = Vec::new();
    for item in items {
        let code = match item {
            Rb::Array(parts) if parts.len() >= 3 => match &parts[2] {
                Rb::Str(s) => Some(s.clone()),
                _ => None,
            },
            Rb::Str(s) => Some(s),
            _ => None,
        };
        if let Some(mut compressed) = code {
            let mut z = flate2::read::ZlibDecoder::new(&compressed[..]);
            let mut src = Vec::new();
            if std::io::Read::read_to_end(&mut z, &mut src).is_ok() {
                out.push(src);
            } else {
                out.push(std::mem::take(&mut compressed));
            }
        }
    }
    Ok(out)
}

/// 把 Rb 尽力转成 serde_json::Value（存档查看用）。
pub fn rb_to_json(v: &Rb) -> serde_json::Value {
    use serde_json::json;
    match v {
        Rb::Nil => serde_json::Value::Null,
        Rb::Bool(b) => json!(b),
        Rb::Int(i) => json!(i),
        Rb::Float(f) => json!(f),
        Rb::Str(s) => json!(String::from_utf8_lossy(s).into_owned()),
        Rb::Sym(s) => json!(format!(":{s}")),
        Rb::Array(a) => serde_json::Value::Array(a.iter().map(rb_to_json).collect()),
        Rb::Hash(pairs) => {
            let mut m = serde_json::Map::new();
            for (i, (k, val)) in pairs.iter().enumerate() {
                let key = match k {
                    Rb::Sym(s) => s.clone(),
                    Rb::Str(b) => String::from_utf8_lossy(b).into_owned(),
                    Rb::Int(i) => i.to_string(),
                    other => format!("#{i}:{other:?}"),
                };
                m.insert(key, rb_to_json(val));
            }
            serde_json::Value::Object(m)
        }
        Rb::Object(cls, vals) => {
            let mut m = serde_json::Map::new();
            m.insert("_class".into(), json!(cls));
            m.insert(
                "_ivars".into(),
                serde_json::Value::Array(vals.iter().map(rb_to_json).collect()),
            );
            serde_json::Value::Object(m)
        }
        Rb::Other => json!(null),
    }
}

/// 供导出用的辅助：marshal 对象 → JSON 字符串。
pub fn to_json_string(data: &[u8]) -> Result<String, String> {
    let v = load(data)?;
    serde_json::to_string_pretty(&rb_to_json(&v)).map_err(|e| e.to_string())
}

// HashMap 引用避免 unused 警告（占位）
#[allow(dead_code)]
type _Unused = HashMap<String, String>;
