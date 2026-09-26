/* ---------------------------------------------------------------------------
   通用 UI 小工具。只放「每个页面都要用」的东西，放不下就说明该拆组件了。
--------------------------------------------------------------------------- */

const $ = (sel, root = document) => root.querySelector(sel);

/** HTML 转义。凡是把外部数据（路径、存档值、游戏名）拼进 innerHTML 的地方都必须过一遍。 */
function esc(s) {
  return String(s === null || s === undefined ? "" : s).replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]
  );
}

/** 由 HTML 字符串造元素（取第一个元素节点）。 */
function el(html) {
  const t = document.createElement("template");
  t.innerHTML = html.trim();
  return t.content.firstElementChild;
}

/** 给按钮加「转圈 + 禁用」的忙碌态，返回复原函数。 */
function busy(btn, text) {
  const old = btn.innerHTML;
  btn.disabled = true;
  btn.innerHTML = `<span class="spin"></span>${esc(text || "处理中")}`;
  return () => {
    btn.disabled = false;
    btn.innerHTML = old;
  };
}

/** 右下角提示条。kind: "err" | "ok" */
function toast(msg, kind = "ok") {
  const t = el(`<div class="toast ${kind === "err" ? "err" : "ok"}">${esc(msg)}</div>`);
  $("#toasts").appendChild(t);
  setTimeout(() => t.remove(), kind === "err" ? 8000 : 4000);
}

/** 统一渲染「原因 + 修法」错误条（Rust 侧返回的文本本身就带换行）。 */
function noteHtml(msg, kind = "err") {
  return `<div class="note ${kind}">${esc(msg)}</div>`;
}

/** 空状态：一定带一个动作按钮，不留死胡同。 */
function emptyHtml(icon, title, note, btnLabel, btnId) {
  const btn = btnLabel ? `<button class="btn btn-ghost" id="${btnId}">${esc(btnLabel)}</button>` : "";
  return `<div class="empty">
    <div class="empty-icon">${esc(icon)}</div>
    <div class="empty-title">${esc(title)}</div>
    <div class="empty-note">${esc(note)}</div>
    ${btn}
  </div>`;
}

/** 折叠块。 */
function foldHtml(summary, bodyHtml, open = false) {
  return `<details class="fold"${open ? " open" : ""}>
    <summary>${esc(summary)}</summary>
    <div class="fold-body">${bodyHtml}</div>
  </details>`;
}

/** 文本截断（中英混排，按字符）。 */
function clip(s, n) {
  const t = String(s === null || s === undefined ? "" : s);
  return t.length <= n ? t : t.slice(0, n) + "…";
}

/** 路径太长时只留尾部（前面用省略号），便于在窄处显示。 */
function shortPath(p) {
  const s = String(p || "");
  return s.length <= 46 ? s : "…" + s.slice(-45);
}

/** 取一个文件路径所在的目录（备份列表是按目录找的）。Windows 的反斜杠也认。 */
function dirOf(p) {
  const s = String(p || "");
  const i = Math.max(s.lastIndexOf("\\"), s.lastIndexOf("/"));
  return i > 0 ? s.slice(0, i) : s;
}

/** 拼接路径（前端只管显示，真正的拼接在内核）。 */
function joinPath(dir, name) {
  const d = String(dir || "").replace(/[\\\/]+$/, "");
  return d ? d + "\\" + String(name || "") : String(name || "");
}

/* ---------------------------------------------------------------------------
   长任务进度条。抽包 / 提文本 / 解锁 三个页面用的是同一套（原本各抄一份），
   只在「容器 id」和「要不要取消按钮」上有差别。
--------------------------------------------------------------------------- */

/**
 * 进度条 HTML。
 *
 * @param hostId    容器 id（`paintProgress` 按它找节点）
 * @param cancelId  取消按钮 id；不传则不给取消按钮（例如解锁那次不打算中断）
 */
function progressHtml(hostId, cancelId) {
  const cancel = cancelId
    ? `<div class="row mt2"><button class="btn btn-ghost btn-sm" id="${cancelId}">取消</button></div>`
    : "";
  return `
    <div class="mt3" id="${hostId}">
      <div class="row tight">
        <span class="small dim2" data-msg style="flex:1 1 auto;min-width:0">正在准备…</span>
        <span class="small mono" data-pct>0%</span>
      </div>
      <div class="bar mt2"><i style="width:0%"></i></div>
      ${cancel}
    </div>`;
}

/** 进度事件到了只更新几个节点，不重绘整页（重绘会闪、也会丢焦点）。 */
function paintProgress(root, hostId, p) {
  const host = $("#" + hostId, root);
  if (!host || !host.firstElementChild) return;
  const pct = Math.max(0, Math.min(100, Math.round((p.frac || 0) * 100)));
  const bar = host.querySelector(".bar > i");
  if (bar) bar.style.width = pct + "%";
  const pv = host.querySelector("[data-pct]");
  if (pv) pv.textContent = pct + "%";
  const mv = host.querySelector("[data-msg]");
  if (mv) mv.textContent = clip(p.msg || "…", 64);
}

/**
 * 订阅后端 `op:progress` 事件。
 *
 * @param page  页面对象（`unlisten` 存在它身上，重进页面时能解绑 —— 不解绑会叠订阅）
 * @param root  页面根节点
 * @param hostId 进度条容器 id
 */
async function subscribeProgress(page, root, hostId) {
  if (page.unlisten) {
    try {
      page.unlisten();
    } catch (_) {}
    page.unlisten = null;
  }
  page.unlisten = await Tauri.on("op:progress", (p) => paintProgress(root, hostId, p));
}

/** 请求取消当前长任务。后端会在当前文件写完后停下。 */
async function cancelTask() {
  await Tauri.call("cancel_task");
  toast("已请求取消，会在当前文件写完后停下。");
}

/** 用系统默认程序打开一个路径（文件或文件夹），失败弹错。 */
async function openPath(p) {
  const r = await Tauri.call("open_folder", { path: p });
  if (!r.ok) toast(r.err, "err");
}

/**
 * 「还没选游戏」的整页空态 —— 8 个页面原本各写一份一样的样板。
 *
 * @param root 页面根节点
 * @param icon/title/note 空态文案
 * @param btnId 「去选游戏」按钮的 id（本函数会绑好跳转）
 */
function renderNeedGame(root, icon, title, note, btnId) {
  root.innerHTML = emptyHtml(icon, title, note, "去选游戏", btnId);
  const b = $("#" + btnId, root);
  if (b) b.addEventListener("click", () => (location.hash = "#detect"));
}

/* ---------------------------------------------------------------------------
   备份与还原。存档页（写回存档 → 改坏存档）和解包页（重新打包 → 改坏封包）
   用的是同一套备份（`<原文件>.stool.bak`），所以渲染与还原也共用一份。
--------------------------------------------------------------------------- */

/**
 * 在容器里挂上「列出备份 + 一键还原」。
 *
 * @param host            容器元素（会整块替换其内容）
 * @param dir             去哪个目录里找 `.stool.bak`
 * @param opts.emptyNote  没找到备份时显示的一句话
 * @param opts.onDone     还原成功后的回调（用来重绘本页）；不传则只刷新备份列表
 */
function mountBackups(host, dir, opts = {}) {
  if (!host) return;
  const emptyNote = opts.emptyNote || "这个目录里还没有备份。";

  /** 「查一次」按钮：失败/空列表时复用，点它重跑 list()。 */
  function loadBtn(label) {
    host.innerHTML = `<button class="btn btn-ghost btn-sm" data-bk="load">${esc(label)}</button>`;
    host.querySelector('[data-bk="load"]').addEventListener("click", list);
  }

  async function list() {
    const btn = host.querySelector('[data-bk="load"]');
    const done = btn ? busy(btn, "读取中…") : null;
    const r = await Tauri.call("restore_list", { dir });
    if (done) done();

    if (!r.ok) {
      host.innerHTML = noteHtml(r.err) + `<div class="mt3"><button class="btn btn-ghost btn-sm" data-bk="retry">再试一次</button></div>`;
      host.querySelector('[data-bk="retry"]').addEventListener("click", list);
      return;
    }

    const rows = r.data || [];
    if (!rows.length) {
      host.innerHTML =
        `<div class="small dim2">${esc(emptyNote)}</div>` +
        `<div class="mt3"><button class="btn btn-ghost btn-sm" data-bk="retry">重新查一次</button></div>`;
      host.querySelector('[data-bk="retry"]').addEventListener("click", list);
      return;
    }

    host.innerHTML = `
      <div class="small dim2">
        找到 ${rows.length} 份备份。
        ${esc(opts.note || "点「还原」就把备份里的内容写回原文件 —— 备份本身会留着，所以可以反复还原。")}
      </div>
      <div class="row tight mt2" style="flex-direction:column;align-items:stretch">
        ${rows
          .map(
            (b) => `<div class="row tight">
              <div style="flex:1 1 auto;min-width:0">
                <div class="mono small" title="${esc(b.path)}">${esc(b.name)}</div>
                <div class="small dim2">${esc(b.size)}${b.age ? " · " + esc(b.age) : ""}</div>
              </div>
              <button class="btn btn-ghost btn-sm" data-bk="one" data-p="${esc(b.path)}">还原</button>
            </div>`
          )
          .join("")}
      </div>`;

    host.querySelectorAll('[data-bk="one"]').forEach((b) => {
      b.addEventListener("click", async () => {
        const d2 = busy(b, "还原中…");
        const r2 = await Tauri.call("restore_one", { bak: b.dataset.p });
        d2();
        if (!r2.ok) return toast(r2.err, "err");
        toast(r2.data || "已还原。");
        if (opts.onDone) opts.onDone(r2.data);
        else list();
      });
    });
  }

  loadBtn("查看备份");
}

/** 写回成功后显示的一条「还原到改之前」，放在写回按钮旁边。 */
function undoBarHtml(bakPath, note) {
  return `
    <div class="row tight mt3" data-undo-host>
      <div style="flex:1 1 auto;min-width:0">
        <div class="small dim2">${esc(note || "写回前的原文件已备份，改坏了可以还原回去。")}</div>
        <div class="mono small dim2" title="${esc(bakPath)}">${esc(shortPath(bakPath))}</div>
      </div>
      <button class="btn btn-ghost btn-sm" data-undo>还原到改之前</button>
    </div>`;
}

/** 绑定 undoBarHtml 里那个按钮。 */
function bindUndo(host, bakPath, onDone) {
  const b = host && host.querySelector("[data-undo]");
  if (!b) return;
  b.addEventListener("click", async () => {
    const done = busy(b, "还原中…");
    const r = await Tauri.call("restore_one", { bak: bakPath });
    done();
    if (!r.ok) return toast(r.err, "err");
    toast(r.data || "已还原。");
    if (onDone) onDone();
  });
}
