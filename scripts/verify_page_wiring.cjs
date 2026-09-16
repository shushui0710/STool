/* ---------------------------------------------------------------------------
   前端页面逻辑的桩测试（不拉 WebView，用 Node 的 vm 直接跑页面对象）。

   为什么需要它：`stool-tauri/src/js/pages/*.js` 的改动没法用 Rust 单测覆盖，
   而这类页面的 bug 又特别典型 —— **点了没反应**（命令名写错、把 Tauri.call 的
   包装对象当数据、预演失败却显示「已通过」）。这些只有真跑一遍逻辑才抓得住。

   用法：
     node D:/STool/scripts/verify_page_wiring.cjs
   （PowerShell 里 stdout 抓不到时：... 2>&1 | Out-File <日志> 再读）

   覆盖的回归点（2026-09 修的两个真 bug）：
     1. text.js  「打开表格去翻译」必须调 open_file，不能是 shell_open（后者不是命令）
     2. mods.js  选目录后 srcDir 必须是**路径字符串**，不能是 {ok,data} 包装对象
     3. mods.js  预演失败时必须显示「没能预演冲突」，不能显示「✔ 检查过了」
     4. extract.js render() 在某个 id 缺失时不能抛异常（否则后续绑定整块被跳过）
--------------------------------------------------------------------------- */

const fs = require("fs");
const path = require("path");
const vm = require("vm");

const SRC = path.join(__dirname, "..", "stool-tauri", "src", "js");

/** 在沙箱里加载一个页面文件，返回 { page, calls, toasts, sandbox }。 */
function loadPage(rel, exportName) {
  const code =
    fs.readFileSync(path.join(SRC, rel), "utf8") + `\n;globalThis.__PAGE = ${exportName};\n`;
  const calls = [];
  const toasts = [];
  const sandbox = {
    console,
    esc: (s) => String(s === undefined || s === null ? "" : s),
    shortPath: (s) => String(s === undefined || s === null ? "" : s),
    noteHtml: (s) => `<note>${String(s)}</note>`,
    emptyHtml: () => "<empty/>",
    clip: (s) => String(s),
    log: () => {},
    toast: (m, k) => toasts.push([m, k]),
    $: () => null,
    Store: { gameRoot: "D:\\g", detect: { engine_name: "test" } },
    Tauri: {
      call: async (cmd, args) => {
        calls.push([cmd, args]);
        return sandbox.__reply(cmd, args);
      },
    },
    __reply: async () => ({ ok: true, data: null }),
  };
  sandbox.globalThis = sandbox;
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(code, sandbox);
  if (!sandbox.__PAGE) throw new Error(`${rel} 里没找到 ${exportName}`);
  return { page: sandbox.__PAGE, calls, toasts, sandbox };
}

const results = [];
function check(name, fn) {
  try {
    fn();
    results.push(["PASS", name, ""]);
  } catch (e) {
    results.push(["FAIL", name, e && e.message ? e.message : String(e)]);
  }
}
function ok(cond, msg) {
  if (!cond) throw new Error(msg);
}

(async () => {
  // ---------- 1. text.js：命令名必须是 open_file ----------
  {
    const { page, calls } = loadPage("pages/text.js", "TextPage");
    const inst = Object.create(page);
    inst.csvPath = "D:\\out\\text.csv";
    inst.toastMsg = "";
    await inst.openCsv();
    check("text.js openCsv 调用已注册的 open_file", () => {
      const cmds = calls.map((c) => c[0]);
      ok(cmds.length === 1, `期望 1 次调用，实际 ${cmds.length}`);
      ok(cmds[0] === "open_file", `期望 open_file，实际 ${cmds[0]}`);
      ok(!cmds.includes("shell_open"), "不应再出现 shell_open");
      ok(calls[0][1] && calls[0][1].path === "D:\\out\\text.csv", "应把 csv 路径传过去");
    });
    // 没有 csvPath 时静默返回，不调用
    const inst2 = Object.create(page);
    inst2.csvPath = "";
    const before = calls.length;
    await inst2.openCsv();
    check("text.js openCsv 没有 csv 时不动", () => ok(calls.length === before, "不该发调用"));
  }

  // ---------- 2. mods.js：pickDir 必须解包出路径字符串 ----------
  {
    const { page, calls, sandbox, toasts } = loadPage("pages/mods.js", "ModsPage");
    sandbox.__reply = async (cmd) => {
      if (cmd === "pick_folder") return { ok: true, data: "D:\\fake\\patch" };
      if (cmd === "mods_preview") return { ok: true, data: { conflict_count: 0, conflicts: [] } };
      return { ok: true, data: null };
    };
    const inst = Object.assign(Object.create(page), {
      root: {},
      render: async () => {},
      preview: null,
      previewErr: "",
      name: "",
      previewing: false,
      busy: false,
      force: false,
      state: { mods: [], conflicts: [], conflict_count: 0, store_dir: "D:\\s" },
    });
    await inst.pickDir();
    check("mods.js pickDir 把 srcDir 存成路径字符串", () => {
      ok(typeof inst.srcDir === "string", `期望 string，实际 ${typeof inst.srcDir}`);
      ok(inst.srcDir === "D:\\fake\\patch", `内容不对：${inst.srcDir}`);
    });
    check("mods.js pickDir 之后 srcDir 不是包装对象", () => {
      ok(!(inst.srcDir && inst.srcDir.ok !== undefined), "srcDir 里混进了 {ok,data}");
    });
    check("mods.js 预演成功且无冲突时显示「检查过了」", () => {
      inst.previewing = false;
      ok(inst.installCardHtml().includes("检查过了"), "应显示检查通过");
    });
    // 取消选择（返回 null）时不应改 srcDir
    sandbox.__reply = async (cmd) =>
      cmd === "pick_folder" ? { ok: true, data: null } : { ok: true, data: null };
    const inst3 = Object.assign(Object.create(page), {
      root: {},
      render: async () => {},
      srcDir: "D:\\keep",
      previewErr: "",
      preview: null,
      name: "",
      previewing: false,
    });
    await inst3.pickDir();
    check("mods.js 取消选择目录时保留原 srcDir", () =>
      ok(inst3.srcDir === "D:\\keep", `被改成了 ${inst3.srcDir}`));

    // 选择失败（pick_folder 报错）要弹错
    sandbox.__reply = async () => ({ ok: false, err: "对话框打不开" });
    toasts.length = 0;
    const inst4 = Object.assign(Object.create(page), {
      root: {},
      render: async () => {},
      srcDir: "",
      previewErr: "",
      preview: null,
      name: "",
      previewing: false,
    });
    await inst4.pickDir();
    check("mods.js 选目录失败时弹错且不改 srcDir", () => {
      ok(inst4.srcDir === "", "不该写入 srcDir");
      ok(toasts.length === 1 && String(toasts[0][1]) === "err", "应有一个 err toast");
    });
  }

  // ---------- 3. mods.js：预演失败不能说「检查过了」 ----------
  {
    const { page, sandbox } = loadPage("pages/mods.js", "ModsPage");
    sandbox.__reply = async (cmd) =>
      cmd === "mods_preview" ? { ok: false, err: "这个目录里没有文件" } : { ok: true, data: null };
    const inst = Object.assign(Object.create(page), {
      root: {},
      render: async () => {},
      srcDir: "D:\\fake\\patch",
      preview: null,
      previewErr: "",
      previewing: false,
      busy: false,
      force: false,
      name: "",
    });
    await inst.runPreview();
    check("mods.js 预演失败时记下原因", () =>
      ok(String(inst.previewErr).includes("没有文件"), `previewErr=${inst.previewErr}`));
    const html = inst.installCardHtml();
    check("mods.js 预演失败时不显示「检查过了」（假保证）", () =>
      ok(!html.includes("检查过了"), "出现了假保证文案"));
    check("mods.js 预演失败时显示「没能预演冲突」", () =>
      ok(html.includes("没能预演冲突"), "缺少提示"));
  }

  // ---------- 4. extract.js：id 缺失时 render 不能抛 ----------
  {
    const { page, sandbox } = loadPage("pages/extract.js", "ExtractPage");
    sandbox.mountBackups = () => {};
    sandbox.$ = () => null; // 所有 id 都取不到 —— 老代码在这里会 TypeError
    const inst = Object.assign(Object.create(page), {
      root: { innerHTML: "" },
      outDir: "D:\\o",
      running: "",
      result: null,
      err: "",
      lastKind: "",
      repackAck: false,
      root2: null,
      progressHtml: () => "",
      advancedHtml: () => "",
    });
    let threw = null;
    try {
      await inst.render();
    } catch (e) {
      threw = e;
    }
    check("extract.js render 在 id 全缺失时不抛异常", () =>
      ok(threw === null, `抛了：${threw && threw.message}`));
  }

  // ---------- 输出 ----------
  const bad = results.filter((r) => r[0] === "FAIL");
  const out = [];
  out.push("=== 前端页面桩测试 ===");
  for (const [st, name, msg] of results) out.push(`  ${st}  ${name}${msg ? "  :: " + msg : ""}`);
  out.push("");
  out.push(`总计 ${results.length} 条，失败 ${bad.length} 条`);
  fs.writeFileSync(path.join(__dirname, "..", "verify", "scratch_2026-09-16", "page_wiring_result.txt"), out.join("\n"), "utf8");
  console.log(out.join("\n"));
  process.exit(bad.length ? 1 : 0);
})();
