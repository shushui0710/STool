/*
 * 锁死 TyranoScript v6「正文逐字 <span class="char">」这一类 bug（2026-09-27）。
 *
 * 背景：v6 的正文入口是 `tyrano.plugin.kag.tag.text.buildMessageHTML(整行明文)`，
 * 它把**每个字**包成 `<span class="char" style="opacity:0">字</span>`。
 * 于是 DOM 里每个文本节点只剩一个字 → hook 的节点级精确匹配**永远命中不了正文**
 * （症状指纹：**角色名翻成中文、正文整片日文**）。
 * 修法：hook 包住 buildMessageHTML，在明文入口换掉整行。
 *
 * 为什么要有这个脚本：Rust 单测只能断言「源码里有 buildMessageHTML 这个字符串」，
 * 断言不了「换完之后真的出中文」；真机验证又要开游戏。
 * 这里用**从真实游戏抠出来的 kag.tag.js::buildMessageHTML**（见 fixture 头，逐字节 + sha256）
 * 当真渲染器，把真 hook 跑起来，直接断言渲染结果。
 *
 * 测哪两份 hook：`tyrano.js` 与 `html.js` —— **两份都带这个接缝**，且 asar 打包的 Tyrano 游戏
 * 实际被判成 `html_game`、走的是 `html.js`（本脚本两个都跑，避免只测到"用不上的那份"）。
 *
 * 关键设计——**反向对照**：同一份真渲染器，接管前必须先看到「日文、无中文」，
 * 接管后才看到「中文、无假名」。没有对照的话，测试可能因为别的原因假通过。
 *
 * 用法：node scripts/verify_hook_tyrano.cjs
 */
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const vm = require("vm");

const HOOKS_DIR = process.env.STOOL_HOOKS_DIR || path.resolve(__dirname, "..", "stool-rs", "assets", "hooks");
const FIXTURE = path.resolve(
  __dirname, "..", "stool-rs", "tests", "fixtures", "hooks", "tyrano_v6_buildMessageHTML.js"
);
// STOOL_HOOKS 可逗号分隔指定要测哪几份（配合 STOOL_HOOKS_DIR 对着改动前的副本跑，验证本脚本真能抓到 bug）
const HOOKS = (process.env.STOOL_HOOKS || "tyrano.js,html.js").split(",").map((s) => s.trim());

// 真实游戏里出现过的一行（也是排版较刁钻的一行：长、含「」与片假名）
const LINE = "近年流行している「鈍理症」の原因となる神経ウイルスだ。";
const ZH = "近年流行的「钝理症」的致病神经病毒。";
const UNMAPPED = "この行は辞書にないので原文のまま。";

const KANA = /[\u3040-\u30ff]/g; // 假名：判「还是不是日文」的可靠判据
const CJK = /[\u4e00-\u9fff]/g;  // 汉字/中文（日文汉字也落在这里，所以必须配合假名一起看）
const CJK_ONE = /[\u4e00-\u9fff]/; // 无 /g：给 .test() 用（带 g 的 test 有 lastIndex 状态，会交替漏判）
const count = (s, re) => (s.match(re) || []).length;
const charSpans = (html) => (html.match(/<span class="char"/g) || []).length;

let bad = 0;
const fail = (m) => { bad++; console.log("  ✗ " + m); };
const ok = (m) => console.log("  ✓ " + m);

// 翻译表：键就是上面那行真实台词
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "stool-tyrano-"));
fs.writeFileSync(path.join(tmp, "stool_translate.json"), JSON.stringify({ [LINE]: ZH }), "utf8");

// 每份 hook 都要一份**全新的宿主与 tyrano 对象**：
//   · 同一个进程里连续跑两份，hook 会看到上一份已包好的函数（__stool 标记）→ 测不出第二份；
//   · 所以每次重建 g.tyrano 与 document，并用 vm 重新执行 IIFE（其闭包状态随之刷新）。
function makeHost() {
  const g = globalThis;
  g.window = g;
  g.location = { pathname: encodeURI("/" + tmp.replace(/\\/g, "/") + "/index.html") };
  g.require = require; // hook 的 fs 直读通道要求 require 在作用域里（vm.runInThisContext 里不是全局）
  g.document = {
    readyState: "complete",
    body: { nodeType: 1, querySelectorAll: () => [] },
    documentElement: { nodeType: 1, querySelectorAll: () => [] },
    addEventListener: () => {},
    createTreeWalker: () => ({ nextNode: () => false })
  };
  g.NodeFilter = { SHOW_TEXT: 4 };
  g.MutationObserver = class { constructor() {} observe() {} disconnect() {} };
  g.XMLHttpRequest = function () {
    this.open = () => {}; this.overrideMimeType = () => {}; this.send = () => {};
  };
  // 真 Tyrano 里 this.kag 指向 kag 实例（stat/tmp/config 在它上面），tag 对象持有该引用。
  // 注意：这只是**宿主**；被测对象是 fixture 里那段真渲染器。
  const kagRoot = {
    stat: { word_nobreak_list: [], ruby_str: "", mark: 0, style_mark: "", font: { edge: "" } },
    tmp: { is_individual_decoration: false, is_text_stroke: false },
    config: {}
  };
  const textTag = Object.assign({ kag: kagRoot }, require(FIXTURE));
  g.tyrano = { plugin: { kag: { tag: { text: textTag } } } };
  return textTag;
}

function runOne(hookFile) {
  console.log(`\n=== ${hookFile} ===`);
  const textTag = makeHost();

  // --- 反向对照：接管前 ---
  const before = textTag.buildMessageHTML(LINE);
  console.log("  接管前: char span =", charSpans(before), " 假名 =", count(before, KANA), " 中文 =", count(before, CJK));
  if (charSpans(before) !== LINE.length) {
    fail(`接管前 char span 数应等于字数（${LINE.length}），实为 ${charSpans(before)} —— 固件可能不是真渲染器`);
  }
  if (count(before, KANA) === 0) {
    fail("接管前应含假名（说明这行确实是日文）—— 对照组无效，测试可能假通过");
  }

  // --- 加载真 hook（它会包住 buildMessageHTML）---
  const hookSrc = fs.readFileSync(path.join(HOOKS_DIR, hookFile), "utf8");
  const logs = [];
  const origLog = console.log;
  console.log = (...a) => { logs.push(a.join(" ")); };
  vm.runInThisContext(hookSrc, { filename: "stool_translate.js" });
  console.log = origLog;

  const loaded = logs.some((l) => l.includes("汉化映射已加载"));
  const seam = logs.some((l) => l.includes("已接管 TyranoScript 逐字文本管线"));
  if (!loaded) fail("hook 没读到翻译表（缺少「汉化映射已加载」）");
  if (!seam) fail("hook 没接管 TyranoScript 逐字文本管线（buildMessageHTML 未被包住）");

  // --- 接管后 ---
  const after = textTag.buildMessageHTML(LINE);
  console.log("  接管后: char span =", charSpans(after), " 假名 =", count(after, KANA), " 中文 =", count(after, CJK));
  console.log("  正文中文 =", (after.match(/[\u4e00-\u9fff]/g) || []).join(""));

  if (count(after, CJK) === 0) fail("接管后正文仍无中文 —— 逐字管线没修好");
  if (count(after, KANA) !== 0) fail(`接管后仍残留假名 ${count(after, KANA)} 个 —— 译文没整行替换干净`);
  // 接管后 char span 数应等于**译文**长度：证明引擎仍在对译文逐字渲染（打字动画/描边等未被破坏）。
  if (charSpans(after) !== ZH.length) {
    fail(`接管后 char span 数应等于译文长度（${ZH.length}），实为 ${charSpans(after)} —— 译文没有走引擎原生逐字渲染`);
  }
  for (const c of new Set([...ZH].filter((ch) => CJK_ONE.test(ch)))) if (!after.includes(c)) fail(`译文字符 ${c} 未出现在渲染结果里`);

  // --- 表外的行必须原样透传 ---
  // 对照对象是 fixture 里那份**没被包装过**的原始函数（hook 换掉的是 textTag 自己的属性）。
  const rawUnmapped = require(FIXTURE).buildMessageHTML.call(textTag, UNMAPPED);
  const passthrough = textTag.buildMessageHTML(UNMAPPED);
  if (passthrough !== rawUnmapped) fail("表外文本被改动了（应原样透传，与未接管时输出一致）");
  if (count(passthrough, KANA) === 0) fail("表外文本丢了假名 —— 透传路径被改坏");

  if (count(after, CJK) > 0 && count(after, KANA) === 0) {
    ok("接管前是日文、接管后整行中文，且逐字 span 结构不变");
  }
}

for (const h of HOOKS) runOne(h);

console.log("");
fs.rmSync(tmp, { recursive: true, force: true });
if (bad) { console.log(`✗ ${bad} 项不符合预期`); process.exit(1); }
console.log(`✓ TyranoScript v6 逐字正文管线回归通过（${HOOKS.length} 份 hook）`);
process.exit(0);
