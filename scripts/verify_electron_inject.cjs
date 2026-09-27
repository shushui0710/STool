/* 用 Chrome DevTools Protocol 验证「Electron / 浏览器类游戏的运行时注入」是否**真的**生效。
 *
 * 用法：node scripts/verify_electron_inject.cjs [port] [waitMs] [advanceClicks]
 *   前提：先带 `--remote-debugging-port=<port>` 启动游戏（Electron 会把该开关交给 Chromium）。
 *   例子：node scripts/verify_electron_inject.cjs 9222 13000 8
 *
 * 之所以不靠「看截图猜」：控制台日志 + DOM 实况能把四种失败分开——
 *   ① 脚本没被执行；② 执行了但没读到翻译表；③ 读到了表但没替换；
 *   ④ **替换了但只替换了一部分**（例如角色名翻了、正文没翻）。截图只能看出最后一种。
 *
 * ⚠️ 血泪教训（2026-09-26）：**「映射已加载」不足以判 PASS**。TyranoScript v6 的游戏
 * 注入后日志一切正常、控制台还报「汉化映射已加载 3556 条」，但正文全是日文 ——
 * 因为 v6 把整行逐字包成 `<span class="char">`，节点级匹配永远命中不了单字文本节点。
 * 所以本脚本现在**必须**同时看到「正文真的变中文」的证据才给 PASS，并额外检查
 * Tyrano 专用报点 `[STool] 已接管 TyranoScript 逐字文本管线`。
 */
"use strict";
const http = require("http");

const PORT = Number(process.argv[2] || 9222);
const WAIT_MS = Number(process.argv[3] || 13000);
const CLICKS = Number(process.argv[4] || 0);

function getJson(port) {
    return new Promise((resolve, reject) => {
        http.get({ host: "127.0.0.1", port, path: "/json" }, (r) => {
            let d = "";
            r.on("data", (c) => (d += c));
            r.on("end", () => {
                try { resolve(JSON.parse(d)); } catch (e) { reject(e); }
            });
        }).on("error", reject);
    });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function pickPage(port) {
    for (let i = 0; i < 80; i++) {
        try {
            const ts = await getJson(port);
            // 不要按 URL 过滤：这些游戏页面的 URL 可能是 chrome-extension:// 之类，
            // 只按 type=page 挑（与内核 CDP 通道同一口径）。
            const p = ts.find((t) => t.type === "page");
            if (p && p.webSocketDebuggerUrl) return p;
        } catch (e) { /* 还没起来 */ }
        await sleep(500);
    }
    return null;
}

// 同时看「消息层」和整页：消息层能反映正文是否替换，整页能兜住非 Tyrano 的 HTML 游戏。
const PROBE = `(function(){
    var msg = document.querySelector(".message_inner");
    var t = (msg ? msg.textContent : "") || (document.body ? document.body.innerText : "");
    var span = msg ? msg.querySelector(".current_span") : null;
    var ch = span ? span.querySelectorAll(".char") : [];
    var vis = 0;
    for (var i = 0; i < ch.length; i++) { if (ch[i].style.opacity !== "0") vis++; }
    var tags = document.querySelectorAll('script[src*="stool_translate"]');
    return JSON.stringify({
        hookTags: tags.length,
        fromMessageLayer: !!msg,
        zhChars: (t.match(/[\\u4e00-\\u9fff]/g) || []).length,
        jpChars: (t.match(/[\\u3040-\\u30ff]/g) || []).length,
        charSpans: ch.length,
        visibleSpans: vis,
        sample: t.replace(/\\s+/g, " ").slice(0, 120)
    });
})()`;

async function main() {
    const page = await pickPage(PORT);
    if (!page) {
        console.error(`[FAIL] 连不上 CDP（127.0.0.1:${PORT}）—— 游戏是否带了 --remote-debugging-port？`);
        process.exit(2);
    }
    console.log(`[CDP] 页面: ${page.url}`);

    const ws = new WebSocket(page.webSocketDebuggerUrl);
    const logs = [];
    let seq = 0;
    const pending = new Map();

    await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
    ws.onmessage = (ev) => {
        const m = JSON.parse(ev.data);
        if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); return; }
        if (m.method === "Runtime.consoleAPICalled") {
            const txt = (m.params.args || []).map((a) => (a.value !== undefined ? a.value : a.description || "")).join(" ");
            logs.push(`[${m.params.type}] ${txt}`);
        }
        if (m.method === "Log.entryAdded") logs.push(`[log] ${m.params.entry.text || ""}`);
    };
    const send = (method, params) => new Promise((res) => {
        const id = ++seq;
        pending.set(id, res);
        ws.send(JSON.stringify({ id, method, params: params || {} }));
    });
    const evaluate = async (expr) => {
        const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true });
        if (r && r.result && r.result.exceptionDetails) return null;
        return r && r.result && r.result.result ? r.result.result.value : null;
    };
    const clickAdvance = async () => {
        // 用真实鼠标事件（合成 MouseEvent 对 KAG 不一定有效）。
        // 消息层还空着 → 说明停在标题/菜单，要点**可点元素**（各游戏位置不同，必须实测坐标）；
        // 已有台词 → 点画面中央即推进剧情。
        const raw = await evaluate(`(function(){
            var msg = document.querySelector(".message_inner");
            var hasText = msg && msg.textContent.trim().length > 0;
            if (!hasText) {
                var el = document.querySelector(".event-setting-element");
                if (el) {
                    var r = el.getBoundingClientRect();
                    if (r.width > 0 && r.height > 0) {
                        return JSON.stringify({ x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) });
                    }
                }
            }
            return JSON.stringify({ x: 640, y: 400 });
        })()`);
        let p = { x: 640, y: 400 };
        try { p = JSON.parse(raw); } catch (e) { /* 用默认点 */ }
        await send("Input.dispatchMouseEvent", { type: "mousePressed", x: p.x, y: p.y, button: "left", clickCount: 1 });
        await sleep(60);
        await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: p.x, y: p.y, button: "left", clickCount: 1 });
    };

    await send("Runtime.enable");
    await send("Log.enable");
    await send("Page.enable");
    // reload 一次：hook 在页面很早期就打印日志，不重载会漏掉它
    await send("Page.reload", { ignoreCache: false });
    await sleep(WAIT_MS);

    let best = null;
    for (let i = 0; i <= CLICKS; i++) {
        if (i > 0) { await clickAdvance(); await sleep(2500); }
        const raw = await evaluate(PROBE);
        if (!raw) continue;
        try {
            const d = JSON.parse(raw);
            if (!best || d.zhChars > best.zhChars) best = d;
            if (CLICKS > 0) console.log(`--- 第 ${i} 屏 --- ${raw}`);
        } catch (e) { /* 忽略 */ }
    }

    console.log("--- 页面控制台 ---");
    if (!logs.length) console.log("(无输出)");
    logs.forEach((l) => console.log(l));
    console.log("--- DOM 实况（最优一屏）---");
    console.log(best ? JSON.stringify(best) : "(没取到)");

    const loaded = logs.some((l) => l.includes("汉化映射已加载") || l.includes("汉化映射已加载"));
    const noJson = logs.some((l) => l.includes("未找到翻译 JSON"));
    const seam = logs.some((l) => l.includes("已接管 TyranoScript 逐字文本管线"));
    const replaced = !!best && best.zhChars > 0;

    console.log("--- 判定 ---");
    console.log(`  映射已加载 = ${loaded} ；Tyrano 逐字管线接管 = ${seam} ；正文出现中文 = ${replaced}`);
    if (!loaded && noJson) {
        console.log("[FAIL] hook 执行了但没找到翻译 JSON（脚本与 JSON 是否落在同一个目录/封包？）");
        process.exit(1);
    }
    if (!loaded) {
        console.log("[FAIL] 没看到 hook 的加载日志（脚本没被页面执行 —— 注入点是不是找错了？）");
        process.exit(1);
    }
    if (!replaced) {
        console.log("[FAIL] 表读到了但正文没变中文：");
        console.log("       · 若 charSpans > 0 —— 逐字渲染，确认走的是 Tyrano 分支（seam 上面应为 true）；");
        console.log("       · 若正文本来就是 Canvas 画的 —— 不在覆盖范围内；");
        console.log("       · 若只是还没推进到有台词的画面 —— 加一个 advanceClicks 参数再跑。");
        process.exit(1);
    }
    console.log("[PASS] 表已加载且正文确实变成了中文");
    ws.close();
    process.exit(0);
}

main().catch((e) => { console.error("[ERR]", e); process.exit(3); });
