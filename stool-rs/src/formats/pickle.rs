//! 最小 pickle 反序列化器（协议 2~5 子集），用于 Ren'Py RPA 索引与 persistent 存档。
//!
//! 支持纯数据 pickle：dict / list / tuple / int / str / bytes / float / bool / None。
//! 不支持全局类实例（Ren'Py 索引不需要；persistent 里遇到则返回占位字符串）。

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Tuple(Vec<Value>),
    Dict(Vec<(Value, Value)>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            Value::Str(s) => Some(s.as_bytes()),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub fn as_dict(&self) -> Option<&Vec<(Value, Value)>> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&Vec<Value>> {
        match self {
            Value::List(l) | Value::Tuple(l) => Some(l),
            _ => None,
        }
    }
}

pub fn loads(data: &[u8]) -> Result<Value, String> {
    let mut p = Parser { data, pos: 0, memo: Vec::new() };
    p.parse()
}

/// GLOBAL('c') 压入的函数占位标记，供 REDUCE 识别 _codecs.encode。
const GLOBAL_FUNC: &str = "\u{0}__global__";

struct Parser<'a> {
    data: &'a [u8],
    pos: usize,
    memo: Vec<Value>,
}

impl<'a> Parser<'a> {
    fn u8(&mut self) -> Result<u8, String> {
        let b = *self.data.get(self.pos).ok_or("pickle 数据越界")?;
        self.pos += 1;
        Ok(b)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err("pickle 数据越界".into());
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u16le(&mut self) -> Result<u16, String> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32le(&mut self) -> Result<u32, String> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32le(&mut self) -> Result<i32, String> {
        Ok(self.u32le()? as i32)
    }
    fn u64le(&mut self) -> Result<u64, String> {
        let s = self.take(8)?;
        Ok(u64::from_le_bytes(s.try_into().unwrap()))
    }

    fn parse(&mut self) -> Result<Value, String> {
        let mut stack: Vec<Value> = Vec::new();
        let mut marks: Vec<usize> = Vec::new();
        loop {
            let op = self.u8()?;
            match op {
                b'(' => marks.push(stack.len()),                          // MARK
                b')' => { stack.push(Value::Tuple(vec![])); }             // EMPTY_TUPLE
                b']' => { stack.push(Value::List(vec![])); }              // EMPTY_LIST
                b'}' => { stack.push(Value::Dict(vec![])); }              // EMPTY_DICT
                b't' => {                                                 // TUPLE
                    let m = *marks.last().ok_or("TUPLE 缺少 MARK")?;
                    let items = stack.split_off(m);
                    marks.pop();
                    stack.push(Value::Tuple(items));
                }
                0x85 => {                                                 // TUPLE1
                    let v = stack.pop().ok_or("栈空")?;
                    stack.push(Value::Tuple(vec![v]));
                }
                0x86 => {                                                 // TUPLE2
                    let (b, a) = (stack.pop().ok_or("栈空")?, stack.pop().ok_or("栈空")?);
                    stack.push(Value::Tuple(vec![a, b]));
                }
                0x87 => {                                                 // TUPLE3
                    let c = stack.pop().ok_or("栈空")?;
                    let b = stack.pop().ok_or("栈空")?;
                    let a = stack.pop().ok_or("栈空")?;
                    stack.push(Value::Tuple(vec![a, b, c]));
                }
                b'a' => {                                                 // APPEND
                    let v = stack.pop().ok_or("栈空")?;
                    match stack.last_mut().ok_or("栈空")? {
                        Value::List(l) => l.push(v),
                        _ => return Err("APPEND 目标不是 list".into()),
                    }
                }
                b'e' => {                                                 // APPENDS
                    let m = *marks.last().ok_or("APPENDS 缺少 MARK")?;
                    let items = stack.split_off(m);
                    marks.pop();
                    match stack.last_mut().ok_or("栈空")? {
                        Value::List(l) => l.extend(items),
                        _ => return Err("APPENDS 目标不是 list".into()),
                    }
                }
                b's' => {                                                 // SETITEM
                    let v = stack.pop().ok_or("栈空")?;
                    let k = stack.pop().ok_or("栈空")?;
                    match stack.last_mut().ok_or("栈空")? {
                        Value::Dict(d) => d.push((k, v)),
                        _ => return Err("SETITEM 目标不是 dict".into()),
                    }
                }
                b'u' => {                                                 // SETITEMS
                    let m = *marks.last().ok_or("SETITEMS 缺少 MARK")?;
                    let items = stack.split_off(m);
                    marks.pop();
                    match stack.last_mut().ok_or("栈空")? {
                        Value::Dict(d) => {
                            for pair in items.chunks(2) {
                                if pair.len() == 2 {
                                    d.push((pair[0].clone(), pair[1].clone()));
                                }
                            }
                        }
                        _ => return Err("SETITEMS 目标不是 dict".into()),
                    }
                }
                b'K' => { let v = self.u8()? as i64; stack.push(Value::Int(v)); }        // BININT1
                b'M' => { let v = self.u16le()? as i64; stack.push(Value::Int(v)); }     // BININT2
                b'J' => { let v = self.i32le()? as i64; stack.push(Value::Int(v)); }     // BININT
                0x8a => {                                                                 // LONG1
                    let n = self.u8()? as usize;
                    let s = self.take(n)?;
                    let mut v: i64 = 0;
                    for (i, b) in s.iter().enumerate() {
                        v |= (*b as i64) << (8 * i);
                    }
                    stack.push(Value::Int(v));
                }
                0x8b => {                                                                 // LONG4
                    let n = self.u32le()? as usize;
                    let s = self.take(n.min(64))?;
                    let mut v: i64 = 0;
                    for (i, b) in s.iter().enumerate().take(8) {
                        v |= (*b as i64) << (8 * i);
                    }
                    stack.push(Value::Int(v));
                }
                b'X' => {                                                                 // BINUNICODE
                    let n = self.u32le()? as usize;
                    let s = self.take(n)?;
                    stack.push(Value::Str(String::from_utf8_lossy(s).into_owned()));
                }
                0x8c => {                                                                 // SHORT_BINUNICODE
                    let n = self.u8()? as usize;
                    let s = self.take(n)?;
                    stack.push(Value::Str(String::from_utf8_lossy(s).into_owned()));
                }
                b'T' => {                                                                 // BINSTRING
                    let n = self.u32le()? as usize;
                    let s = self.take(n)?;
                    stack.push(Value::Bytes(s.to_vec()));
                }
                b'C' => {                                                                 // SHORT_BINBYTES
                    let n = self.u8()? as usize;
                    stack.push(Value::Bytes(self.take(n)?.to_vec()));
                }
                b'B' => {                                                                 // BINBYTES
                    let n = self.u32le()? as usize;
                    stack.push(Value::Bytes(self.take(n)?.to_vec()));
                }
                0x80 => { self.u8()?; }                    // PROTO（版本号）
                0x95 => { self.take(8)?; }                 // FRAME
                0x94 => {                                  // MEMOIZE
                    let v = stack.last().cloned().ok_or("MEMOIZE 栈空")?;
                    self.memo.push(v);
                }
                b'q' => { let i = self.u8()? as usize; self.memo_put(i, stack.last()); }   // SHORT_BINPUT
                b'r' => { let i = self.u32le()? as usize; self.memo_put(i, stack.last()); } // BINPUT
                b'h' => { let i = self.u8()? as usize; Self::memo_get(&mut stack, &self.memo, i)?; } // BINGET
                b'j' => { let i = self.u32le()? as usize; Self::memo_get(&mut stack, &self.memo, i)?; } // LONG_BINGET
                b'G' => {                                  // BINFLOAT
                    let bits = self.u64le()?;
                    stack.push(Value::Float(f64::from_bits(bits)));
                }
                0x88 => stack.push(Value::Bool(true)),     // NEWTRUE
                0x89 => stack.push(Value::Bool(false)),    // NEWFALSE
                b'N' => stack.push(Value::None),           // NONE
                b'S' => {                                  // STRING（引号包裹，latin-1）
                    let mut end = self.pos;
                    while end < self.data.len() && self.data[end] != b'\n' {
                        end += 1;
                    }
                    let raw = &self.data[self.pos..end];
                    self.pos = end + 1;
                    stack.push(Value::Str(parse_quoted(raw)));
                }
                b'V' => {                                  // UNICODE（原始文本行）
                    let mut end = self.pos;
                    while end < self.data.len() && self.data[end] != b'\n' {
                        end += 1;
                    }
                    let s = String::from_utf8_lossy(&self.data[self.pos..end]).into_owned();
                    self.pos = end + 1;
                    stack.push(Value::Str(s));
                }
                b'R' => {                                  // REDUCE
                    let arg = stack.pop().ok_or("栈空")?;
                    let f = stack.pop().ok_or("栈空")?;
                    // _codecs.encode(s, 'latin1') → bytes（Python2 风格 bytes 键，Ren'Py 索引常见）
                    if f == Value::Str(GLOBAL_FUNC.into()) {
                        if let Value::Tuple(items) = &arg {
                            if items.len() == 2 {
                                if let (Value::Str(s), Value::Str(enc)) = (&items[0], &items[1]) {
                                    if enc == "latin1" {
                                        stack.push(Value::Bytes(
                                            s.chars().map(|c| c as u32 as u8).collect(),
                                        ));
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                    stack.push(Value::Str("<object>".to_string()));
                }
                b'c' => {                                  // GLOBAL：module\n name\n → 压入函数占位
                    for _ in 0..2 {
                        while self.pos < self.data.len() && self.data[self.pos] != b'\n' {
                            self.pos += 1;
                        }
                        self.pos += 1;
                    }
                    stack.push(Value::Str(GLOBAL_FUNC.to_string()));
                }
                b'g' | b'p' => {                           // GET/PUT（文本行参数）→ 跳过
                    while self.pos < self.data.len() && self.data[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                    self.pos += 1;
                }
                b'o' | b'i' | 0x93 => {                    // OBJ/INST/NEWOBJ → 占位
                    stack.push(Value::Str("<object>".to_string()));
                }
                b'.' => return stack.pop().ok_or_else(|| "pickle 无返回值".to_string()),
                _ => return Err(format!("未支持的 pickle 操作码: 0x{op:02x} @ {}", self.pos - 1)),
            }
        }
    }

    fn memo_put(&mut self, idx: usize, v: Option<&Value>) {
        while self.memo.len() <= idx {
            self.memo.push(Value::None);
        }
        if let Some(v) = v {
            self.memo[idx] = v.clone();
        }
    }

    fn memo_get(stack: &mut Vec<Value>, memo: &[Value], idx: usize) -> Result<(), String> {
        let v = memo.get(idx).cloned().ok_or("memo 引用越界")?;
        stack.push(v);
        Ok(())
    }
}

fn parse_quoted(raw: &[u8]) -> String {
    // pickle 的 STRING 是 repr 风格带引号文本
    let s = String::from_utf8_lossy(raw);
    let s = s.trim();
    let inner = s
        .strip_prefix('\'')
        .and_then(|x| x.strip_suffix('\''))
        .or_else(|| s.strip_prefix('"').and_then(|x| x.strip_suffix('"')))
        .unwrap_or(s);
    inner
        .replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\t", "\t")
        .replace("\\\\", "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpa_style_index() {
        // 模拟 Ren'Py 索引 pickle：{b"img/a.png": [(10, 20)]}
        let mut idx = std::collections::BTreeMap::new();
        idx.insert(
            b"img/a.png".to_vec(),
            vec![(10u64 ^ 0x1234, 20u64 ^ 0x1234)],
        );
        let data = pickle_rs_dump(&idx);
        let v = loads(&data).unwrap();
        let d = v.as_dict().unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].0.as_bytes().unwrap(), b"img/a.png");
        let chunks = d[0].1.as_list().unwrap();
        let t = chunks[0].as_list().unwrap();
        assert_eq!(t.len(), 2);
    }

    /// 用 Python pickle 语义手工构造一个 dict pickle（协议 2），仅供测试。
    fn pickle_rs_dump(map: &std::collections::BTreeMap<Vec<u8>, Vec<(u64, u64)>>) -> Vec<u8> {
        let mut out = vec![0x80, 2];
        out.push(b'}');
        for (k, chunks) in map {
            out.push(b'X');
            out.extend_from_slice(&(k.len() as u32).to_le_bytes());
            out.extend_from_slice(k);
            out.push(b']');
            for (a, b) in chunks {
                out.push(b'J');
                out.extend_from_slice(&(*a as i32).to_le_bytes());
                out.push(b'J');
                out.extend_from_slice(&(*b as i32).to_le_bytes());
                out.push(0x86); // TUPLE2
                out.push(b'a');
            }
            out.push(b's');
        }
        out.push(b'.');
        out
    }

    #[test]
    fn persistent_style() {
        // persistent: {'seen': True, 'count': 3, 'names': ['a', 'b']}
        let mut data = vec![0x80, 2, b'}'];
        // 直接手工：'seen' -> true
        data.push(b'X'); data.extend_from_slice(&4u32.to_le_bytes()); data.extend_from_slice(b"seen");
        data.push(0x88);
        data.push(b's');
        data.push(b'X'); data.extend_from_slice(&5u32.to_le_bytes()); data.extend_from_slice(b"count");
        data.push(b'K'); data.push(3);
        data.push(b's');
        data.push(b'X'); data.extend_from_slice(&5u32.to_le_bytes()); data.extend_from_slice(b"names");
        data.push(b']');
        for name in ["a", "b"] {
            data.push(b'X'); data.extend_from_slice(&1u32.to_le_bytes()); data.extend_from_slice(name.as_bytes());
            data.push(b'a');
        }
        data.push(b's');
        data.push(b'.');
        let v = loads(&data).unwrap();
        let d = v.as_dict().unwrap();
        assert_eq!(d[0].1, Value::Bool(true));
        assert_eq!(d[1].1, Value::Int(3));
        assert_eq!(d[2].1.as_list().unwrap().len(), 2);
    }
}
