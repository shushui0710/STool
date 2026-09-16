/*
 * 校验 MV/MZ 运行时汉化 Hook 的行为（内嵌在 stool-rs/src/features/inject.rs 的 HOOK_JS 里，
 * Rust 单测只能做源码断言，测不到真实替换行为，所以这里把 JS 抠出来在 Node 里跑一遍）。
 *
 * 覆盖两类核心回归：
 *  1) 资源名不能被动 —— 翻译表的「原文」常与游戏资源文件名同形（那个验证游戏里有 158 个撞名，
 *     如 `タイトル画面` → 标题画面）。旧实现只按 `looksLikePath` 判断，而 `タイトル画面`
 *     既无扩展名也无斜杠，于是 `$dataSystem.title1Name` 被整句替换成中文，
 *     游戏报 `Failed to load img/titles1/标题画面.png` 并卡死在标题画面。
 *  2) 片段替换要生效 —— 表里有上千条「片段键」（は防御の構えを取った！ / ダメージを受けた！），
 *     是给「角色名/数字 + 模板」拼出来的句子用的。只做整句匹配时这类句子整段漏掉。
 *
 * 用法：node scripts/verify_inject_hook.cjs
 *      STOOL_HOOK_JS=<另一个 inject.rs 或 .js> node scripts/verify_inject_hook.cjs   # 换源对照
 */
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const vm = require("vm");

// --- 1. 从 Rust 源码里抠出 HOOK_JS ---
// 可用 STOOL_HOOK_JS=<路径> 覆盖（指向另一个 inject.rs 或直接指向 .js），
// 用来对着历史版本跑一遍、确认这份校验真的能抓到那个 bug（而不是空过）。
const RUST_SRC = process.env.STOOL_HOOK_JS || path.resolve(__dirname, "..", "stool-rs", "src", "features", "inject.rs");

const src = fs.readFileSync(RUST_SRC, "utf8");
const m = src.match(/const HOOK_JS: &str = r#"([\s\S]*?)"#;/);
const HOOK = m ? m[1] : src;
if (!HOOK || HOOK.length < 1000) {
  console.error("✗ 没能从 " + RUST_SRC + " 里取出 HOOK_JS");
  process.exit(2);
}

// --- 2. 翻译表：故意含与资源名同形的原文 + 片段键 ---
const MAP = {
  // 与资源文件名撞名（必须「不」被替换）
  タイトル画面: "标题画面",
  "ガーデン・シティ_2": "花园城市_2",
  スチル1: "静态图像1",
  Grassland: "草原",
  Trees: "树木",
  Actor1: "演员1",
  Face1: "脸1",
  Battler1: "战斗图1",
  备注: "被改过",
  // 正常整句（必须被替换）
  天使の早漏治療クリニック: "天使的早泄治疗诊所",
  レベル: "等级",
  勇者: "勇士",
  こんにちは: "你好",
  はい: "是",
  いいえ: "否",
  村: "村庄",
  村人: "村民",
  テスト: "测试",
  新しいゲーム: "新游戏",
  "効果がないようだ…": "好像没有效果…",
  // 片段键：只有子串替换才生效
  "は防御の構えを取った！": "摆出了防御的架势！",
  "ダメージを受けた！": "受到了伤害！",
  クールタイム: "冷却时间",
  // 最长优先：セラR 必须赢过 セラ，否则会先吃掉「セラ」再撞上「R」
  セラ: "塞拉",
  セラR: "塞拉R",
  // 1 字键：必须被 MIN_FRAG 挡在子串替换之外（否则会把整段文本改烂）
  あ: "A"
};

// --- 3. 造一个最小 RPG Maker 运行环境 ---
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "stool-hook-"));
fs.writeFileSync(path.join(tmp, "stool_translate.json"), JSON.stringify(MAP), "utf8");

const g = globalThis;
g.window = g;
g.location = { pathname: encodeURI("/" + tmp.replace(/\\/g, "/") + "/index.html") };
// Hook 走的是 `require("fs")` 直读分支；但 require 在 CJS 里是模块作用域、不是全局，
// 不显式挂上去，vm 里就 `typeof require !== "function"` → 整张表读不到（校验会假通过）。
g.require = require;

const $dataSystem = {
  title1Name: "タイトル画面",
  title2Name: "",
  battleback1Name: "Grassland",
  battleback2Name: "Trees",
  titleBgm: { name: "ガーデン・シティ_2", volume: 90, pitch: 100, pan: 0 },
  gameTitle: "天使の早漏治療クリニック",
  currencyUnit: "G",
  terms: { basic: ["レベル", "HP"], messages: { actionFailure: "効果がないようだ…" } }
};
const $dataActors = [{
  id: 1, name: "勇者", nickname: "勇者", profile: "テスト",
  characterName: "Actor1", characterIndex: 0, faceName: "Face1",
  battlerName: "Battler1", note: "备注"
}];
const $dataItems = [{ id: 1, name: "新しいゲーム", description: "テスト", note: "<x:1>" }];
const $dataMap = {
  displayName: "村",
  parallaxName: "スチル1",
  events: [null, {
    id: 2, name: "EV002", note: "",
    pages: [{ list: [
      { code: 101, indent: 0, parameters: ["Face1", 0, 2, 0, "村人"] },      // [0]=头像名(资源) [4]=说话人(文本)
      { code: 401, indent: 0, parameters: ["こんにちは"] },                  // 正文
      { code: 231, indent: 0, parameters: [1, "スチル1", 0, 0, 0, 100, 100, 255, 0] }, // 显示图片
      { code: 241, indent: 0, parameters: [0, "ガーデン・シティ_2", 90, 100, 0] },     // 播放 BGM
      { code: 102, indent: 0, parameters: [["はい", "いいえ"], 0, 0, 2, 0] }, // 选项
      { code: 402, indent: 0, parameters: ["はい", 0] },                     // 当[选项]
      { code: 355, indent: 0, parameters: ['$gameScreen.showPicture(1, "スチル1", 0, 0, 0, 100, 100, 255, 0)'] },
      { code: 108, indent: 0, parameters: ["コメント"] }                      // 注释
    ] }]
  }]
};
const $dataCommonEvents = [{ id: 1, name: "CE1", list: [{ code: 401, indent: 0, parameters: ["こんにちは"] }] }];

g.$dataSystem = $dataSystem;
g.$dataActors = $dataActors;
g.$dataItems = $dataItems;
g.$dataMap = $dataMap;
g.$dataCommonEvents = $dataCommonEvents;

g.DataManager = {};
g.Scene_Boot = function () {};
g.Scene_Boot.prototype.start = function () {};
g.Scene_Map = function () {};
g.Scene_Map.prototype.start = function () {};
g.Window_Base = function () {};
g.Window_Base.prototype.drawText = function () { return arguments[0]; };
g.Window_Base.prototype.drawTextEx = function () { return arguments[0]; };
// DTextPicture / Text2Frame 这类插件把文字直接画进位图，不走 Window_Base
g.Bitmap = function () {};
g.Bitmap.prototype.drawText = function () { return arguments[0]; };

// --- 4. 跑 Hook，并触发数据库/地图/显示三层 ---
vm.runInThisContext(HOOK, { filename: "stool_translate.js" });
g.Scene_Boot.prototype.start.call({});
g.Scene_Map.prototype.start.call({});
const wDraw = (s) => g.Window_Base.prototype.drawText.call({}, s);
const bDraw = (s) => g.Bitmap.prototype.drawText.call({}, s);

// --- 5. 断言 ---
const ev = $dataMap.events[1].pages[0].list;
const cases = [
  // 资源名字段：必须原样不动（这就是标题画面那个 bug）
  [$dataSystem.title1Name, "タイトル画面", "$dataSystem.title1Name（标题画面图片名）"],
  [$dataSystem.battleback1Name, "Grassland", "$dataSystem.battleback1Name（战斗背景图名）"],
  [$dataSystem.titleBgm.name, "ガーデン・シティ_2", "$dataSystem.titleBgm.name（标题 BGM 文件名）"],
  [$dataActors[0].characterName, "Actor1", "$dataActors[0].characterName（行走图名）"],
  [$dataActors[0].faceName, "Face1", "$dataActors[0].faceName（头像名）"],
  [$dataActors[0].battlerName, "Battler1", "$dataActors[0].battlerName（战斗图名）"],
  [$dataActors[0].note, "备注", "$dataActors[0].note（备注）"],
  [$dataMap.parallaxName, "スチル1", "$dataMap.parallaxName（远景图名）"],
  [ev[2].parameters[1], "スチル1", "指令 231 显示图片的文件名"],
  [ev[3].parameters[1], "ガーデン・シティ_2", "指令 241 播放 BGM 的文件名"],
  [ev[6].parameters[0], '$gameScreen.showPicture(1, "スチル1", 0, 0, 0, 100, 100, 255, 0)', "指令 355 脚本原文"],
  [ev[7].parameters[0], "コメント", "指令 108 注释"],
  [ev[0].parameters[0], "Face1", "指令 101 的头像文件名参数"],
  // 正常文本：必须被替换
  [$dataSystem.gameTitle, "天使的早泄治疗诊所", "$dataSystem.gameTitle"],
  [$dataSystem.terms.basic[0], "等级", "$dataSystem.terms.basic[0]"],
  [$dataSystem.terms.messages.actionFailure, "好像没有效果…", "$dataSystem.terms.messages"],
  [$dataActors[0].name, "勇士", "$dataActors[0].name（角色名）"],
  [$dataActors[0].profile, "测试", "$dataActors[0].profile"],
  [$dataItems[0].name, "新游戏", "$dataItems[0].name"],
  [$dataMap.displayName, "村庄", "$dataMap.displayName"],
  [ev[0].parameters[4], "村民", "指令 101 的说话人姓名参数"],
  [ev[1].parameters[0], "你好", "指令 401 正文"],
  [ev[4].parameters[0].join("/"), "是/否", "指令 102 选项文本"],
  [ev[5].parameters[0], "是", "指令 402 分支标题"],
  [$dataCommonEvents[0].list[0].parameters[0], "你好", "公共事件里的 401 正文"],
  [wDraw("新しいゲーム"), "新游戏", "Window_Base.drawText 显示层兜底"],
  // 子串兜底：整句没命中 → 最长片段替换
  [wDraw("SHUは防御の構えを取った！"), "SHU摆出了防御的架势！", "子串: 角色名 + 模板"],
  [bDraw("14ダメージを受けた！"), "14受到了伤害！", "Bitmap.drawText: 数字 + 模板"],
  [wDraw("【クールタイム】1"), "【冷却时间】1", "子串: 被括号包裹"],
  [wDraw("セラR&L"), "塞拉R&L", "子串: 最长优先（セラR 必须赢过 セラ）"],
  [wDraw("あああ"), "あああ", "MIN_FRAG: 1 字键不参与子串替换"],
  [wDraw("こんにちは"), "你好", "整句优先于子串（整句命中直接用）"]
];

let bad = 0;
for (const [got, want, label] of cases) {
  if (got !== want) {
    bad++;
    console.log("✗ " + label + "\n    期望: " + JSON.stringify(want) + "\n    实际: " + JSON.stringify(got));
  }
}
console.log("");
console.log(bad === 0
  ? "✓ Hook 行为校验通过（" + cases.length + " 项）：资源名/脚本/注释未被动，整句与片段替换均按预期生效"
  : "✗ " + bad + " / " + cases.length + " 项不符合预期");
fs.rmSync(tmp, { recursive: true, force: true });
process.exit(bad === 0 ? 0 : 1);
