// 在「第一个 page 目标」上求值，判定它到底是不是游戏页。
// 用法: node cdp_eval_first_page.cjs <port>
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
function rpc(ws, id, method, params = {}, sessionId) {
  return new Promise((resolve, reject) => {
    const msg = { id, method, params };
    if (sessionId) msg.sessionId = sessionId;
    const t = setTimeout(() => reject(new Error(`${method} timeout`)), 8000);
    const onMsg = (ev) => {
      let v; try { v = JSON.parse(ev.data); } catch { return; }
      if (v.id !== id) return;
      clearTimeout(t);
      ws.removeEventListener("message", onMsg);
      if (v.error) reject(new Error(`${method}: ${JSON.stringify(v.error)}`));
      else resolve(v.result);
    };
    ws.addEventListener("message", onMsg);
    ws.send(JSON.stringify(msg));
  });
}

const EXPRS = [
  "location.href",
  "document.title",
  "typeof nw !== 'undefined' ? nw.App.manifest.name : 'no-nw'",
  "typeof $gameParty",
  "typeof SceneManager",
  "typeof $dataSystem",
  "typeof $gameParty !== 'undefined' ? $gameParty._gold : 'n/a'",
  "document.body ? document.body.innerHTML.length : -1",
];

(async () => {
  const ver = JSON.parse(await getJson("/json/version"));
  const ws = new WebSocket(ver.webSocketDebuggerUrl);
  await new Promise((res, rej) => {
    ws.addEventListener("open", res);
    ws.addEventListener("error", () => rej(new Error("ws connect failed")));
  });
  const all = await rpc(ws, 1, "Target.getTargets");
  const page = all.targetInfos.find((t) => t.type === "page");
  if (!page) { console.log("没有 page 目标"); ws.close(); return; }
  console.log(`选中 page: ${page.url}`);
  const att = await rpc(ws, 2, "Target.attachToTarget", { targetId: page.targetId, flatten: true });
  let id = 10;
  for (const e of EXPRS) {
    try {
      const r = await rpc(ws, id++, "Runtime.evaluate", { expression: e, returnByValue: true }, att.sessionId);
      console.log(`  ${e}  =>  ${JSON.stringify(r.result && r.result.value)}`);
    } catch (err) {
      console.log(`  ${e}  =>  失败: ${err.message}`);
    }
  }
  ws.close();
})().catch((e) => { console.log("失败:", e.message); process.exit(1); });
