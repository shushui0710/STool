/*
 * 三份运行时汉化 hook 的语法校验（~50ms，不需要 cargo）。
 *
 * 为什么要这个：hook 原先以 `const HOOK_JS: &str = r#"..."#` 的形式内嵌在
 * stool-rs/src/features/inject.rs 里 —— 那玩意儿**不能 lint、不能单独跑**，
 * 改一个字符就得重建整个 exe 才知道有没有把 JS 写坏。
 * 2026-09 已抽成真文件 stool-rs/assets/hooks/{mv_mz,html,tyrano}.js（inject.rs 用
 * include_str! 内嵌）。于是有了这条最快的反馈回路：
 *
 *     node scripts/check_hooks.cjs      # 改完 hook 立刻验语法
 *
 * 门禁 `scripts/gate.ps1` 也会跑它（放在 clippy 之前 —— 语法错 50ms 就该停，
 * 不必等 3 分钟的 clippy）。
 *
 * 用法：node scripts/check_hooks.cjs [文件...]   # 默认校验全部三份
 */
"use strict";

const { execFileSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const DIR = path.resolve(__dirname, "..", "stool-rs", "assets", "hooks");
const ALL = ["mv_mz.js", "html.js", "tyrano.js"];
const targets = process.argv.slice(2).length ? process.argv.slice(2) : ALL;

let bad = 0;
for (const f of targets) {
  const p = path.isAbsolute(f) ? f : path.join(DIR, f);
  if (!fs.existsSync(p)) {
    console.log(`\u2717 ${f}  不存在：${p}`);
    bad++;
    continue;
  }
  try {
    // 用 node 自己的解析器，跟运行时口径一致（比 vm.Script 更贴近真实 `node x.js`）
    execFileSync(process.execPath, ["--check", p], { stdio: ["ignore", "ignore", "pipe"] });
    console.log(`\u2713 ${f}  ${String(fs.statSync(p).size).padStart(6)} B  语法合法`);
  } catch (e) {
    bad++;
    const err = (e.stderr ? e.stderr.toString() : String(e)).trim();
    console.log(`\u2717 ${f}  语法错误：\n${err.split("\n").map((l) => "    " + l).join("\n")}`);
  }
}

console.log("");
if (bad) {
  console.log(`\u2717 ${bad}/${targets.length} 份 hook 未通过语法校验`);
  process.exit(1);
}
console.log(`\u2713 ${targets.length} 份 hook 语法全部合法`);
