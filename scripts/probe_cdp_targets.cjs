// 用 CDP 的 browser 级 WebSocket 枚举全部 target，并尝试在游戏页上求值。
// 关键结论点：NW.js(Chromium 85) 的 /json 列表里看不到主窗口页，
//             必须走 Target.getTargets + Target.attachToTarget(flatten)。
// 用法: node probe_cdp_targets.cjs [port]
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
    req.on("timeout", () => {
      req.destroy();
      reject(new Error("http timeout"));
    });
  });
}

function rpc(ws, id, method, params = {}, sessionId) {
  return new Promise((resolve, reject) => {
    const msg = { id, method, params };
    if (sessionId) msg.sessionId = sessionId;
    const t = setTimeout(() => reject(new Error(`${method} timeout`)), 8000);
    const onMsg = (ev) => {
      let v;
      try {
        v = JSON.parse(ev.data);
      } catch {
        return;
      }
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

(async () => {
  const ver = JSON.parse(await getJson("/json/version"));
  console.log("== /json/version ==");
  console.log("  Browser:", ver.Browser);
  console.log("  browserWs:", ver.webSocketDebuggerUrl);

  const ws = new WebSocket(ver.webSocketDebuggerUrl);
  await new Promise((res, rej) => {
    ws.addEventListener("open", res);
    ws.addEventListener("error", () => rej(new Error("ws connect failed")));
  });
  console.log("  [browser ws 已连接]");

  const all = await rpc(ws, 1, "Target.getTargets");
  console.log("== Target.getTargets 全量 ==");
  for (const t of all.targetInfos) {
    console.log(`  type=${t.type}  attached=${t.attached}  title="${t.title}"`);
    console.log(`      url=${t.url}`);
    console.log(`      id=${t.targetId}`);
  }

  // 挑一个"非 chrome-extension"的目标；优先 url 是 file:// / http:// 的
  const cands = all.targetInfos.filter((t) => !String(t.url).startsWith("chrome-extension://"));
  console.log(`== 非扩展目标数: ${cands.length} ==`);

  const pick = cands.find((t) => t.type === "page") || cands[0];
  if (!pick) {
    console.log("!! 没有任何非扩展目标 —— 游戏页不可达");
    ws.close();
    return;
  }
  console.log(`== 选中: type=${pick.type} url=${pick.url} ==`);

  const att = await rpc(ws, 2, "Target.attachToTarget", { targetId: pick.targetId, flatten: true });
  const sid = att.sessionId;
  console.log("  sessionId:", sid);

  for (const expr of ["1+1", "typeof $gameParty", "typeof $gameParty !== 'undefined' ? $gameParty._gold : 'n/a'"]) {
    try {
      const r = await rpc(ws, 3 + Math.floor(Math.random() * 1000), "Runtime.evaluate", { expression: expr, returnByValue: true }, sid);
      console.log(`  eval(${expr}) => ${JSON.stringify(r.result && r.result.value)}`);
    } catch (e) {
      console.log(`  eval(${expr}) => 失败: ${e.message}`);
    }
  }
  ws.close();
})().catch((e) => {
  console.log("probe 失败:", e.message);
  process.exit(1);
});
