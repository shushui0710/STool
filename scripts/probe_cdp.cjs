// 精确复现 STool `http_get_json` 的行为，判断 10060 到底出在 connect / write / read。
// 用法: node probe_cdp.cjs [port]
const net = require("net");

const PORT = Number(process.argv[2] || 7654);
const HOST = "127.0.0.1";

function httpGetRaw(host, port, path, timeoutMs = 3000) {
  return new Promise((resolve) => {
    const t0 = Date.now();
    let stage = "connect";
    let bytes = 0;
    const chunks = [];
    const s = net.connect(port, host);
    s.setTimeout(timeoutMs);
    s.on("connect", () => {
      stage = "write";
      s.write(`GET ${path} HTTP/1.1\r\nHost: ${host}:${port}\r\nConnection: close\r\n\r\n`);
      stage = "read";
    });
    s.on("data", (d) => {
      bytes += d.length;
      chunks.push(d);
    });
    s.on("end", () =>
      resolve({ ok: true, ms: Date.now() - t0, bytes, stage, err: null, body: Buffer.concat(chunks).toString("utf8") })
    );
    s.on("timeout", () => {
      resolve({ ok: false, ms: Date.now() - t0, bytes, stage, err: `READ TIMEOUT at ${stage}`, body: Buffer.concat(chunks).toString("utf8") });
      s.destroy();
    });
    s.on("error", (e) => {
      resolve({ ok: false, ms: Date.now() - t0, bytes, stage, err: `${stage}: ${e.message} (code=${e.code})`, body: "" });
      s.destroy();
    });
  });
}

(async () => {
  const r = await httpGetRaw(HOST, PORT, "/json");
  console.log("== STool http_get_json 复现 ==");
  console.log(JSON.stringify({ ok: r.ok, ms: r.ms, bytes: r.bytes, stage: r.stage, err: r.err }, null, 0));
  if (!r.ok) {
    console.log("--- 出错前已收到的字节（STool 会整段丢弃）---");
    console.log(JSON.stringify(r.body.slice(0, 400)));
  }
  // 再单独打印全文，看清 target 列表
  const r2 = r.ok ? r : await httpGetRaw(HOST, PORT, "/json", 3000);
  if (r2.body) {
    console.log("== /json 原文 ==");
    console.log(r2.body);
    try {
      const arr = JSON.parse(r2.body.match(/\[[\s\S]*\]/)[0]);
      console.log("== target 摘要 ==");
      for (const t of arr) console.log(`  type=${t.type}  title="${t.title}"  url=${t.url}`);
      const firstPage = arr.find((t) => t.type === "page");
      console.log("== STool 会挑中的第一个 page ==");
      console.log("  " + JSON.stringify(firstPage && { title: firstPage.title, url: firstPage.url }));
    } catch (e) {
      console.log("解析失败: " + e.message);
    }
  }
})();
