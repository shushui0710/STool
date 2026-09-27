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
    foldHtml: (title, body) => `<fold t="${String(title)}">${String(body)}</fold>`,
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

  // ---------- 5. unlock.js：按名写值必须透传到 unlock_run ----------
  //
  // 背景：2026-09 给「解锁全 CG」加了 `--opt:set=` 的界面入口（只写点名的键，不猜）。
  // 这类「界面填了、命令没传」的漏接是**静默失效**：界面照常跑、报告照常出，
  // 只是用户填的键一个都没写进去 —— 只有真跑一遍逻辑才看得出来。
  {
    const { page, calls, sandbox } = loadPage("pages/unlock.js", "UnlockPage");
    sandbox.__reply = async (cmd) =>
      cmd === "unlock_run" ? { ok: true, data: { success: true, files_done: 3 } } : { ok: true, data: null };
    const inst = Object.assign(Object.create(page), {
      root: {},
      render: async () => {},
      subscribe: async () => {},
      loadBackups: async () => {},
      running: false,
      apply: false,
      route: "unity",
      saveDir: "",
      filter: "",
      set: "GameState_Follower=200,GameState_Money=999999",
      result: null,
      err: "",
      prog: null,
    });
    const html = inst.advancedHtml();
    check("unlock.js 进阶里有「按名写指定值」输入框（id=uSet）", () => {
      ok(html.includes('id="uSet"'), "缺少 id=uSet 的输入框");
      ok(html.includes("按名写指定值"), "缺少说明文案");
    });
    await inst.run();
    const c = calls.find((x) => x[0] === "unlock_run");
    check("unlock.js run 把 set 透传给 unlock_run", () => {
      ok(!!c, "根本没调 unlock_run");
      ok(
        c[1].set === "GameState_Follower=200,GameState_Money=999999",
        `set 被吞了或被改：${JSON.stringify(c[1].set)}`
      );
      ok(c[1].filter === "" && c[1].apply === false, "其它字段被串味了");
    });
  }

  // ---------- 6. unlock.js：必须能把「有哪些键可改」列出来 ----------
  //
  // 背景：`--opt:set=` 要求用户报键名，可用户不知道有哪些键 —— 于是加了只读的
  // 「键列表」。这里钉两件事：①列表真的画出来了、引擎自身的键被单独归一组；
  // ②点一行真的把键名填进 #uSet（只填名+`=`，值留空）。后者最易漏 ——
  // 事件绑了但 dataset 取错，点下去毫无反应。
  {
    const { page, calls, sandbox, toasts } = loadPage("pages/unlock.js", "UnlockPage");
    sandbox.__reply = async (cmd) => (cmd === "unlock_list_keys" ? { ok: true, data: null } : { ok: true, data: null });
    const inst = Object.assign(Object.create(page), {
      root: {},
      render: async () => {},
      running: false,
      showKeys: true,
      keysLoaded: true,
      keys: [
        { name: "GameState_Money", value: '"6108"', kind: "二进制", ty: 3, family: "GameState", engine_own: false },
        { name: "Skill_MaidenLevel", value: "3", kind: "DWORD", ty: 4, family: "Skill", engine_own: false },
        { name: "unity.ready", value: "1", kind: "DWORD", ty: 4, family: "unity", engine_own: true },
      ],
      plan: { routes: [{ key: "registry_keys", label: "改注册表键位" }] },
      set: "",
    });

    const html = inst.keysBlockHtml();
    check("unlock.js 键列表画出来了，且引擎自身的键单独归一组", () => {
      ok(html.includes("GameState_Money"), "缺游戏键 GameState_Money");
      ok(html.includes('data-key="GameState_Money"'), "游戏键没带 data-key（点了没反应）");
      ok(html.includes("引擎自身的设置"), "缺「引擎自身的设置」分组标题");
      ok(html.indexOf("GameState_Money") < html.indexOf("引擎自身的设置"), "游戏键应排在引擎键之前");
    });

    check("unlock.js 非注册表路线时不显示键列表（别白跑一趟）", () => {
      const p2 = Object.assign(Object.create(page), { plan: { routes: [{ key: "copy_save" }] } });
      ok(p2.keysBlockHtml() === "", "非注册表路线却画了键列表");
    });

    // 点行填值：给它一个假 input，看名字有没有真的写进去（且只写名字）
    const fakeBox = { value: "", focus() {}, setSelectionRange() {} };
    sandbox.$ = (sel) => (sel === "#uSet" ? fakeBox : null);
    inst.pickKey("GameState_Money");
    check("unlock.js 点键名把「名称=」填进 #uSet，不替用户猜值", () => {
      ok(fakeBox.value === "GameState_Money=", `填进去的是 ${JSON.stringify(fakeBox.value)}`);
      ok(inst.set === "GameState_Money=", "实例上的 set 没同步");
      ok(!/=\d/.test(fakeBox.value), "不该自动填值（替他猜数就是瞎写注册表）");
    });
    // 连点第二个键：接着追加，不覆盖
    inst.pickKey("Skill_MaidenLevel");
    check("unlock.js 连点多个键是追加、不覆盖", () => {
      ok(
        fakeBox.value === "GameState_Money=,Skill_MaidenLevel=",
        `连点后变成 ${JSON.stringify(fakeBox.value)}`
      );
    });
    // 重复点同一个键：不重复加
    inst.pickKey("GameState_Money");
    check("unlock.js 重复点同一个键不重复追加", () => {
      ok(fakeBox.value === "GameState_Money=,Skill_MaidenLevel=", "重复追加了");
      ok(toasts.some((t) => String(t[0]).includes("已经在框里")), "没提示「已经在框里」");
    });

    check("unlock.js 自带存档块说明了「可能只有设置文件」", () => {
      // `C0771存档` 实录：包里只有 System.bin（音量/跳过），CG 标记在 data*.bin。
      // 若界面只说「最省事的一条路」，用户跑完没解锁会以为工具坏了。
      const { page } = loadPage("pages/unlock.js", "UnlockPage");
      page.plan = {
        engine: "unity",
        route: "bundled",
        bundled: ["C0771存档\\C0771存档\\存档\\System.bin"],
        save_dirs: ["C:\\Users\\x\\AppData\\LocalLow\\X\\KKC4"],
        routes: [],
      };
      const html = page.discoveryHtml();
      ok(html.includes("设置文件"), "没提「设置文件」");
      ok(html.includes("进度存档"), "没提「进度存档」");
      ok(html.includes("注册表"), "没给替代出路");
      ok(!html.includes("这是最省事的一条路"), "还在做过度承诺");
    });
  }

  // ---------- 7. runtime.js：内核返回的多行文案必须保住换行 ----------
  //
  // 背景（2026-09-27）：「方式一 · 启动并连接」失败时，内核会返回**三行**归因
  // （`runtime::connect_failure_hint`：`连接失败…\n原因：…NW.js ≥0.70…\n出路：改用存档编辑`）。
  // 这段文案原本渲染在 `<div class="small …">` 里，而 `.small` **只设字号**
  // （`app.css:312`），没有 `white-space: pre-wrap` → 三行挤成一段。
  // 后果不是「没提示」，而是**用户读不到「换姿势也没用」这句**，于是把同一件事重试到死。
  //
  // 判据两条：①必须落在 `.note`（`app.css:585` 带 pre-wrap）而不是 `.small`；
  //           ②模板里原样的 `\n` 要还在（别顺手被压成空格）。
  {
    const { page } = loadPage("pages/runtime.js", "RuntimePage");
    const base = {
      mv: { applicable: true, port_open: false, note: "端口没开" },
      mvState: null,
      mvBusy: false,
    };
    const inst = Object.assign(Object.create(page), {
      ...base,
      mvMsg:
        "连接调试端口 7654 失败：连接 127.0.0.1:7654 失败: 由于目标计算机积极拒绝，无法连接。 (os error 10061)\n" +
        "原因：这个游戏自带 NW.js 0.76.1，而 NW.js 从 0.70 起就不再打开调试端口（上游缺陷 nwjs/nw.js#8191）。\n" +
        "出路：改用「存档编辑」。",
      mvMsgErr: true,
    });
    const html = inst.mvCardHtml();
    check("runtime.js 方式一的失败文案走 .note（保住换行）", () => {
      ok(/class="note err/.test(html), '失败文案没进 .note.err —— .small 没有 pre-wrap，三行会被挤成一段');
      ok(!/class="small[^"]*"[^>]*>\s*连接调试端口/.test(html), "失败文案还在 .small 里");
      ok(html.includes("原因：这个游戏自带 NW.js 0.76.1"), "文案没渲染出来");
      ok(html.includes("\n原因："), "换行被吞了（应是原样 \\n，不是空格）");
      ok(html.includes("\n出路："), "「出路」那一行没保住换行");
    });
    check("runtime.js 普通状态不标红（xxErr 要成对复位）", () => {
      const okInst = Object.assign(Object.create(page), {
        ...base,
        mvMsg: "已连上，开始改吧。",
        mvMsgErr: false,
      });
      const h2 = okInst.mvCardHtml();
      ok(!/class="note err/.test(h2), "正常状态被标成了错误红");
      ok(/class="note info/.test(h2), "正常状态应走 .note.info");
    });
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
