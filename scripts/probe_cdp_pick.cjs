// 严格复刻内核 `try_connect` 的挑选过程，把每一步都打出来：
//  1. 取 /json 原始列表（含顺序与 type）
//  2. 按 type 排序（page 优先）—— 和内核一致
//  3. 对每个目标的 webSocketDebuggerUrl 单独建连，求值 is_rpgm_page
//  4. 打印内核会最终选中的那个（第一个 is_rpgm_page=true，否则第一个连上的兜底）
//
// 用途：验证「刷新永远报还没进存档」是否因为钉到了 background_page 之类的错目标。
// 用法: node probe_cdp_pick.cjs [port]
const http = require("http");
const PORT = Number(process.argv[2] || 7654);
const HOST = "127.0.0.1";

function getJson(path, timeoutMs = 5000) {
  return new Promise((resolve, reject) => {
    const req = http.get({ host: HOST, port: PORT, path, timeout: timeoutMs }, (r) => {
      let d = "";
      r.on("data", (c) => (d += c));
      r.on("end", () => resolve(d));
    });
    req.on("error", reject);
    req.on("timeout", () => { req.destroy(); reject(new Error("http timeout")); });
  });
}

/** 在某个目标的 ws 上求一条表达式，然后关掉。 */
function evalOn(wsUrl, expr, timeoutMs = 8000) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    const t = setTimeout(() => { try { ws.close(); } catch {} reject(new Error("timeout")); }, timeoutMs);
    ws.addEventListener("error", () => { clearTimeout(t); reject(new Error("ws 连接失败")); });
    ws.addEventListener("open", () => {
      ws.send(JSON.stringify({ id: 1, method: "Runtime.evaluate",
        params: { expression: expr, returnByValue: true } }));
    });
    ws.addEventListener("message", (ev) => {
      let v; try { v = JSON.parse(ev.data); } catch { return; }
      if (v.id !== 1) return;
      clearTimeout(t);
      try { ws.close(); } catch {}
      if (v.error) return reject(new Error(JSON.stringify(v.error)));
      // 直连目标的响应形如 {"id":1,"result":{"result":{type,value}}}
      // —— 与内核走的 /result/result/value 一致。
      const ro = v.result && v.result.result;
      resolve(ro ? ro.value : undefined);
    });
  });
}

const IS_PAGE = "typeof $gameParty !== 'undefined'";
const DETAIL =
  "'gp=' + (typeof $gameParty==='undefined'?'UNDEF':($gameParty===null?'NULL':'OBJ'))" +
  " + ' scene=' + ((typeof SceneManager!=='undefined'&&SceneManager._scene)?SceneManager._scene.constructor.name:'-')" +
  " + ' title=' + document.title";

(async () => {
  const raw = JSON.parse(await getJson("/json"));
  console.log(`/json 返回 ${raw.length} 条（服务端原始顺序）：`);
  raw.forEach((t, i) => {
    console.log(`  #${i} type=${t.type}  url=${t.url}`);
    console.log(`      title=${t.title}  hasWs=${!!t.webSocketDebuggerUrl}`);
  });

  // 复刻内核排序：page 优先，stable
  const cands = raw
    .filter((t) => t.webSocketDebuggerUrl)
    .slice()
    .sort((a, b) => (a.type === "page" ? 0 : 1) - (b.type === "page" ? 0 : 1));

  console.log(`\n内核候选顺序（过滤掉无 ws 的，page 优先）：`);
  cands.forEach((t, i) => console.log(`  [${i}] type=${t.type} url=${t.url}`));

  console.log(`\n逐个建连求值（复刻内核行为）：`);
  let picked = null, fallback = null;
  for (const t of cands) {
    let page = null, detail = null, err = null;
    try { page = await evalOn(t.webSocketDebuggerUrl, IS_PAGE); }
    catch (e) { err = e.message; }
    try { detail = await evalOn(t.webSocketDebuggerUrl, DETAIL); } catch {}
    const mark = err ? "连接失败" : page ? "★ 游戏页" : "非游戏页";
    console.log(`  - type=${t.type}  ${mark}  is_rpgm_page=${page}  ${err ? "err=" + err : detail}`);
    if (!err && page === true && !picked) picked = t;
    if (!err && !fallback) fallback = t;
  }

  const final = picked || fallback;
  console.log(`\n内核最终会选：${final ? `${final.type}  ${final.url}` : "（无）"}`);
  console.log(picked ? "  → 命中游戏页（正常）" : "  → ⚠️ 走兜底！钉在非游戏页上，刷新将永远报「还没进入存档」");
  process.exit(0);
})().catch((e) => { console.log("失败:", e.message); process.exit(1); });
