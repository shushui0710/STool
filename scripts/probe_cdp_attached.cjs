// 用浏览器级 ws 调 Target.getTargets，打印原始 targetInfos（含 attached 标志）。
// 目的：判断 STool 的会话当前钉在哪个目标上。
// 用法: node probe_cdp_attached.cjs [port]
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

(async () => {
  const ver = JSON.parse(await getJson("/json/version"));
  const ws = new WebSocket(ver.webSocketDebuggerUrl);
  await new Promise((res, rej) => {
    ws.addEventListener("open", res);
    ws.addEventListener("error", () => rej(new Error("ws connect failed")));
  });
  ws.addEventListener("message", (ev) => {
    const v = JSON.parse(ev.data);
    if (v.id !== 1) return;
    console.log("Target.getTargets =>");
    for (const t of v.result.targetInfos) {
      console.log(`  type=${t.type}  attached=${t.attached}  title=${t.title}`);
      console.log(`    url=${t.url}`);
      console.log(`    id=${t.targetId}`);
    }
    ws.close();
    process.exit(0);
  });
  ws.send(JSON.stringify({ id: 1, method: "Target.getTargets", params: {} }));
  setTimeout(() => { console.log("timeout"); process.exit(1); }, 8000);
})().catch((e) => { console.log("失败:", e.message); process.exit(1); });
