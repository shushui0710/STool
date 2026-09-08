//! 运行时实时修改：RPG Maker MV/MZ 调试协议通道（CDP）。
//!
//! 原理（与 MTool 同思路，不注入 DLL）：
//! NW.js 游戏内嵌 Chromium。用 `--remote-debugging-port=PORT` 启动游戏后，
//! 通过 Chrome DevTools Protocol 的 WebSocket 在游戏页面里执行 JS，
//! 直接读写 $gameVariables / $gameSwitches / 金币 / 物品 —— 改后即时生效。
//!
//! 通用引擎（RPG Maker XP/VX、Wolf、SRPG、Unity、Ren'Py 等）请用
//! `memscan` 内存扫描修改器，本模块不再重复。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// 一个已连接的 MV/MZ 调试会话。
pub struct DebugGame {
    ws: WsConn,
    next_id: u64,
}

impl DebugGame {
    /// 以调试模式启动游戏（自动加 --remote-debugging-port 参数）。
    /// 游戏窗口会正常弹出，之后用 connect 连接。
    pub fn launch(exe: &Path, port: u16) -> Result<u32, String> {
        if !exe.is_file() {
            return Err(format!("游戏程序不存在: {}", exe.display()));
        }
        std::process::Command::new(exe)
            .arg(format!("--remote-debugging-port={port}"))
            .spawn()
            .map(|c| c.id())
            .map_err(|e| format!("启动失败: {e}"))
    }

    /// 连接调试端口。游戏刚启动时页面可能还没就绪，内部会轮询最多 15 秒。
    pub fn connect(port: u16) -> Result<DebugGame, String> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut last_err = String::new();
        while Instant::now() < deadline {
            match try_connect(port) {
                Ok(g) => return Ok(g),
                Err(e) => last_err = e,
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err(format!(
            "连接调试端口 {port} 失败：{last_err}\n提示：游戏必须带调试参数启动，且是 NW.js 版 MV/MZ（目录里有 Game.exe + www/）。"
        ))
    }

    /// 在游戏页面执行任意 JS，返回求值结果。
    pub fn eval(&mut self, js: &str) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "id": id,
            "method": "Runtime.evaluate",
            "params": { "expression": js, "returnByValue": true, "awaitPromise": false }
        });
        self.ws.send_text(&msg.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let text = self.ws.recv_text(deadline)?;
            let v: Value = serde_json::from_str(&text).map_err(|e| format!("CDP 消息解析失败: {e}"))?;
            if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
                if let Some(err) = v.pointer("/error/message") {
                    return Err(format!("CDP 错误: {err}"));
                }
                // 异常对象（如 JS 报错 undefined 变量）
                if v.pointer("/result/result/subtype") == Some(&json!("error")) {
                    let desc = v.pointer("/result/result/description").and_then(|d| d.as_str()).unwrap_or("JS 执行出错");
                    return Err(format!("游戏内 JS 出错: {desc}"));
                }
                return Ok(v.pointer("/result/result/value").cloned().unwrap_or(Value::Null));
            }
            // 其他消息（事件等）忽略，继续等
        }
        Err("等待 CDP 响应超时".into())
    }

    /// 探测游戏状态（是否已进入 RPG Maker 场景）。返回状态 JSON。
    pub fn read_state(&mut self) -> Result<Value, String> {
        let js = "JSON.stringify({\
            mv: typeof $gameParty !== 'undefined',\
            gold: typeof $gameParty !== 'undefined' ? $gameParty._gold : null,\
            variables: typeof $gameVariables !== 'undefined' ? $gameVariables._data : null,\
            switches: typeof $gameSwitches !== 'undefined' ? $gameSwitches._data : null,\
            items: typeof $gameParty !== 'undefined' ? $gameParty._items : null\
        })";
        let raw = self.eval(js)?;
        let s = raw.as_str().ok_or("状态返回格式异常")?;
        serde_json::from_str(s).map_err(|e| format!("状态 JSON 解析失败: {e}"))
    }

    /// 读取名称表：变量名 / 开关名 / 物品名（来自 $dataSystem 和 $dataItems）。
    /// 用于把 "#12" 显示成 "#12 学生的好感度"。
    pub fn read_names(&mut self) -> Result<Value, String> {
        let js = "JSON.stringify({\
            vars: typeof $dataSystem !== 'undefined' ? $dataSystem.variables : null,\
            sw: typeof $dataSystem !== 'undefined' ? $dataSystem.switches : null,\
            items: typeof $dataItems !== 'undefined' ? $dataItems.map(function(x){return x?x.name:null;}) : null\
        })";
        let raw = self.eval(js)?;
        let s = raw.as_str().ok_or("名称表返回格式异常")?;
        serde_json::from_str(s).map_err(|e| format!("名称表解析失败: {e}"))
    }

    /// 判断游戏是否已经加载 RPG Maker 核心（MainMenu/Scene 还没进也认）。
    pub fn is_rpgm_ready(&mut self) -> Result<bool, String> {
        Ok(self.eval("typeof $gameParty !== 'undefined'")?.as_bool().unwrap_or(false))
    }

    pub fn set_gold(&mut self, n: i64) -> Result<Value, String> {
        self.eval(&format!("$gameParty.gainGold({n} - $gameParty._gold); $gameParty._gold"))
    }

    pub fn set_variable(&mut self, id: i64, val: &Value) -> Result<Value, String> {
        let lit = js_literal(val)?;
        self.eval(&format!("$gameVariables.setValue({id}, {lit}); $gameVariables.value({id})"))
    }

    pub fn set_switch(&mut self, id: i64, on: bool) -> Result<Value, String> {
        self.eval(&format!("$gameSwitches.setValue({id}, {on}); $gameSwitches.value({id})"))
    }

    /// 设置物品数量（gainItem 内部会刷新菜单与持有上限）。
    pub fn set_item(&mut self, item_id: i64, count: i64) -> Result<Value, String> {
        let js = format!(
            "(function(){{ var it = $dataItems[{item_id}] || $dataWeapons[{item_id}] || $dataArmors[{item_id}]; \
             if (!it) return null; \
             var cur = $gameParty._items[it.id] || 0; \
             $gameParty.gainItem(it, {count} - cur); \
             return $gameParty._items[it.id] || 0; }})()"
        );
        self.eval(&js)
    }
}

/// 探测调试端口是否已就绪（游戏可能已带调试参数在运行）。
pub fn probe_port(port: u16) -> bool {
    http_get_json(&format!("127.0.0.1:{port}"), "/json").is_ok()
}

/// 在游戏目录里找 MV/MZ 启动程序（Game.exe / nw.exe / 任意非安装器 exe）。
pub fn find_game_exe(root: &Path) -> Option<PathBuf> {
    for name in ["Game.exe", "nw.exe", "Game_start.exe"] {
        let p = root.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let lower = name.to_lowercase();
            let is_exe = p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("exe")).unwrap_or(false);
            let skip = lower.contains("unins") || lower.contains("crash") || lower.contains("report") || lower.contains("updater");
            if is_exe && !skip {
                return Some(p);
            }
        }
    }
    None
}

/// 把 JSON 值转成 JS 字面量。
fn js_literal(v: &Value) -> Result<String, String> {
    match v {
        Value::Number(_) | Value::Bool(_) => Ok(v.to_string()),
        Value::String(s) => Ok(format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))),
        Value::Null => Ok("null".into()),
        Value::Array(_) | Value::Object(_) => Ok(v.to_string()),
    }
}

/// 获取 /json 列表，挑第一个页面，建立 WebSocket。
fn try_connect(port: u16) -> Result<DebugGame, String> {
    let body = http_get_json(&format!("127.0.0.1:{port}"), "/json")?;
    let list: Vec<Value> = serde_json::from_str(&body).map_err(|e| format!("目标列表解析失败: {e}"))?;
    let page = list
        .iter()
        .find(|t| t.get("type").and_then(|x| x.as_str()) == Some("page"))
        .ok_or("调试端口已开但未发现游戏页面")?;
    let ws_url = page
        .get("webSocketDebuggerUrl")
        .and_then(|x| x.as_str())
        .ok_or("页面缺少 webSocketDebuggerUrl")?;
    let ws = WsConn::connect(ws_url)?;
    Ok(DebugGame { ws, next_id: 1 })
}

/// 极简 HTTP GET：读回响应体文本。
fn http_get_json(host: &str, path: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(host).map_err(|e| format!("连接 {host} 失败: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(3))).ok();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf);
    // 跳过头部，找 \r\n\r\n 后的 body
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or(text.into_owned());
    Ok(body.trim_start_matches('\u{feff}').trim().to_string())
}

// ---------------------------------------------------------------------------
// 极简 WebSocket 客户端（仅满足 CDP：文本帧、客户端掩码、ping/pong、分片）
// ---------------------------------------------------------------------------

struct WsConn {
    stream: TcpStream,
}

impl WsConn {
    fn connect(ws_url: &str) -> Result<WsConn, String> {
        // ws://127.0.0.1:9222/devtools/page/XXXX
        let rest = ws_url.strip_prefix("ws://").ok_or("不是 ws:// 地址")?;
        let (hostport, path) = rest.split_once('/').unwrap_or((rest, ""));
        let path = format!("/{path}");
        let mut stream = TcpStream::connect(hostport).map_err(|e| format!("连接 WebSocket {hostport} 失败: {e}"))?;
        stream.set_nodelay(true).ok();
        stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
        // 固定 Key 即可完成握手（本地回环，无中间人）
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        // 读握手响应头
        let mut resp = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match stream.read(&mut byte) {
                Ok(1) => {
                    resp.push(byte[0]);
                    if resp.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                Ok(_) => continue,
                Err(e) => return Err(format!("WebSocket 握手读取失败: {e}")),
            }
            if resp.len() > 8192 {
                return Err("WebSocket 握手响应异常".into());
            }
        }
        let head = String::from_utf8_lossy(&resp);
        if !head.starts_with("HTTP/1.1 101") {
            return Err(format!("WebSocket 握手被拒绝: {}", head.lines().next().unwrap_or("")));
        }
        Ok(WsConn { stream })
    }

    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let payload = text.as_bytes();
        let len = payload.len();
        let mut header = Vec::with_capacity(10);
        header.push(0x81); // FIN + text
        // 客户端帧必须掩码（bit7=1）
        if len < 126 {
            header.push(0x80 | len as u8);
        } else if len < 65536 {
            header.push(0x80 | 126);
            header.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            header.push(0x80 | 127);
            header.extend_from_slice(&(len as u64).to_be_bytes());
        }
        let mask = [0x5A, 0xA5, 0x3C, 0xC3]; // 固定掩码（本地回环可用）
        header.extend_from_slice(&mask);
        let mut frame = header;
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.stream.write_all(&frame).map_err(|e| format!("WebSocket 发送失败: {e}"))?;
        self.stream.flush().map_err(|e| e.to_string())
    }

    /// 读一条完整消息（自动处理分片与 ping）。
    fn recv_text(&mut self, deadline: Instant) -> Result<String, String> {
        let mut message: Vec<u8> = Vec::new();
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err("WebSocket 接收超时".into());
            }
            self.stream
                .set_read_timeout(Some((deadline - now).min(Duration::from_secs(10))))
                .ok();
            let b0 = self.read_exact_n(1)?[0];
            let opcode = b0 & 0x0F;
            let b1 = self.read_exact_n(1)?[0];
            let masked = b1 & 0x80 != 0;
            let mut len = (b1 & 0x7F) as u64;
            if len == 126 {
                let ext = self.read_exact_n(2)?;
                len = u16::from_be_bytes([ext[0], ext[1]]) as u64;
            } else if len == 127 {
                let ext = self.read_exact_n(8)?;
                len = u64::from_be_bytes(ext.try_into().unwrap());
            }
            let mask_key = if masked { Some(self.read_exact_n(4)?) } else { None };
            let mut payload = self.read_exact_n(len as usize)?;
            if let Some(mk) = &mask_key {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= mk[i % 4];
                }
            }
            match opcode {
                0x1 | 0x2 => {
                    message.extend_from_slice(&payload);
                    return Ok(String::from_utf8_lossy(&message).into_owned());
                }
                0x0 => {
                    // 分片续帧
                    message.extend_from_slice(&payload);
                    if b0 & 0x80 != 0 {
                        return Ok(String::from_utf8_lossy(&message).into_owned());
                    }
                }
                0x8 => return Err("游戏侧关闭了 WebSocket 连接".into()),
                0x9 => {
                    // ping → pong
                    let mut pong = vec![0x8A, 0x80];
                    pong.extend_from_slice(&[0; 4]);
                    let _ = self.stream.write_all(&pong);
                }
                0xA => {} // pong，忽略
                _ => {}
            }
        }
    }

    fn read_exact_n(&mut self, n: usize) -> Result<Vec<u8>, String> {
        let mut buf = vec![0u8; n];
        self.stream.read_exact(&mut buf).map_err(|e| format!("WebSocket 读取失败: {e}"))?;
        Ok(buf)
    }
}

// ---------------------------------------------------------------------------
// 测试（纯逻辑部分，不需要真实游戏）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_literal_types() {
        assert_eq!(js_literal(&json!(12)).unwrap(), "12");
        assert_eq!(js_literal(&json!(true)).unwrap(), "true");
        assert_eq!(js_literal(&json!("勇者")).unwrap(), "'勇者'");
        assert_eq!(js_literal(&json!("it's")).unwrap(), "'it\\'s'");
        assert_eq!(js_literal(&Value::Null).unwrap(), "null");
        assert_eq!(js_literal(&json!([1, 2])).unwrap(), "[1,2]");
    }
}
