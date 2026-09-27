/* STool HTML 运行时汉化注入 —— 由 STool 自动生成，请勿手改
 * 读取：<入口 HTML 同目录>/stool_translate.json（{"原文":"译文"}，允许分组嵌套）
 * 规则：整句精确匹配；未命中保留原文。注意：Canvas 画面内的文本无法替换。
 * TyranoScript v6：额外接管 buildMessageHTML —— 它把正文逐字包成 <span class="char">，
 * 节点级匹配必然失效（见 installTyranoSeam 注释）。
 */
(function () {
    "use strict";
    var MAP = null;

    function flatten(obj, out) {
        out = out || {};
        for (var k in obj) {
            if (!Object.prototype.hasOwnProperty.call(obj, k)) continue;
            var v = obj[k];
            if (v && typeof v === "object") flatten(v, out);
            else if (typeof v === "string") out[k] = v;
        }
        return out;
    }

    function parseJsonText(txt) {
        try { return flatten(JSON.parse(txt.replace(/^\uFEFF/, ""))); } catch (e) { return null; }
    }

    // 推断入口页所在目录 —— Electron 打包时页面在 asar 内，形如
    // file:///D:/…/resources/app.asar/index.html，取到 .../resources/app.asar。
    // Electron 自带的 fs 补丁能透明读 asar 内文件，所以这个路径直接可读。
    function pageDir() {
        try {
            var p = decodeURIComponent(location.pathname).replace(/\\/g, "/");
            p = p.slice(0, p.lastIndexOf("/"));
            if (/^\/[A-Za-z]:/.test(p)) p = p.slice(1);
            return p;
        } catch (e) { return ""; }
    }

    function loadMap() {
        var files = ["stool_translate.json", "translation.json"];
        var dir = pageDir();
        // 通道 1：Electron 预加载桥。打包成 Electron 且开了 contextIsolation 时，
        // 页面主世界**没有** require/fs —— 这类游戏的 preload 常把 fs 挂在 window 上
        // （实测的 Tyrano+Electron 游戏就是 window.studio_api.fs_default）。
        try {
            var api = window.studio_api;
            var fs1 = api && (api.fs_default || api.fs);
            if (fs1 && dir) {
                for (var i = 0; i < files.length; i++) {
                    var f1 = (api.path ? api.path.join(dir, files[i]) : dir + "/" + files[i]);
                    try {
                        if (fs1.existsSync(f1)) {
                            MAP = parseJsonText(fs1.readFileSync(f1, "utf8"));
                            if (MAP) { console.log("[STool] 汉化映射已加载: " + files[i] + " (" + Object.keys(MAP).length + " 条)"); return; }
                        }
                    } catch (e0) { /* 换下一个候选文件 */ }
                }
            }
        } catch (e) { /* 转下一条通道 */ }
        // 通道 2：页面主世界直接有 node（nodeIntegration 且未开隔离）
        try {
            if (typeof require === "function" && typeof process !== "undefined" &&
                process.versions && process.versions.node && dir) {
                var fs2 = require("fs");
                var p2 = require("path");
                for (var k = 0; k < files.length; k++) {
                    var f2 = p2.join(dir, files[k]);
                    if (fs2.existsSync(f2)) {
                        MAP = parseJsonText(fs2.readFileSync(f2, "utf8"));
                        if (MAP) { console.log("[STool] 汉化映射已加载: " + files[k] + " (" + Object.keys(MAP).length + " 条)"); return; }
                    }
                }
            }
        } catch (e1) { /* 转 XHR */ }
        // 通道 3：XHR（明文 HTML 游戏走这条）
        for (var j = 0; j < files.length; j++) {
            try {
                var xhr = new XMLHttpRequest();
                xhr.open("GET", files[j], true);
                xhr.overrideMimeType("application/json");
                xhr.onload = function () {
                    if (xhr.status === 200 || (xhr.status === 0 && xhr.responseText)) {
                        MAP = parseJsonText(xhr.responseText);
                        if (MAP && document.readyState !== "loading") start();
                    }
                };
                xhr.send(null);
                break; // 异步只挂第一个候选，未命中就不再尝试
            } catch (e2) { /* 忽略 */ }
        }
        if (!MAP) console.warn("[STool] 未找到翻译 JSON，本次运行不做替换");
    }

    function tr(s) {
        if (!MAP || typeof s !== "string") return s;
        var m = MAP[s];
        return (typeof m === "string" && m.length > 0) ? m : s; // 未命中 → 保留原文
    }

    function translateNode(n) {
        var t = n.nodeValue;
        if (typeof t !== "string" || !t.trim()) return;
        var m = MAP[t] !== undefined ? MAP[t] : MAP[t.trim()];
        if (typeof m === "string" && m.length > 0 && n.nodeValue !== m) n.nodeValue = m;
    }

    function walkDom(root) {
        try {
            var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null, false);
            var nodes = [];
            while (walker.nextNode()) nodes.push(walker.currentNode);
            nodes.forEach(translateNode);
        } catch (e) { /* 忽略 */ }
    }

    // ---- TyranoScript v6：接管「逐字渲染」的正文入口 ----
    //
    // v6 的文本管线是：buildMessageHTML(整行明文) → 把每个字包成
    // <span class="char" style="opacity:0">字</span> → 整串 .current_span.html(html)。
    // 于是 DOM 里每个文本节点只剩一个字，**节点级精确匹配永远命中不了**
    // （实测：角色名能翻、正文全是日文）。在「明文入口」替换最稳：译文交给 Tyrano
    // 自己逐字渲染，打字动画、换行、描边、ruby 全部原样保留。
    // 非 Tyrano 页面拿不到这个对象，函数直接返回 false，无副作用。
    function installTyranoSeam() {
        try {
            var TY = window.tyrano || (typeof tyrano !== "undefined" ? tyrano : null);
            var T = TY && TY.plugin && TY.plugin.kag && TY.plugin.kag.tag && TY.plugin.kag.tag.text;
            if (!T || typeof T.buildMessageHTML !== "function") return false;
            if (T.buildMessageHTML.__stool) return true;
            var orig = T.buildMessageHTML;
            var wrapped = function (message_str) {
                var a = Array.prototype.slice.call(arguments);
                if (typeof message_str === "string") {
                    var m = tr(message_str);
                    if (m !== message_str) a[0] = m;
                }
                return orig.apply(this, a);
            };
            wrapped.__stool = true;
            T.buildMessageHTML = wrapped;
            console.log("[STool] 已接管 TyranoScript 逐字文本管线");
            return true;
        } catch (e) { return false; }
    }

    // Tyrano 一般比本脚本晚初始化 → 轮询到它出现为止（最多 10 秒）
    function pollTyranoSeam() {
        if (installTyranoSeam()) return;
        var n = 0;
        var timer = setInterval(function () {
            if (installTyranoSeam() || ++n > 100) clearInterval(timer);
        }, 100);
    }

    function start() {
        if (!MAP) return;
        walkDom(document.body || document.documentElement || document);
        var mo = new MutationObserver(function (muts) {
            muts.forEach(function (m) {
                if (m.type === "characterData" && m.target.nodeType === 3) translateNode(m.target);
                m.addedNodes.forEach(function (n) {
                    if (n.nodeType === 3) translateNode(n);
                    else if (n.nodeType === 1) walkDom(n);
                });
            });
        });
        mo.observe(document.documentElement || document, { childList: true, subtree: true, characterData: true });
        pollTyranoSeam();
    }

    loadMap();
    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", function () { start(); });
    } else {
        start();
    }
})();
