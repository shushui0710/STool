/* ---------------------------------------------------------------------------
   ⑤ 改存档。

   设计要点（docs/TAURI重构方案.md §5.1 第 4/6 条）：
   - 搜索优先：大列表虚拟化，只渲染可见行（几千行也不卡）；
   - 常用字段速改：不想搜索的人一键改金币/等级/HP，不必懂字段位置；
   - 原始结构（字段位置）收进「进阶」，主页面不再有「用途不明的滚动区」。
--------------------------------------------------------------------------- */

/** 常用字段预设：按「用户会说的词」找字段，找不到就不显示这一格。 */
const QUICK_FIELDS = [
  { label: "金币 / 钱", keys: ["gold", "money"] },
  { label: "等级", keys: ["level", "lv"] },
  { label: "经验值", keys: ["exp"] },
  { label: "体力 HP", keys: ["hp"] },
  { label: "魔力 MP", keys: ["mp"] },
];

const SavePage = {
  title: "改存档",
  sub: "改金币、等级、道具数量。改完点「写回存档」，原文件会先自动备份，随时能还原。",

  root: null,
  info: null,
  rows: [],
  query: "",
  scope: "all",
  selected: null,
  dirty: false,
  locs: [],
  files: [],
  curLoc: "",
  quick: [],
  err: "",
  autoTried: false,
  // 写回成功后记住这次的备份路径 —— 有了它才能在页面上就地给出「还原到改之前」。
  // 存档是「每写一次就覆盖备份」，所以这个备份 = 上一次写之前的样子。
  lastBackup: "",

  ROW_H: 30,
  BUFFER: 8,

  actions() {
    return this.info ? `<button class="btn btn-ghost btn-sm" id="sSwitch">换一个存档</button>` : "";
  },

  async mount(host) {
    this.root = host;
    log("save: mount 进入");
    if (this.info) {
      this.renderEditor();
      return this;
    }
    await this.renderPicker();
    log("save: renderPicker 完成");

    // 自动化验证钩子：STOOL_SAVE 指定存档时启动即加载。
    // 这里的 trace 用 **await** 写日志，保证顺序确定（`log()` 是发了不管，
    // 一旦后面某步卡住就看不出卡在哪）。
    if (!this.info && !this.autoTried) {
      this.autoTried = true;
      await Tauri.call("log_line", { s: "app.js trace: 即将调用 env_save_path" });
      const r = await Tauri.call("env_save_path");
      await Tauri.call("log_line", {
        s: `app.js trace: env_save_path 返回 ok=${r.ok} data=${r.data || ""} err=${r.err || ""}`,
      });
      if (r.ok && r.data) {
        await Tauri.call("log_line", { s: `app.js trace: 即将 loadSave ${r.data}` });
        await this.loadSave(r.data);
        await Tauri.call("log_line", { s: "app.js trace: loadSave 返回" });
      }
    }
    return this;
  },

  // =========================================================================
  // 选存档
  // =========================================================================

  async renderPicker() {
    if (!Store.gameRoot) {
      this.root.innerHTML = `<div class="seg">${emptyHtml(
        "🎮",
        "还不知道要改哪个游戏的存档",
        "STool 得先知道游戏在哪，才能找到它的存档目录。",
        "去选游戏",
        "sGoDetect"
      )}</div>`;
      $("#sGoDetect").addEventListener("click", () => {
        location.hash = "#detect";
      });
      return;
    }

    if (!this.locs.length && !this.err) {
      const r = await Tauri.call("save_locations", { root: Store.gameRoot });
      this.locs = r.ok ? r.data : [];
      if (!r.ok) this.err = r.err;
    }

    const locHtml = this.locs.length
      ? `<div class="row tight" style="flex-direction:column;align-items:stretch">
          ${this.locs
            .map(
              (l) => `<button class="action" data-dir="${esc(l.path)}">
                <div class="action-title">${esc(l.label)}</div>
                <div class="action-hint mono">${esc(l.path)}</div>
              </button>`
            ).join("")}
        </div>`
      : `<div class="small dim2">没在这个游戏附近找到常见的存档目录。用下面的「直接选存档文件」也能打开。</div>`;

    const fileHtml = this.curLoc
      ? `<div class="card">
          <div class="card-head">
            <div class="card-title">这里面的存档</div>
            <div class="card-hint mono">${esc(this.curLoc)}</div>
          </div>
          ${
            this.files.length
              ? `<div class="row tight" style="flex-direction:column;align-items:stretch">
                  ${this.files
                    .map(
                      (f) => `<button class="action" data-file="${esc(f.path)}">
                        <div class="action-title">${esc(f.name)}</div>
                        <div class="action-hint">${esc(f.size)}${f.age ? " · 修改于 " + esc(f.age) : ""}</div>
                      </button>`
                    ).join("")}
                </div>`
              : `<div class="small dim2">这个目录里没有认识的存档文件。</div>`
          }
        </div>`
      : "";

    this.root.innerHTML = `
      <div class="seg">
        <div class="say">
          <span class="say-mark">?</span>
          <div class="say-body">
            先找到存档文件。点下面任意一个位置看里面的存档；找到了就点开它。<br>
            改之前 STool 会自动留一份备份，所以不用先自己复制。
          </div>
        </div>
      </div>

      <div class="seg">
        <div class="card-head">
          <div class="card-title">这个游戏可能的存档位置</div>
          <div class="card-hint">当前游戏：${esc(shortPath(Store.gameRoot))}</div>
        </div>
        ${locHtml}
        <div class="card-foot">
          <button class="btn btn-ghost" id="sPick">直接选一个存档文件…</button>
        </div>
      </div>

      ${fileHtml ? `<div class="seg">${fileHtml}</div>` : ""}
      ${this.err ? `<div class="seg">${noteHtml(this.err)}</div>` : ""}
    `;

    this.root.querySelectorAll("[data-dir]").forEach((b) =>
      b.addEventListener("click", () => this.openLoc(b.dataset.dir))
    );
    this.root.querySelectorAll("[data-file]").forEach((b) =>
      b.addEventListener("click", () => this.loadSave(b.dataset.file))
    );
    $("#sPick", this.root).addEventListener("click", () => this.pickFile());
  },

  async openLoc(dir) {
    const r = await Tauri.call("save_files", { dir });
    if (!r.ok) return toast(r.err, "err");
    this.files = r.data;
    this.curLoc = dir;
    await this.renderPicker();
  },

  async pickFile() {
    const r = await Tauri.call("pick_and_load_save");
    if (!r.ok) return toast(r.err, "err");
    if (!r.data) return; // 取消
    await this.loadSave(r.data.path);
  },

  async loadSave(path) {
    const r = await Tauri.call("load_save", { path });
    if (!r.ok) return toast(r.err, "err");
    this.info = r.data;
    this.rows = [];
    this.selected = null;
    this.query = "";
    this.dirty = false;
    this.quick = [];
    this.lastBackup = "";
    refreshChrome();
    this.renderEditor();
    this.loadQuick();
    log(`save: format=${r.data.format} writable=${r.data.writable} top=${r.data.top_fields} path=${r.data.path}`);

    const q = await Tauri.call("env_query");
    if (q.ok && q.data) {
      const box = $("#sQuery", this.root);
      if (box) box.value = q.data;
      this.doSearch(q.data);
    }
  },

  // =========================================================================
  // 编辑器
  // =========================================================================

  renderEditor() {
    const d = this.info;
    const ro = !d.writable;
    const warn = ro
      ? noteHtml("这个格式只能看、不能改（STool 目前写不了它）。\n改法：MV(.rpgsave) / MZ(.rmzsave) / 明文 JSON 可以改。", "warn")
      : "";

    this.root.innerHTML = `
      <div class="seg">
        <div class="card">
          <div class="row tight">
            <span class="pill ${ro ? "muted" : "high"}">${ro ? "只读" : "可修改"}</span>
            <span class="tag">${esc(d.format)}</span>
            <span class="small dim">顶层 ${d.top_fields} 项</span>
          </div>
          <div class="verdict-path mt2">${esc(d.path)}</div>
          ${warn ? `<div class="mt3">${warn}</div>` : ""}
        </div>
      </div>

      <div class="seg">
        <div class="card">
          <div class="card-head">
            <div class="card-title">常用字段速改</div>
            <div class="card-hint">不用搜索，直接改这几样</div>
          </div>
          <div id="quickHost" class="quick">
            <div class="small dim">正在找常用字段…</div>
          </div>
        </div>
      </div>

      <div class="seg">
        <div class="card">
          <div class="card-head">
            <div class="card-title">搜索字段</div>
            <div class="card-hint">想改什么就搜什么，比如 gold、等级、道具名</div>
          </div>
          <div class="row">
            <input class="field grow" id="sQuery" placeholder="搜字段名或内容…（回车开始搜）">
            <select class="field" id="sScope">
              <option value="all">字段名 + 内容</option>
              <option value="keys">只搜字段名</option>
              <option value="values">只搜内容</option>
            </select>
            <button class="btn btn-ghost" id="sGo">搜索</button>
          </div>

          <div class="row tight mt3">
            <span class="small dim" id="sCount"></span>
          </div>

          <div class="vlist mt2" id="vlist">
            <div class="vlist-inner" id="vlistInner"></div>
          </div>

          <div class="mt3" id="editHost"></div>
        </div>
      </div>

      <div class="seg">
        <div class="card">
          <div id="commitHost"></div>
        </div>
      </div>

      <div class="seg">
        ${foldHtml(
          "备份与还原（改坏了用这里）",
          `<div id="bkHost"></div>`,
          !!this.lastBackup
        )}
      </div>

      <div class="seg">
        ${foldHtml(
          "进阶：这是什么文件、字段位置怎么看",
          `<ul class="bullets">
             <li>存档格式：${esc(d.format)}</li>
             <li>字段位置（JSON Pointer）形如 <span class="mono">/party/_gold</span>，用于精确定位一个值。</li>
             <li>改完必须点「写回存档」才会真正写进文件；备份是 <span class="mono">&lt;文件名&gt;.stool.bak</span>，想还原就点上面的「备份与还原」。</li>
           </ul>`
        )}
      </div>
    `;

    $("#sGo", this.root).addEventListener("click", () => {
      const q = $("#sQuery", this.root).value;
      this.doSearch(q);
    });
    $("#sQuery", this.root).addEventListener("keydown", (e) => {
      if (e.key === "Enter") this.doSearch($("#sQuery", this.root).value);
    });
    $("#sScope", this.root).addEventListener("change", (e) => {
      this.scope = e.target.value;
      if (this.query) this.doSearch(this.query);
    });

    const list = $("#vlist", this.root);
    list.addEventListener("scroll", () => this.paintRows(), { passive: true });
    list.addEventListener("click", (e) => {
      const row = e.target.closest(".vrow");
      if (!row) return;
      this.selected = this.rows[Number(row.dataset.i)];
      this.paintRows();
      this.renderEdit();
    });

    this.paintRows();
    this.renderEdit();
    this.paintCommit();

    // 备份列表：初始只渲染一个按钮，点了才去读盘（不点不发 IPC）。
    mountBackups($("#bkHost", this.root), dirOf(d.path), {
      emptyNote: "这个存档还没有备份（写过一次之后就有了）。",
      onDone: () => this.reloadAfterRestore(),
    });
  },

  /** 还原之后：文件内容已变，把手上的文档丢掉重新从磁盘读一遍。 */
  async reloadAfterRestore() {
    const p = this.info && this.info.path;
    this.lastBackup = "";
    if (!p) return;
    await this.loadSave(p);
  },

  async doSearch(q) {
    this.query = q;
    if (!q) {
      this.rows = [];
      this.paintRows();
      return;
    }
    const r = await Tauri.call("search_save", { query: q, scope: this.scope });
    if (!r.ok) return toast(r.err, "err");
    this.rows = r.data || [];
    this.selected = null;
    this.paintRows();
    this.renderEdit();
    log(`save search "${q}" scope=${this.scope} -> ${this.rows.length} 行`);
  },

  /** 只渲染可见窗口内的行 —— 这是「几千行不卡」的关键。 */
  paintRows() {
    const list = $("#vlist", this.root);
    const inner = $("#vlistInner", this.root);
    if (!list || !inner) return;

    const total = this.rows.length;
    const H = this.ROW_H;
    inner.style.height = total * H + "px";

    const top = list.scrollTop;
    const h = list.clientHeight || 320;
    const start = Math.max(0, Math.floor(top / H) - this.BUFFER);
    const end = Math.min(total, Math.ceil((top + h) / H) + this.BUFFER);

    let html = "";
    for (let i = start; i < end; i++) {
      const r = this.rows[i];
      const on = this.selected && this.selected.path === r.path ? " on" : "";
      html += `<div class="vrow${on}" style="top:${i * H}px" data-i="${i}">
        <div class="vrow-key" title="${esc(r.path)}">${esc(r.key)}</div>
        <div class="vrow-val v-${esc(r.kind)}" title="${esc(r.value)}">${esc(clip(r.value, 70))}</div>
        <div class="vrow-ty">${esc(r.ty)}</div>
      </div>`;
    }
    inner.innerHTML = html;

    const cnt = $("#sCount", this.root);
    if (cnt) {
      cnt.textContent = total
        ? `找到 ${total} 个字段${total >= 500 ? "（只显示前 500 个，把关键词写具体些更准）" : ""} · 点一行就能改`
        : this.query
        ? "没有匹配的字段。换个关键词试试，或者把范围改成「字段名 + 内容」。"
        : "";
    }
  },

  renderEdit() {
    const host = $("#editHost", this.root);
    if (!host) return;
    const r = this.selected;
    if (!r) {
      host.innerHTML = "";
      return;
    }
    const ro = this.info && !this.info.writable;
    host.innerHTML = `
      <div class="card flat">
        <div class="row tight">
          <div style="flex:1 1 auto;min-width:0">
            <div class="quick-label">正在改这个字段</div>
            <div class="quick-key" style="margin-bottom:0">${esc(r.path)}</div>
          </div>
          <input class="field field-num" id="editVal" value="${esc(r.value)}" ${ro ? "disabled" : ""}>
          <span class="tag">${esc(r.ty)}</span>
          <button class="btn btn-primary btn-sm" id="editSave" ${ro ? "disabled" : ""}>改成这个值</button>
        </div>
        <div class="small dim mt2">数字直接填数字；文本直接打字；是 / 否 填 true / false。</div>
      </div>
    `;
    if (ro) return;
    $("#editSave", host).addEventListener("click", () => this.commitEdit());
    const box = $("#editVal", host);
    box.addEventListener("keydown", (e) => {
      if (e.key === "Enter") this.commitEdit();
    });
    box.focus();
    box.select();
  },

  async commitEdit() {
    const r = this.selected;
    const box = $("#editVal", this.root);
    if (!r || !box) return;
    const res = await Tauri.call("set_save_value", { ptr: r.path, raw: box.value });
    if (!res.ok) return toast(res.err, "err");

    const updated = res.data;
    const i = this.rows.findIndex((x) => x.path === updated.path);
    if (i >= 0) this.rows[i] = updated;
    this.selected = updated;

    // 常用字段里如果是同一个字段，一起刷新（否则两处显示不一致）
    for (const q of this.quick) {
      if (q.row.path === updated.path) q.row = updated;
    }

    this.dirty = true;
    this.paintRows();
    this.renderEdit();
    this.paintQuick();
    this.paintCommit();
    refreshChrome();
  },

  paintCommit() {
    const host = $("#commitHost", this.root);
    if (!host) return;
    const ro = this.info && !this.info.writable;
    if (ro) {
      host.innerHTML = `<div class="small dim">这个存档只读，没有可写回的内容。</div>`;
      return;
    }
    const state = this.dirty
      ? noteHtml("有改动还没写进文件。点右边「写回存档」才会生效。", "info")
      : `<div class="small dim">还没有改动。</div>`;
    host.innerHTML = `
      <div class="row tight">
        <div style="flex:1 1 auto">${state}</div>
        <button class="btn btn-primary" id="sCommit" ${this.dirty ? "" : "disabled"}>写回存档</button>
      </div>
      <div class="small dim mt2">
        写回前会自动把原文件备份成 <span class="mono">&lt;文件名&gt;.stool.bak</span>，改坏了可以拿它还原。
      </div>
      ${
        this.lastBackup
          ? undoBarHtml(this.lastBackup, "刚写回的那一次已经备份好了。想反悔就点右边 —— 文件会回到写回之前的样子。")
          : ""
      }
    `;
    const b = $("#sCommit", host);
    if (b) b.addEventListener("click", () => this.commitSave());
    if (this.lastBackup) bindUndo(host, this.lastBackup, () => this.reloadAfterRestore());
  },

  async commitSave() {
    const btn = $("#sCommit", this.root);
    const done = btn ? busy(btn, "写回中…") : null;
    const r = await Tauri.call("commit_save");
    if (done) done();
    if (!r.ok) return toast(r.err, "err");
    this.dirty = false;
    // 内核的备份规则就是「原路径 + .stool.bak」，所以这里能算出刚生成的那份备份。
    if (this.info) this.lastBackup = this.info.path + ".stool.bak";
    this.paintCommit();
    refreshChrome();
    toast(r.data || "已写回存档。");
  },

  // -- 常用字段 -------------------------------------------------------------

  async loadQuick() {
    const out = [];
    for (const f of QUICK_FIELDS) {
      for (const k of f.keys) {
        const r = await Tauri.call("search_save", { query: k, scope: "keys" });
        if (!r.ok) break;
        const hit = (r.data || []).find((x) => x.kind === "num");
        if (hit) {
          out.push({ label: f.label, row: hit });
          break;
        }
      }
    }
    this.quick = out;
    this.paintQuick();
  },

  paintQuick() {
    const host = $("#quickHost", this.root);
    if (!host) return;
    const ro = this.info && !this.info.writable;
    if (!this.quick.length) {
      host.innerHTML = `<div class="small dim">这个存档里没找到常见的数值字段。用下面的搜索找找看。</div>`;
      return;
    }
    host.innerHTML = this.quick
      .map(
        (q, i) => `<div class="quick-item">
          <div class="quick-label">${esc(q.label)}</div>
          <div class="quick-key" title="${esc(q.row.path)}">${esc(q.row.key)}</div>
          <div class="row tight">
            <input class="field field-num" data-qbox="${i}" value="${esc(q.row.value)}" ${ro ? "disabled" : ""}>
            <button class="btn btn-ghost btn-sm" data-qgo="${i}" ${ro ? "disabled" : ""}>改</button>
          </div>
        </div>`
      )
      .join("");

    // 用下标定位，而不是把路径塞进选择器（路径里可能有引号/斜杠，拼选择器会炸）
    host.querySelectorAll("[data-qgo]").forEach((b) => {
      b.addEventListener("click", () => {
        const i = Number(b.dataset.qgo);
        const box = host.querySelector(`[data-qbox="${i}"]`);
        if (box) this.commitQuick(this.quick[i].row.path, box.value);
      });
    });
    host.querySelectorAll("[data-qbox]").forEach((box) => {
      box.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          const i = Number(box.dataset.qbox);
          this.commitQuick(this.quick[i].row.path, box.value);
        }
      });
    });
  },

  async commitQuick(ptr, raw) {
    const res = await Tauri.call("set_save_value", { ptr, raw });
    if (!res.ok) return toast(res.err, "err");
    const updated = res.data;
    for (const q of this.quick) if (q.row.path === updated.path) q.row = updated;
    const i = this.rows.findIndex((x) => x.path === updated.path);
    if (i >= 0) this.rows[i] = updated;
    this.dirty = true;
    this.paintQuick();
    this.paintRows();
    this.paintCommit();
    refreshChrome();
  },

  bindActions() {
    const b = $("#sSwitch");
    if (b) {
      b.addEventListener("click", () => {
        this.info = null;
        this.rows = [];
        this.files = [];
        this.curLoc = "";
        this.locs = [];
        this.err = "";
        refreshChrome();
        this.renderPicker();
      });
    }
  },
};
