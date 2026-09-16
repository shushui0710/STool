// 枚举调试端口下的**全部**目标，并在每个目标上求值，判定哪些是「游戏页」。
//
// 用途：验证 `try_connect` 的「按 type 排序 + 第一个连上的留作兜底」是否会钉错目标。
// 用法: node probe_cdp_all_targets.cjs [port]
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

const EXPR =
  "'gp=' + (typeof $gameParty==='undefined'?'UNDEF':($gameParty===null?'NULL':'OBJ'))" +
  " + ' sm=' + (typeof SceneManager)" +
  " + ' scene=' + ((typeof SceneManager!=='undefined'&&SceneManager._scene)?SceneManager._scene.constructor.name:'-')";

(async () => {
  const ver = JSON.parse(await getJson("/json/version"));
  console.log("devtools:", ver["Browser"] || ver["User-Agent"] || "?");
  const ws = new WebSocket(ver.webSocketDebuggerUrl);
  await new Promise((res, rej) => {
    ws.addEventListener("open", res);
    ws.addEventListener("error", () => rej(new Error("ws connect failed")));
  });

  const all = await rpc(ws, 1, "Target.getTargets");
  const infos = all.targetInfos || [];
  console.log(`共 ${infos.length} 个目标：\n`);

  // 按 try_connect 的规则排序：page 优先，其余保持原序
  const sorted = infos.slice().sort((a, b) => (a.type === "page" ? 0 : 1) - (b.type === "page" ? 0 : 1));

  let id = 100;
  for (const t of sorted) {
    console.log(`- type=${t.type}  id=${t.targetId}`);
    console.log(`  url=${t.url}`);
    console.log(`  title=${t.title}`);
    if (!t.webSocketDebuggerUrl) { console.log("  (不可附加)\n"); continue; }
    try {
      const att = await rpc(ws, id++, "Target.attachToTarget", { targetId: t.targetId, flatten: true });
      const r = await rpc(ws, id++, "Runtime.evaluate", { expression: EXPR, returnByValue: true }, att.sessionId);
      const v = r.result && r.result.value;
      console.log(`  => ${v === undefined ? JSON.stringify(r.result) : v}`);
      console.log(`  是游戏页？${String(v).includes("gp=OBJ") || String(v).includes("gp=NULL") ? "是" : "否"}`);
      await rpc(ws, id++, "Target.detachFromTarget", { sessionId: att.sessionId });
    } catch (e) {
      console.log(`  => 求值失败: ${e.message}`);
    }
    console.log("");
  }
  ws.close();
})().catch((e) => { console.log("失败:", e.message); process.exit(1); });
