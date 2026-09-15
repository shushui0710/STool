/* ---------------------------------------------------------------------------
   外壳：左侧任务导航 + 路由 + 主题 + 顶栏。

   导航按**任务**分组（上手 / 深入），不按技术机制（封包 / 注入）——
   这是本次重构针对「不知道点哪个」这条根问题的直接改动
   （见 docs/TAURI重构方案.md §1、§4.1）。
--------------------------------------------------------------------------- */

/** 导航分组。顺序 = 建议的上手顺序，但不是强制流程（各页都能独立进）。 */
const NAV_GROUPS = [
  { title: "上手", ids: ["detect", "extract", "preview", "text"] },
  { title: "深入", ids: ["save", "runtime", "unlock", "mods"] },
];

/** 每页的标题/副标题/导航提示。`impl` 为空 = 还没搬过来（走占位页）。 */
const PAGE_META = {
  detect: { title: "选游戏", sub: "告诉 STool 你要处理哪个游戏。认出来之后，后面每一步都会自动带上它。", hint: "游戏在哪、是什么引擎", impl: DetectPage },
  extract: { title: "取出素材", sub: "把游戏里的图片、音乐、脚本拿出来，存成普通文件。", hint: "从打包文件里取出东西", impl: ExtractPage },
  preview: { title: "看素材", sub: "直接浏览已经拿出来的图片、音乐和文本。", hint: "看图、听声、翻文本", impl: PreviewPage },
  text: { title: "翻译文字", sub: "把游戏里的台词提成表格，翻译完再塞回游戏。", hint: "汉化的主流程", impl: TextPage },
  save: { title: "改存档", sub: "改金币、等级、道具数量。改完点「写回存档」，原文件会先自动备份。", hint: "改存档里的数值", impl: SavePage },
  runtime: { title: "游戏里改数值", sub: "游戏开着也能改，改完立刻生效，不用退出重开。", hint: "直接改运行中的内存", impl: RuntimePage },
  unlock: { title: "解锁全CG", sub: "一键打开所有回想与画廊，不用一格格解锁。", hint: "不想一格格自己解锁", impl: UnlockPage },
  mods: { title: "装MOD", sub: "安装、停用玩家做的补丁。原文件会自动备份，随时能还原。", hint: "装别人做的改动", impl: ModsPage },
  tools: { title: "工具箱", sub: "设置、诊断、帮助 —— 平时用不到，出问题才来。", hint: "出问题才用", impl: ToolsPage },
};

// 先把「哪些页真接了」记下来，再给其余的补占位实现 ——
// 否则占位页会覆盖掉本来就写好的页面，导航也分不清真假。
const REAL_PAGES = new Set(Object.keys(PAGE_META).filter((id) => PAGE_META[id].impl));

for (const id of Object.keys(PAGE_META)) {
  if (!PAGE_META[id].impl) PAGE_META[id].impl = makeSoonPage(id);
}

let currentId = "";
let currentPage = null;

// ---------------------------------------------------------------------------
// 导航
// ---------------------------------------------------------------------------

function renderNav() {
  const group = (title, ids) => `
    <div class="nav-group">
      <div class="nav-group-title">${esc(title)}</div>
      ${ids
        .map((id) => {
          const m = PAGE_META[id];
          const soon = !REAL_PAGES.has(id);
          return `<button class="nav-item${soon ? " soon" : ""}" data-page="${id}">
            <span class="nav-item-title">${esc(m.title)}</span>
            <span class="nav-item-hint">${esc(m.hint || "")}</span>
          </button>`;
        })
        .join("")}
    </div>`;

  $("#nav").innerHTML =
    NAV_GROUPS.map((g) => group(g.title, g.ids)).join("") + group("其他", ["tools"]);

  $("#nav")
    .querySelectorAll("[data-page]")
    .forEach((b) => b.addEventListener("click", () => (location.hash = "#" + b.dataset.page)));
}

function markNav(id) {
  $("#nav")
    .querySelectorAll("[data-page]")
    .forEach((b) => b.classList.toggle("on", b.dataset.page === id));
}

/** 顶栏右侧按钮 + 左下「当前游戏」chip。页面状态变化后调它。 */
function refreshChrome() {
  const chip = $("#gameChip");
  if (Store.gameRoot) {
    const name = String(Store.gameRoot).split(/[\\/]/).filter(Boolean).pop() || Store.gameRoot;
    $("#gameChipName").textContent = name;
    $("#gameChipName").title = Store.gameRoot;
    chip.hidden = false;
  } else {
    chip.hidden = true;
  }

  if (currentPage && typeof currentPage.actions === "function") {
    $("#topActions").innerHTML = currentPage.actions() || "";
    if (typeof currentPage.bindActions === "function") currentPage.bindActions();
  } else {
    $("#topActions").innerHTML = "";
  }
}

// ---------------------------------------------------------------------------
// 路由
// ---------------------------------------------------------------------------

async function setPage(id) {
  if (!PAGE_META[id]) id = "detect";
  const meta = PAGE_META[id];

  if (currentPage && typeof currentPage.unmount === "function") currentPage.unmount();

  currentId = id;
  currentPage = meta.impl;
  markNav(id);

  $("#pageTitle").textContent = currentPage.title || meta.title;
  $("#pageSub").textContent = currentPage.sub || meta.sub || "";
  $("#topActions").innerHTML = "";

  const host = $("#content");
  host.innerHTML = "";
  await currentPage.mount(host);
  refreshChrome();
}

// ---------------------------------------------------------------------------
// 主题
// ---------------------------------------------------------------------------

function applyTheme(t) {
  document.documentElement.dataset.theme = t;
  const b = $("#themeBtn");
  if (b) b.textContent = t === "dark" ? "浅色模式" : "深色模式";
  try {
    localStorage.setItem("stool.theme", t);
  } catch (_) {}
}

// ---------------------------------------------------------------------------
// 启动
// ---------------------------------------------------------------------------

function log(s) {
  Tauri.call("log_line", { s: "app.js " + s });
}

window.addEventListener("error", (e) => {
  log(`前端异常: ${e.message} @${e.filename}:${e.lineno}`);
});
window.addEventListener("unhandledrejection", (e) => {
  log(`未处理的 Promise 失败: ${e.reason && e.reason.message ? e.reason.message : e.reason}`);
});

(async function boot() {
  renderNav();

  let theme = "light";
  try {
    theme = localStorage.getItem("stool.theme") || "light";
  } catch (_) {}
  applyTheme(theme);

  $("#themeBtn").addEventListener("click", () => {
    applyTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark");
  });

  $("#gameChip").addEventListener("click", () => {
    location.hash = "#detect";
  });

  window.addEventListener("hashchange", () => {
    const id = location.hash.replace(/^#/, "") || "detect";
    if (id !== currentId) setPage(id);
  });

  const info = await Tauri.call("app_info");
  if (info.ok) {
    $("#ver").textContent = `界面 ${info.data.version} · 内核 ${info.data.core_version}`;
  } else {
    $("#ver").textContent = "未连上内核";
  }

  log(`启动完成；__TAURI__=${window.__TAURI__ ? "ok" : "missing"}`);

  // 起始页：URL hash 优先，其次 STOOL_PAGE（与 egui 版同一口径），最后「选游戏」
  const envPage = await Tauri.call("env_page");
  const id = location.hash.replace(/^#/, "") || (envPage.ok && envPage.data ? envPage.data : "detect");
  await setPage(id);

  log(`页面已挂载: ${id} / 标题="${$("#pageTitle").textContent}"`);
})();
