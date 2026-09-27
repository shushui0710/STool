/* STool TyranoBuilder 运行时汉化注入 —— 由 STool 自动生成，请勿手改
 * 读取：<index.html 同目录>/stool_translate.json（{"原文":"译文"}，允许分组嵌套）
 * 规则：整句精确匹配；未命中保留原文；不修改任何游戏文件（仅本脚本 + JSON 是新增的）。
 * 覆盖：DOM 文本节点（MutationObserver）+ jQuery html/text + message 元素 textContent
 *      + TyranoScript v6 的 buildMessageHTML（逐字 span 正文）。
 * 注意：Tyrano 若把文字画进 Canvas，则不在覆盖范围内。
 */
(function () {
    "use strict";
    var MAP = null;
    var DONE = false;

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

    // 推断入口页所在目录（Electron 打包时页面在 asar 内，fs 补丁可透明读取）。
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
        // 通道 1：Electron 预加载桥（contextIsolation 下页面主世界没有 require/fs，
        // 这类游戏的 preload 常把 fs 挂在 window 上，如 window.studio_api.fs_default）。
        try {
            var api = window.studio_api;
            var fs1 = api && (api.fs_default || api.fs);
            if (fs1 && dir) {
                for (var i = 0; i < files.length; i++) {
                    var f1 = (api.path ? api.path.join(dir, files[i]) : dir + "/" + files[i]);
                    try {
                        if (fs1.existsSync(f1)) {
                            MAP = parseJsonText(fs1.readFileSync(f1, "utf8"));
                            if (MAP) { console.log("[STool] Tyrano 汉化映射已加载: " + files[i]); return; }
                        }
                    } catch (e0) { /* 换下一个候选文件 */ }
                }
            }
        } catch (e) { /* 转下一条通道 */ }
        // 通道 2：页面主世界直接有 node
        try {
            if (typeof require === "function" && typeof process !== "undefined" &&
                process.versions && process.versions.node && dir) {
                var fs2 = require("fs");
                var p2 = require("path");
                for (var k = 0; k < files.length; k++) {
                    var f2 = p2.join(dir, files[k]);
                    if (fs2.existsSync(f2)) {
                        MAP = parseJsonText(fs2.readFileSync(f2, "utf8"));
                        if (MAP) { console.log("[STool] Tyrano 汉化映射已加载: " + files[k]); return; }
                    }
                }
            }
        } catch (e1) { /* 转 XHR */ }
        // 通道 3：XHR（浏览器/明文 HTML 环境）
        for (var j = 0; j < files.length; j++) {
            try {
                var xhr = new XMLHttpRequest();
                xhr.open("GET", files[j], false);
                xhr.overrideMimeType("application/json");
                xhr.send(null);
                if (xhr.status === 200 || (xhr.status === 0 && xhr.responseText)) {
                    MAP = parseJsonText(xhr.responseText);
                    if (MAP) return;
                }
            } catch (e2) { /* 试下一个 */ }
        }
        if (!MAP) console.warn("[STool] 未找到翻译 JSON，本次运行不做替换");
    }

    function tr(s) {
        if (!MAP || typeof s !== "string") return s;
        var m = MAP[s];
        if (typeof m === "string" && m.length > 0) return m;
        var t = s.trim();
        if (t !== s) {
            var m2 = MAP[t];
            if (typeof m2 === "string" && m2.length > 0) return m2;
        }
        return s;
    }

    // ---- 1) DOM 文本节点 ----

    function translateNode(n) {
        var t = n.nodeValue;
        if (typeof t !== "string" || !t.trim()) return;
        var m = tr(t);
        if (m !== t) n.nodeValue = m;
    }

    function walkDom(root) {
        if (!root) return;
        try {
            var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null, false);
            var nodes = [];
            while (walker.nextNode()) nodes.push(walker.currentNode);
            nodes.forEach(translateNode);
        } catch (e) { /* 忽略 */ }
    }

    // ---- 2) message 元素级兜底（文本被 <br> 拆成多节点时） ----

    function translateElem(el) {
        if (!el || el.nodeType !== 1) return;
        var cls = (el.className && typeof el.className === "string") ? el.className : "";
        if (!/(^|\s|-)(message|text|name|serif|glink|tyrano)(\s|-|$)/i.test(cls)) return;
        var t = el.textContent;
        if (typeof t !== "string" || !t.trim()) return;
        var m = tr(t);
        if (m !== t) el.textContent = m;
    }

    function walkElems(root) {
        if (!root || root.nodeType !== 1) return;
        var list = root.querySelectorAll ? root.querySelectorAll(".message,[class*=message]") : [];
        for (var i = 0; i < list.length; i++) translateElem(list[i]);
    }

    // ---- 3) jQuery html/text 包装（Tyrano KAG 写消息的主通道） ----

    function installJqHook() {
        if (typeof window.jQuery !== "function") return;
        var fn = window.jQuery.fn;
        ["html", "text"].forEach(function (name) {
            if (typeof fn[name] !== "function" || fn[name].__stool) return;
            var orig = fn[name];
            var wrapped = function (v) {
                // 只有"写入"（带参数）且是字符串时才尝试替换；读取原样透传
                if (arguments.length > 0 && typeof v === "string") {
                    var m = tr(v);
                    if (m !== v) arguments[0] = m;
                }
                return orig.apply(this, arguments);
            };
            wrapped.__stool = true;
            fn[name] = wrapped;
        });
    }

    // ---- TyranoScript v6：接管「逐字渲染」的正文入口 ----
    //
    // v6 把每个字包成 <span class="char">，DOM 里每个文本节点只剩一个字，
    // 节点级精确匹配永远命中不了正文（角色名那条路才翻得动）。在这里按整行明文替换，
    // 译文仍交给 Tyrano 自己逐字渲染，动画/换行/描边全保留。
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
        if (DONE || !MAP) return;
        DONE = true;
        walkDom(document.body || document.documentElement || document);
        walkElems(document.body || document.documentElement || document);
        var mo = new MutationObserver(function (muts) {
            muts.forEach(function (m) {
                if (m.type === "characterData" && m.target.nodeType === 3) translateNode(m.target);
                m.addedNodes.forEach(function (n) {
                    if (n.nodeType === 3) translateNode(n);
                    else if (n.nodeType === 1) { walkDom(n); walkElems(n); }
                });
            });
        });
        mo.observe(document.documentElement || document, { childList: true, subtree: true, characterData: true });
        pollTyranoSeam();
    }

    loadMap();
    // jQuery 可能比本脚本晚加载（脚本顺序不保证），DOMContentLoaded 再补挂一次
    installJqHook();
    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", function () { installJqHook(); start(); });
    } else {
        installJqHook();
        start();
    }
})();
