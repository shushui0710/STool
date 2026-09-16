//! 运行时实时修改：RPG Maker MV/MZ 调试协议通道（CDP）。
//!
//! 原理（与 MTool 同思路，不注入 DLL）：
//! NW.js 游戏内嵌 Chromium。用 `--remote-debugging-port=PORT` 启动游戏后，
//! 通过 Chrome DevTools Protocol 的 WebSocket 在游戏页面里执行 JS，
//! 直接读写 $gameVariables / $gameSwitches / 金币 / 物品 —— 改后即时生效。
//!
//! 通用引擎（RPG Maker XP/VX、Wolf、SRPG、Unity、Ren'Py 等）请用
//! `memscan` 内存扫描修改器，本模块不再重复。
//!
//! 实战踩过的四个坑（都在 `try_connect` / `http_get_json` / 状态读取上）：
//! 1. **`/json` 里报的页面 URL 不可信**：NW.js 把主游戏窗口跑在内置扩展源下，
//!    上报成 `chrome-extension://<id>/index.html`，看着像扩展页、其实就是游戏页。
//!    别按 URL 挑页面，只按「能不能读到 `$gameParty`」挑。
//! 2. **HTTP 响应不能靠 EOF 判断读完**：Chromium 忽略 `Connection: close` 保持长连接，
//!    `read_to_end` 会一直等 EOF 直到读超时；Windows 读超时 errno = 10060（WSAETIMEDOUT），
//!    表现为「连接调试端口 7654 失败」——实际响应早就完整收到了。必须按 Content-Length / chunked 收。
//! 3. **「已声明」≠「已创建」**：MV/MZ 把 `$gameParty` / `$dataSystem` / `$gameVariables`
//!    在 `rmmz_managers.js` 里声明成 **`null`**，标题画面下 `typeof` 是 `"object"` 却是 null。
//!    读状态 / 读名称表一律要判 null（`read_state` / `read_names` 已经判了），
//!    写操作走 `ensure_ready()` 给「请先进游戏」的提示，而不是把 TypeError 丢给用户。
//! 4. **「连上了」≠「连对页面了」**：同一扩展源下还挂着没有 `$gameParty` 的
//!    `background_page`，且往往**先于游戏页就绪**。绝不能把它当兜底收下 ——
//!    否则会话钉死在那里，界面永远「还没进入存档」、点「刷新」永远不动。
//!    只接受 `is_rpgm_page()` 为真的目标，并留 `DebugGame::retarget()` 让刷新能自纠。

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
        let mut cmd = std::process::Command::new(exe);
        cmd.arg(format!("--remote-debugging-port={port}"));
        // 工作目录固定到游戏自己的目录：NW.js 解析 app 是看 exe 位置（已实测与 cwd 无关），
        // 但游戏自身按相对路径写 save/ 之类时看 cwd —— 不设就可能落到 STool 的目录里。
        if let Some(dir) = exe.parent() {
            cmd.current_dir(dir);
        }
        cmd.spawn().map(|c| c.id()).map_err(|e| format!("启动失败: {e}"))
    }

    /// 连接调试端口。游戏刚启动时页面可能还没就绪，内部会轮询最多 15 秒。
    ///
    /// 失败文案要能区分「端口没开」和「端口开了但读不出响应」：
    /// 后者曾被 `read_to_end` 等 EOF 误报成连接失败（见 `http_get_json`）。
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
            "连接调试端口 {port} 失败：{last_err}\n\
             提示：游戏必须带调试参数启动（Game.exe --remote-debugging-port={port}），且是 NW.js 版 RPG Maker MV/MZ。\n\
             改法：① 用本页的「启动并连接」，由 STool 带参数把游戏拉起来；\
             ② 若游戏是你自己双击开的，先完全关掉再点「启动并连接」。\n\
             注：MV 的网页文件在 www\\ 子目录，MZ 直接放在游戏根目录 —— 两种都支持。"
        ))
    }

    /// 重新挑选目标页 —— 会话可能钉在了错页面上，或游戏重启后换了页面。
    ///
    /// 背景（2026-09 实测）：STool 是「启动游戏 → 立刻连接」，那一刻游戏页目标
    /// 可能还没出现在 `/json` 里，列表里只有 NW.js 的 `background_page`。
    /// 旧实现会把这个后台页当兜底收下，于是**整个会话钉死在一个没有 `$gameParty`
    /// 的页面上**：`read_state` 永远返回 `mv=false`，界面永远说「还没进入存档」，
    /// 用户点「刷新」自然毫无反应（刷的是同一个错目标）。所以刷新 / 写入前都先自纠。
    ///
    /// 返回 `Ok(true)` = 换了目标（名称表得重读）；`Ok(false)` = 本来就是游戏页。
    pub fn retarget(&mut self, port: u16) -> Result<bool, String> {
        if self.is_rpgm_page().unwrap_or(false) {
            return Ok(false);
        }
        let g = try_connect(port)?;
        self.ws = g.ws;
        self.next_id = 1;
        Ok(true)
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

    /// 探测游戏状态（金币 / 变量 / 开关 / 物品）。返回状态 JSON。
    ///
    /// 全程判 null：标题画面时 `$gameParty` / `$gameVariables` 都是 null，
    /// 不判就会抛 `TypeError`，界面连「还没进存档」这句提示都显示不出来。
    /// 此时 `mv` 为 false，由界面侧给出改法。
    pub fn read_state(&mut self) -> Result<Value, String> {
        let js = "JSON.stringify((function(){\
            var gp = (typeof $gameParty !== 'undefined' && $gameParty !== null) ? $gameParty : null;\
            var gv = (typeof $gameVariables !== 'undefined' && $gameVariables !== null) ? $gameVariables : null;\
            var gs = (typeof $gameSwitches !== 'undefined' && $gameSwitches !== null) ? $gameSwitches : null;\
            return {\
                mv: gp !== null,\
                gold: gp ? gp._gold : null,\
                variables: gv ? gv._data : null,\
                switches: gs ? gs._data : null,\
                items: gp ? gp._items : null\
            };})())";
        let raw = self.eval(js)?;
        let s = raw.as_str().ok_or("状态返回格式异常")?;
        serde_json::from_str(s).map_err(|e| format!("状态 JSON 解析失败: {e}"))
    }

    /// 读取名称表：变量名 / 开关名 / 物品名（来自 $dataSystem 和 $dataItems）。
    /// 用于把 "#12" 显示成 "#12 学生的好感度"。
    ///
    /// 数据表在标题画面就已加载（`DataManager.loadDatabase` 在 Scene_Boot 里跑），
    /// 但 `$dataSystem` 同样是被声明成 null 的全局量 —— 必须判 null。
    pub fn read_names(&mut self) -> Result<Value, String> {
        let js = "JSON.stringify((function(){\
            var ds = (typeof $dataSystem !== 'undefined' && $dataSystem !== null) ? $dataSystem : null;\
            var di = (typeof $dataItems !== 'undefined' && $dataItems !== null) ? $dataItems : null;\
            return {\
                vars: ds ? ds.variables : null,\
                sw: ds ? ds.switches : null,\
                items: di ? di.map(function(x){return x ? x.name : null;}) : null\
            };})())";
        let raw = self.eval(js)?;
        let s = raw.as_str().ok_or("名称表返回格式异常")?;
        serde_json::from_str(s).map_err(|e| format!("名称表解析失败: {e}"))
    }

    /// 当前目标是不是「RPG Maker 页面」。
    ///
    /// MV/MZ 的全局量在 `rmmz_managers.js` 里就**声明**了（值为 null），
    /// 所以标题画面也算「已声明」；而浏览器扩展页里根本没有这些名字。
    /// 挑目标时用这个判断，别用 [`DebugGame::is_rpgm_ready`]。
    pub fn is_rpgm_page(&mut self) -> Result<bool, String> {
        Ok(self.eval("typeof $gameParty !== 'undefined'")?.as_bool().unwrap_or(false))
    }

    /// 游戏核心是否已经创建（`DataManager.createGameObjects` 跑过了）。
    ///
    /// ⚠️ MV/MZ 把全局量声明成 **`null`**，所以「已声明」≠「已创建」：
    /// 标题画面 / 读盘途中 `$gameParty` 是 null，此时读金币、变量会直接抛
    /// `TypeError: Cannot read property '_gold' of null`。这里必须判 `!== null`。
    pub fn is_rpgm_ready(&mut self) -> Result<bool, String> {
        Ok(self
            .eval("typeof $gameParty !== 'undefined' && $gameParty !== null")?
            .as_bool()
            .unwrap_or(false))
    }

    /// 写操作前的统一前置检查：没进存档就给「原因 + 修法」，
    /// 而不是把游戏侧的 `TypeError` 原样丢给用户。
    fn ensure_ready(&mut self) -> Result<(), String> {
        if self.is_rpgm_ready()? {
            return Ok(());
        }
        Err("游戏还没进入存档（停在标题画面或正在读盘），读不到也改不了金币 / 变量。\n\
             改法：在游戏里点「开始游戏」或「继续游戏」进到游戏画面，再点「刷新」。"
            .to_string())
    }

    pub fn set_gold(&mut self, n: i64) -> Result<Value, String> {
        self.ensure_ready()?;
        self.eval(&format!("$gameParty.gainGold({n} - $gameParty._gold); $gameParty._gold"))
    }

    pub fn set_variable(&mut self, id: i64, val: &Value) -> Result<Value, String> {
        self.ensure_ready()?;
        let lit = js_literal(val)?;
        self.eval(&format!("$gameVariables.setValue({id}, {lit}); $gameVariables.value({id})"))
    }

    pub fn set_switch(&mut self, id: i64, on: bool) -> Result<Value, String> {
        self.ensure_ready()?;
        self.eval(&format!("$gameSwitches.setValue({id}, {on}); $gameSwitches.value({id})"))
    }

    /// 设置物品数量（gainItem 内部会刷新菜单与持有上限）。
    pub fn set_item(&mut self, item_id: i64, count: i64) -> Result<Value, String> {
        self.ensure_ready()?;
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

/// 获取 `/json` 列表，挑出**游戏页**并建立 WebSocket。
///
/// ⚠️ **不能按 URL 挑页面**：NW.js（Chromium 85 那批构建）把主游戏窗口跑在
/// 内置扩展的源下，`/json` 里报的是 `chrome-extension://<内置扩展 id>/index.html`，
/// 看着像扩展页、其实就是游戏页（实测该目标里 `document.title` 是游戏名，
/// `$gameParty` / `SceneManager` / `$dataSystem` 都在）。
///
/// ⚠️ **也不能「连上就算数」**：同一个扩展源下还挂着一个
/// `type=background_page` 的 `_generated_background_page.html`。它**没有**
/// `$gameParty`，却常常**先于游戏页就绪**。旧实现「连上但不是游戏页就留作兜底」，
/// 于是把它收下当会话 → 之后所有读状态都 `mv=false`，界面永远显示
/// 「还没进入存档」，点「刷新」永远不动。2026-09 实测确认：STool 的会话确实钉在
/// `background_page` 上（`attached=true`），而 `page` 上 `$gameParty` / `SceneManager`
/// 一切正常、`SceneManager._scene` 已经是 `Scene_Map`。
///
/// 所以判定只有一条：**必须 `is_rpgm_page()` 为真**。用 `is_rpgm_page` 而不是
/// `is_rpgm_ready` —— 标题画面下 `$gameParty` 是 `null`，但页面是对的，
/// 拿 ready 判会把正确页面也拒掉。挑不到就返回错误，让 [`DebugGame::connect`]
/// 继续轮询：**宁可多等几秒，也不静默钉在错页面上**。
fn try_connect(port: u16) -> Result<DebugGame, String> {
    let body = http_get_json(&format!("127.0.0.1:{port}"), "/json")?;
    let list: Vec<Value> = serde_json::from_str(&body).map_err(|e| format!("目标列表解析失败: {e}"))?;
    let mut cands: Vec<(&str, &str)> = list
        .iter()
        .filter_map(|t| {
            let ws = t.get("webSocketDebuggerUrl").and_then(|x| x.as_str())?;
            let ty = t.get("type").and_then(|x| x.as_str()).unwrap_or("");
            Some((ty, ws))
        })
        .collect();
    if cands.is_empty() {
        return Err("调试端口已开，但没发现可附加的页面（游戏可能还没加载完）".into());
    }
    // page 优先（stable sort：同档保留服务端给的顺序），只为少试几次错目标；
    // 真正说了算的是下面那句 `is_rpgm_page()`。
    cands.sort_by_key(|(ty, _)| u8::from(*ty != "page"));

    let mut first_err = String::new();
    let mut seen_non_game = false;
    for (_, ws_url) in cands {
        match WsConn::connect(ws_url) {
            Ok(ws) => {
                let mut g = DebugGame { ws, next_id: 1 };
                if g.is_rpgm_page().unwrap_or(false) {
                    return Ok(g);
                }
                seen_non_game = true; // NW.js 的 background_page 会落到这里
            }
            Err(e) => {
                if first_err.is_empty() {
                    first_err = e;
                }
            }
        }
    }
    Err(if seen_non_game {
        "调试端口已开，但还没出现游戏画面（游戏可能仍在加载，或读到的不是 RPG Maker 页面）"
            .to_string()
    } else if first_err.is_empty() {
        "调试端口已开，但无法附加到任何页面".to_string()
    } else {
        first_err
    })
}

/// 单次 HTTP 响应体上限：DevTools 的 /json 只有几 KB，8 MiB 纯粹兜底防炸内存。
const MAX_HTTP_BYTES: usize = 8 * 1024 * 1024;

/// 极简 HTTP GET：按 **Content-Length / chunked** 收全响应体，读回文本。
///
/// ⚠️ 这里**不能**用 `read_to_end`（靠对端关连接判断结束）：
/// Chromium 的 DevTools HTTP 服务会忽略 `Connection: close` 保持长连接，
/// `read_to_end` 就一直等 EOF，3 秒后触发读超时；Windows 下读超时的 errno 正是
/// **10060（WSAETIMEDOUT）**，于是「响应其实已完整收到」被误报成
/// 「连接调试端口 7654 失败：… (os error 10060)」——已实测复现（收到 951 字节仍报错）。
fn http_get_json(host: &str, path: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(host).map_err(|e| format!("连接 {host} 失败: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(3))).ok();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if response_complete(&buf) {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break, // 对端关连接：拿现有数据尽力解析
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => {
                // 读超时/被中断前若已收全，按成功处理（这正是 Chromium 长连接的场景）
                if !response_complete(&buf) {
                    return Err(format!("读取 {host}{path} 响应失败: {e}"));
                }
                break;
            }
        }
        if buf.len() > MAX_HTTP_BYTES {
            return Err(format!("HTTP 响应超过 {MAX_HTTP_BYTES} 字节，已放弃"));
        }
    }
    let body = extract_body(&buf).ok_or_else(|| format!("{host}{path} 的响应不是合法 HTTP"))?;
    Ok(body.trim_start_matches('\u{feff}').trim().to_string())
}

/// `buf` 里是否已经有一个**完整**的 HTTP 响应。
/// 头没读完 / 体不够长 → false（还得继续读）；
/// 既没有 `Content-Length` 也不是 chunked → false（只能靠对端关连接收尾）。
fn response_complete(buf: &[u8]) -> bool {
    let Some((head, body_at)) = split_head(buf) else {
        return false;
    };
    let head = String::from_utf8_lossy(head);
    if header_value(&head, "transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        return chunked_complete(&buf[body_at..]);
    }
    if let Some(n) = header_value(&head, "content-length").and_then(|v| v.trim().parse::<usize>().ok()) {
        return buf.len() - body_at >= n;
    }
    false
}

/// 取响应体文本：chunked 先解块，有 `Content-Length` 取定长，否则取剩余全部。
fn extract_body(buf: &[u8]) -> Option<String> {
    let (head, body_at) = split_head(buf)?;
    let head = String::from_utf8_lossy(head);
    let rest = &buf[body_at..];
    if header_value(&head, "transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        return Some(String::from_utf8_lossy(&dechunk(rest)).into_owned());
    }
    let n = header_value(&head, "content-length")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(rest.len())
        .min(rest.len());
    Some(String::from_utf8_lossy(&rest[..n]).into_owned())
}

/// 找响应头结束位置：返回 (头字节, 体起始下标)。`\r\n\r\n` 与 `\n\n` 都认。
fn split_head(buf: &[u8]) -> Option<(&[u8], usize)> {
    if let Some(p) = find_sub(buf, b"\r\n\r\n") {
        return Some((&buf[..p], p + 4));
    }
    if let Some(p) = find_sub(buf, b"\n\n") {
        return Some((&buf[..p], p + 2));
    }
    None
}

/// 朴素子串查找（只在几 KB 的头部 / 块头里用，不需要更快的算法）。
fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// 从响应头里取某个头的值（大小写不敏感；同名只取第一个）。
fn header_value(head: &str, name: &str) -> Option<String> {
    let want = name.to_ascii_lowercase();
    head.lines().skip(1).find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k.trim().to_ascii_lowercase() == want).then(|| v.trim().to_string())
    })
}

/// chunked 体是否已收到终止块（0 长度块）。
fn chunked_complete(body: &[u8]) -> bool {
    let mut i = 0usize;
    loop {
        let Some(nl) = find_sub(&body[i..], b"\r\n").map(|p| p + i) else {
            return false;
        };
        let size_line = String::from_utf8_lossy(&body[i..nl]);
        let Ok(n) = usize::from_str_radix(size_line.split(';').next().unwrap_or("").trim(), 16) else {
            return false;
        };
        i = nl + 2;
        if n == 0 {
            return true; // 0 长度块：后面还有 trailer/空行，但体已结束
        }
        if body.len() < i + n + 2 {
            return false;
        }
        i += n + 2; // 数据 + 结尾 CRLF
    }
}

/// 解 chunked 体（把所有数据块拼起来）。
fn dechunk(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < body.len() {
        let Some(nl) = find_sub(&body[i..], b"\r\n").map(|p| p + i) else {
            break;
        };
        let size_line = String::from_utf8_lossy(&body[i..nl]);
        let Ok(n) = usize::from_str_radix(size_line.split(';').next().unwrap_or("").trim(), 16) else {
            break;
        };
        i = nl + 2;
        if n == 0 {
            break;
        }
        if body.len() < i + n {
            out.extend_from_slice(&body[i..]); // 残缺块：能拿多少拿多少
            break;
        }
        out.extend_from_slice(&body[i..i + n]);
        i += n + 2;
    }
    out
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
    use std::net::TcpListener;

    #[test]
    fn js_literal_types() {
        assert_eq!(js_literal(&json!(12)).unwrap(), "12");
        assert_eq!(js_literal(&json!(true)).unwrap(), "true");
        assert_eq!(js_literal(&json!("勇者")).unwrap(), "'勇者'");
        assert_eq!(js_literal(&json!("it's")).unwrap(), "'it\\'s'");
        assert_eq!(js_literal(&Value::Null).unwrap(), "null");
        assert_eq!(js_literal(&json!([1, 2])).unwrap(), "[1,2]");
    }

    // -----------------------------------------------------------------------
    // HTTP 响应解析 —— 「连接调试端口 7654 失败 os error 10060」的根因回归
    // -----------------------------------------------------------------------

    const RESP: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Type: application/json\r\n\r\n[1,2]";

    #[test]
    fn http_complete_by_content_length() {
        // 头完了、体没收全 → 还得继续读
        assert!(!response_complete(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n[1,"));
        // 体够了就算完整 —— **关键**：不依赖对端关连接（Chromium 就是不关）
        assert!(response_complete(RESP));
        // 多收到的字节不影响判定
        let mut more = RESP.to_vec();
        more.extend_from_slice(b"tail");
        assert!(response_complete(&more));
    }

    #[test]
    fn http_incomplete_cases() {
        assert!(!response_complete(b""), "空缓冲");
        assert!(!response_complete(b"HTTP/1.1 200 OK\r\nContent-Len"), "头没结束");
        // 既无 Content-Length 也非 chunked：只能等 EOF，不能算完整
        assert!(!response_complete(b"HTTP/1.1 200 OK\r\nServer: x\r\n\r\nhi"));
    }

    #[test]
    fn http_chunked_completeness() {
        assert!(!response_complete(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n"));
        assert!(response_complete(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n"));
        // 头名大小写不敏感
        assert!(response_complete(b"HTTP/1.1 200 OK\r\ntransfer-encoding: Chunked\r\n\r\n0\r\n\r\n"));
    }

    #[test]
    fn http_extract_body_variants() {
        assert_eq!(extract_body(RESP).unwrap(), "[1,2]");
        assert_eq!(
            extract_body(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n")
                .unwrap(),
            "abcde"
        );
        // 无长度信息 → 取剩余全部
        assert_eq!(extract_body(b"HTTP/1.1 200 OK\r\nA: b\r\n\r\nxyz").unwrap(), "xyz");
        // Content-Length 报大了 → 有多少给多少，不 panic
        assert_eq!(extract_body(b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\nab").unwrap(), "ab");
        // 头都没结束 → 明确失败
        assert!(extract_body(b"HTTP/1.1 200 OK\r\n").is_none());
    }

    /// 服务端带 `Content-Length` 但**故意不关连接**（Chromium DevTools 的真实行为）。
    /// 老实现用 `read_to_end` 会在这里一直等 EOF，3 秒后读超时 → os error 10060。
    #[test]
    fn http_get_json_survives_server_that_never_closes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = s.read(&mut req);
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nContent-Type: application/json\r\n\r\n[{\"a\":1}]",
            );
            let _ = s.flush();
            // 保持连接开着（远超客户端 3 秒读超时）
            std::thread::sleep(Duration::from_secs(10));
        });
        let body = http_get_json(&addr.to_string(), "/json").unwrap();
        assert_eq!(body, "[{\"a\":1}]");
    }

    /// 响应**确实**没收全时必须报错，不能把半截体当成功返回。
    #[test]
    fn http_get_json_errors_on_truncated_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = s.read(&mut req);
            // 声明 100 字节，一个字节都不发
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n");
            let _ = s.flush();
            std::thread::sleep(Duration::from_secs(10));
        });
        let err = http_get_json(&addr.to_string(), "/json").unwrap_err();
        assert!(err.contains("响应失败"), "应报读取失败，实际：{err}");
    }
}

// ---------------------------------------------------------------------------
// 假 CDP 服务：钉死「挑目标」的取舍
//    回归 2026-09 用户报的「点刷新没反应」——
//    STool 会话钉在了 NW.js 的 background_page 上（那上面没有 $gameParty），
//    于是读状态永远 mv=false、界面永远「还没进入存档」，刷新当然毫无反应。
// ---------------------------------------------------------------------------

#[cfg(test)]
mod pick_target_tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;

    /// 一个迷你 CDP：同一端口既答 `GET /json`（按给定目标列表），
    /// 也接受 WebSocket 并回答 `Runtime.evaluate`（按目标决定 `$gameParty` 在不在）。
    /// 只模拟 `try_connect` 真正用到的那几种报文。
    /// `targets` 的**顺序就是 `/json` 返回的顺序**（NW.js 实测后台页可能排在游戏页前面）。
    fn start(targets: Vec<(&'static str, bool)>) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let spec: Arc<Vec<(String, bool)>> =
            Arc::new(targets.into_iter().map(|(t, g)| (t.to_string(), g)).collect());
        std::thread::spawn(move || {
            for conn in l.incoming().flatten() {
                let spec = spec.clone();
                std::thread::spawn(move || serve(conn, port, spec));
            }
        });
        port
    }

    fn serve(mut s: TcpStream, port: u16, spec: Arc<Vec<(String, bool)>>) {
        let head = read_head(&mut s);
        if head.starts_with("GET /json") {
            let items: Vec<Value> = spec
                .iter()
                .enumerate()
                .map(|(i, (ty, _))| {
                    json!({
                        "id": format!("t{i}"),
                        "type": ty,
                        "title": "rmmz-game",
                        "url": "chrome-extension://njgcanhfjdabfmnlmpmdedalocpafnhl/index.html",
                        "webSocketDebuggerUrl": format!("ws://127.0.0.1:{port}/devtools/page/t{i}"),
                    })
                })
                .collect();
            let body = Value::Array(items).to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes());
            let _ = s.flush();
            return;
        }
        // WebSocket 分支：请求行是 `GET /devtools/page/t<idx> HTTP/1.1`，
        // 先按空格取第 2 段拿到纯路径，再取末段 —— 别直接 rsplit('/')，
        // 那样会连 " HTTP/1.1" 一起带进来（parse 失败退化成 0，测试就假过了）。
        let path = head
            .lines()
            .next()
            .unwrap_or("")
            .split_whitespace()
            .nth(1)
            .unwrap_or("");
        let idx: usize = path
            .rsplit('/')
            .next()
            .and_then(|x| x.strip_prefix('t'))
            .and_then(|x| x.parse().ok())
            .unwrap_or(0);
        let has_game = spec.get(idx).map(|x| x.1).unwrap_or(false);
        // 客户端只校验响应行是 "HTTP/1.1 101"，不校验 Sec-WebSocket-Accept
        let _ = s.write_all(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        );
        let _ = s.flush();
        while let Some(payload) = read_frame(&mut s) {
            let id = serde_json::from_slice::<Value>(&payload)
                .ok()
                .and_then(|v| v.get("id").cloned())
                .unwrap_or_else(|| json!(1));
            let body = json!({
                "id": id,
                "result": { "result": { "type": "boolean", "value": has_game } }
            })
            .to_string();
            if write_text(&mut s, &body).is_err() {
                return;
            }
        }
    }

    /// 读到 `\r\n\r\n` 为止的请求头。
    fn read_head(s: &mut TcpStream) -> String {
        let mut v = Vec::new();
        let mut b = [0u8; 1];
        while matches!(s.read(&mut b), Ok(1)) {
            v.push(b[0]);
            if v.ends_with(b"\r\n\r\n") || v.len() > 8192 {
                break;
            }
        }
        String::from_utf8_lossy(&v).into_owned()
    }

    /// 读一条客户端帧（CDP 的请求都很短，只处理掩码文本帧）。
    fn read_frame(s: &mut TcpStream) -> Option<Vec<u8>> {
        let mut h = [0u8; 2];
        s.read_exact(&mut h).ok()?;
        let masked = h[1] & 0x80 != 0;
        let mut len = (h[1] & 0x7f) as usize;
        if len == 126 {
            let mut e = [0u8; 2];
            s.read_exact(&mut e).ok()?;
            len = u16::from_be_bytes(e) as usize;
        } else if len == 127 {
            let mut e = [0u8; 8];
            s.read_exact(&mut e).ok()?;
            len = u64::from_be_bytes(e) as usize;
        }
        let mut mask = [0u8; 4];
        if masked {
            s.read_exact(&mut mask).ok()?;
        }
        let mut p = vec![0u8; len];
        s.read_exact(&mut p).ok()?;
        if masked {
            for (i, b) in p.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
        }
        Some(p)
    }

    /// 发一条服务端文本帧（服务端不掩码）。
    fn write_text(s: &mut TcpStream, text: &str) -> std::io::Result<()> {
        let p = text.as_bytes();
        let mut f = vec![0x81u8];
        if p.len() < 126 {
            f.push(p.len() as u8);
        } else {
            f.push(126);
            f.extend_from_slice(&(p.len() as u16).to_be_bytes());
        }
        f.extend_from_slice(p);
        s.write_all(&f)?;
        s.flush()
    }

    /// 列表里只有后台页时必须**报错**，而不是把它收下当会话。
    #[test]
    fn only_background_page_must_be_rejected() {
        let port = start(vec![("background_page", false)]);
        let err = try_connect(port).err().expect("不能把非游戏页收下当会话");
        assert!(err.contains("还没出现游戏画面"), "文案要说清原因，实际：{err}");
    }

    /// 后台页与游戏页同时在场时，无论谁排在前面都必须挑中游戏页。
    #[test]
    fn picks_game_page_whatever_the_order() {
        for order in [
            vec![("page", true), ("background_page", false)],
            vec![("background_page", false), ("page", true)],
        ] {
            let port = start(order);
            let mut g = try_connect(port).expect("应挑中游戏页");
            assert!(g.is_rpgm_page().unwrap(), "挑中的目标必须能读到 $gameParty");
        }
    }

    /// 目标都没有可附加的 ws 地址时，报错而不是 panic。
    #[test]
    fn no_attachable_target_reports_error() {
        let port = start(vec![]);
        let err = try_connect(port).err().expect("空目标列表应报错");
        assert!(err.contains("没发现可附加的页面"), "实际：{err}");
    }
}
