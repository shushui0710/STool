/* STool 运行时汉化注入（MTool 式）—— 由 STool 自动生成，请勿手改
 * 读取顺序：<游戏目录>/stool_translate.json → translation.json
 * 格式：{ "原文": "译文" }（也允许分组嵌套，加载时自动拍平）
 * 规则：整句优先；整句没命中时再做「最长片段」贪心替换（与 MTool 同款，因为表里有上千条
 *      片段键，如「は防御の構えを取った！」「のダメージを受けた！」）；都没命中就保留原文。
 *      不修改任何游戏文件。
 *      —— 只替换「给人看的文字」：数据库/地图里的普通字符串 + 事件指令的文字参数（白名单）。
 *      —— 资源名字段（title1Name/faceName/*Name、音频描述符的 name）与路径/备注一律跳过，
 *         否则引擎会按中文名去找图片音频（典型报错 Failed to load img/titles1/中文名.png）。
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

    function loadMap() {
        var files = ["stool_translate.json", "translation.json"];
        try {
            if (typeof require === "function" && typeof process !== "undefined" &&
                process.versions && process.versions.node && typeof location !== "undefined") {
                var fs = require("fs");
                var p = require("path");
                var root = decodeURIComponent(location.pathname).replace(/\\/g, "/");
                root = root.slice(0, root.lastIndexOf("/"));
                if (/^\/[A-Za-z]:/.test(root)) root = root.slice(1); // Windows 下 XHR 风格路径去掉开头斜杠
                for (var i = 0; i < files.length; i++) {
                    var f = p.join(root, files[i]);
                    if (fs.existsSync(f)) {
                        MAP = parseJsonText(fs.readFileSync(f, "utf8"));
                        if (MAP) { console.log("[STool] 汉化映射已加载: " + files[i] + " (" + Object.keys(MAP).length + " 条)"); return; }
                    }
                }
            }
        } catch (e) { console.warn("[STool] 本地读取失败，改用 XHR", e); }
        for (var j = 0; j < files.length; j++) {
            try {
                var xhr = new XMLHttpRequest();
                xhr.open("GET", files[j], false);
                xhr.overrideMimeType("application/json");
                xhr.send(null);
                if (xhr.status === 200 || (xhr.status === 0 && xhr.responseText)) {
                    MAP = parseJsonText(xhr.responseText);
                    if (MAP) { console.log("[STool] 汉化映射已加载(XHR): " + files[j]); return; }
                }
            } catch (e2) { /* 忽略，试下一个 */ }
        }
        console.warn("[STool] 未找到翻译 JSON，本次运行不做替换");
    }

    // ---- 替换规则：整句优先 → 未命中再做「最长片段」贪心替换 -------------------
    // 为什么需要子串：MTool 的表里有上千条「片段键」（は防御の構えを取った！ / のダメージを受けた！ /
    // おっぱいですよ」 …），是给「角色名/数字 + 模板」拼出来的句子用的。只做整句匹配的话，
    // 「SHUは防御の構えを取った！」「\v[61]ダメージを受けた！」这类一句都命中不了（等于整段漏掉）。
    var TRIE = null;       // 翻译表的前缀树。贪心取最长片段靠它，别退回「逐位×每种键长」的写法
    var TRIE_END = "\u0000"; // 节点上存译文的键（正文不会含 NUL）
    var MIN_FRAG = 2;      // 片段最短长度。1 字键会把整段文本改烂，永不参与子串替换
    var CACHE = Object.create(null); // 输入 → 输出。drawText 每帧重复率高，缓存是性能关键
    var CACHE_N = 0;
    var CACHE_MAX = 8192;

    function buildTrie() {
        var root = {};
        for (var k in MAP) {
            if (k.length < MIN_FRAG) continue;
            var v = MAP[k];
            if (typeof v !== "string" || v.length === 0) continue;
            var node = root;
            for (var i = 0; i < k.length; i++) {
                var c = k.charAt(i);
                node = node[c] || (node[c] = {});
            }
            node[TRIE_END] = v;
        }
        TRIE = root;
    }

    // 从左到右，每个起点沿 Trie 走到走不动为止，取「命中的最长前缀」。
    // 一趟走下来只按实际字符推进，不做任何子串分配 —— 早先用「按长度降序逐个 substr 试」
    // 的写法，实测 2.2ms/串，一屏几百次 drawText 直接把游戏拖垮。
    function subst(s) {
        if (!TRIE) return s;
        var out = "", i = 0, n = s.length;
        while (i < n) {
            var node = TRIE, best = null, bestLen = 0;
            for (var j = i; j < n; j++) {
                node = node[s.charAt(j)];
                if (!node) break;
                var v = node[TRIE_END];
                if (typeof v === "string") { best = v; bestLen = j - i + 1; }
            }
            if (best !== null) { out += best; i += bestLen; }
            else { out += s.charAt(i); i++; }
        }
        return out;
    }

    function tr(s) {
        if (!MAP || typeof s !== "string" || s === "") return s;
        var m = MAP[s];
        if (typeof m === "string" && m.length > 0) return m; // 整句命中：最准，直接用
        if (!TRIE) return s;
        var c = CACHE[s];
        if (typeof c === "string") return c;
        var out = subst(s);
        if (CACHE_N >= CACHE_MAX) { CACHE = Object.create(null); CACHE_N = 0; }
        CACHE[s] = out;
        CACHE_N++;
        return out;
    }

    // 这些字段存的是「资源文件名」或「引擎/插件备注」——把译文写回去会让引擎按中文名去找
    // 图片/音频，直接报 "Failed to load img/xxx/中文名.png"。翻译表里的原文常与资源名同形
    // （本游戏实测 158 个：タイトル画面 / スチル1 / ガーデン・シティ_2 ...），所以按字段跳过。
    var SKIP_KEYS = {
        characterName: 1, faceName: 1, battlerName: 1, note: 1, meta: 1,
        title1Name: 1, title2Name: 1,
        battleback1Name: 1, battleback2Name: 1,
        parallaxName: 1, tilesetName: 1, animationName: 1
    };
    function looksLikePath(s) {
        return /[\/\\]/.test(s) ||
            /\.(png|ogg|m4a|wav|mp3|jpg|jpeg|webp|json|txt|rvdata2?|rpgmvp|rpgmvo|rpgmvm|exe)$/i.test(s);
    }
    // 音频描述符 {name, volume, pitch, pan}：这里的 name 是音频文件名（$dataSystem.titleBgm 等）
    function isSoundDescriptor(o) {
        return !Array.isArray(o) && typeof o.name === "string" && typeof o.volume === "number";
    }

    // 事件指令「参数位 → 给人看的文字」白名单。**没列到的一律不动**，这样
    // 显示图片(231/232)、播放音频(241/245/249/250)、更换战斗背景(132)、更换角色图像(236)、
    // 脚本(355/655)、注释(108/408) 这些携带资源名或代码的指令就不会被改坏。
    var TEXT_PARAM = {
        101: [4], // 显示文字：说话人姓名（[0] 是头像文件名，不能动）
        102: [0], // 显示选项：选项文本数组
        401: [0], // 显示文字：正文行
        402: [0], // 当[选项]：分支标题要跟选项一起变
        405: [0], // 显示滚动文字：正文行
        320: [1], // 更改姓名
        324: [1], // 更改称号
        325: [1]  // 更改简介
    };
    function applyTextParams(cmd) {
        var idx = TEXT_PARAM[cmd.code];
        if (!idx) return;
        var ps = cmd.parameters;
        if (!Array.isArray(ps)) return;
        for (var i = 0; i < idx.length; i++) {
            var v = ps[idx[i]];
            if (typeof v === "string") ps[idx[i]] = tr(v);
            else if (Array.isArray(v)) {
                for (var j = 0; j < v.length; j++) if (typeof v[j] === "string") v[j] = tr(v[j]);
            }
        }
    }

    function walk(v, depth, seen) {
        if (depth > 8 || !v || typeof v !== "object") return;
        for (var i = 0; i < seen.length; i++) if (seen[i] === v) return; // 循环引用保护
        seen.push(v);
        if (Array.isArray(v)) {
            for (var a = 0; a < v.length; a++) {
                var e = v[a];
                if (e && typeof e === "object") walk(e, depth + 1, seen);
                else if (typeof e === "string") v[a] = tr(e);
            }
        } else if (typeof v.code === "number" && Array.isArray(v.parameters)) {
            applyTextParams(v); // 事件指令：只碰白名单参数位
        } else {
            var sound = isSoundDescriptor(v);
            for (var k in v) {
                if (!Object.prototype.hasOwnProperty.call(v, k)) continue;
                if (SKIP_KEYS[k]) continue;
                if (sound && k === "name") continue; // 音频文件名
                var val = v[k];
                if (val && typeof val === "object") walk(val, depth + 1, seen);
                else if (typeof val === "string" && !looksLikePath(val)) v[k] = tr(val);
            }
        }
        seen.pop();
    }

    function walkDatabase() {
        var names = ["$dataActors", "$dataClasses", "$dataSkills", "$dataItems", "$dataWeapons",
            "$dataArmors", "$dataEnemies", "$dataTroops", "$dataStates", "$dataSystem",
            "$dataMapInfos", "$dataCommonEvents"];
        for (var i = 0; i < names.length; i++) {
            var d = window[names[i]];
            if (d) walk(d, 0, []);
        }
    }

    function installDataHooks() {
        // Scene_Boot.start 时全部数据库已加载（MV/MZ 一致）
        if (typeof Scene_Boot !== "undefined" && Scene_Boot.prototype && Scene_Boot.prototype.start) {
            var _sb = Scene_Boot.prototype.start;
            Scene_Boot.prototype.start = function () {
                var r = _sb.apply(this, arguments);
                try { walkDatabase(); } catch (e) { console.warn("[STool] 数据库替换出错", e); }
                return r;
            };
        }
        // 每张地图的事件文本（对话 401 / 选项 102 / 显示滚动文字等）
        if (typeof Scene_Map !== "undefined" && Scene_Map.prototype && Scene_Map.prototype.start) {
            var _sms = Scene_Map.prototype.start;
            Scene_Map.prototype.start = function () {
                var r = _sms.apply(this, arguments);
                try { if (typeof $dataMap !== "undefined" && $dataMap) walk($dataMap, 0, []); } catch (e) {}
                return r;
            };
        }
    }

    // 动态文本（脚本临时拼出来的对话等）显示层兜底；MV/MZ 同签名，改 arguments[0] 即可
    // 显示层兜底：动态拼出来的文本（含 \v[n] 之类转义码展开后的结果）只能在这里拦。
    // Window_Base 覆盖常规窗口；Bitmap.drawText 再兜最底层一层 —— DTextPicture / Text2Frame
    // 这类「把文字直接画进位图」的插件不经过 Window_Base，只挂 Window_Base 会整片漏掉。
    function patchDrawText(obj, name) {
        if (!obj || typeof obj[name] !== "function") return;
        var orig = obj[name];
        obj[name] = function () {
            if (arguments.length > 0) arguments[0] = tr(arguments[0]);
            return orig.apply(this, arguments);
        };
    }

    function installTextHooks() {
        if (typeof Window_Base !== "undefined") {
            patchDrawText(Window_Base.prototype, "drawText");
            patchDrawText(Window_Base.prototype, "drawTextEx");
        }
        if (typeof Bitmap !== "undefined") patchDrawText(Bitmap.prototype, "drawText");
    }

    loadMap();
    if (MAP) {
        buildTrie(); // 片段替换用的前缀树，装钩子前必须先建好
        if (typeof DataManager !== "undefined" || typeof Scene_Boot !== "undefined") installDataHooks();
        installTextHooks();
    }
})();
