/* ---------------------------------------------------------------------------
   ⑧ 装MOD。

   这一页的核心矛盾：**安装 = 覆盖游戏原文件**。所以三件事必须做到位：
   1. 说清代价：安装会把补丁目录里的文件覆盖到游戏目录，原文件自动备份到
      `stool_mods/_backups/`，可以随时卸载还原。这话要写在按钮**上面**，不是结果里。
   2. 冲突要在**装之前**就能看见：同一文件被两个已启用 MOD 覆盖时，游戏可能处于
      不可预期状态（谁后装谁生效，卸载一个会把另一个的改动一起还原掉）。
      内核 `install_mod` 已经在写盘前拒绝冲突，但那样只能等报错；
      所以这里多一步「预演」（`mods_preview`），选好目录就先把冲突摆出来。
   3. 装完能一键停用/卸载：停用 = 还原原文件但保留 MOD 记录（随时再开）；
      卸载 = 还原原文件 + 删掉记录。两者是不同的东西，按钮文案必须分开。

   界面**不自己判冲突** —— `conflict_count` 与 `conflicts[]` 都来自内核同一份
   `mods::conflicts`，否则「界面说没冲突、内核装的时候却拒绝」。
--------------------------------------------------------------------------- */

const ModsPage = {
  title: "装MOD",
  sub: "安装、停用别人做的补丁。原文件会自动备份，随时能还原。",

  root: null,
  state: null, // ModsState：{ mods, conflicts, store_dir, conflict_count }
  loading: true,
  err: "",
  msg: "", // 提示条（"ok" | "err" 由 msgKind 决定）

  // 安装表单
  srcDir: "", // 补丁目录
  name: "", // MOD 名（留空用目录名）
  force: false, // 允许冲突覆盖
  preview: null, // mods_preview 的结果（只关心 conflicts）
  previewing: false,

  busy: false,
  open: {}, // 折叠状态：{ [name]: true }

  actions() {
    return "";
  },

  async mount(host) {
    this.root = host;
    // 每次进页面重采一次（用户可能刚在别处装过、或手动删过文件）
    this.state = null;
    this.loading = true;
    this.err = "";
    this.msg = "";
    this.preview = null;
    this.busy = false;

    if (!Store.gameRoot) return this.renderNeedGame();

    await this.render(); // 先画骨架（含「正在读取…」）
    await this.loadState();
    return this;
  },

  unmount() {},

  // -- 数据 -----------------------------------------------------------------

  async loadState() {
    const r = await Tauri.call("mods_state");
    this.loading = false;
    if (!r.ok) {
      this.err = r.err;
      log(`mods: mods_state 失败 ${r.err}`);
    } else {
      this.state = r.data;
      log(
        `mods: ${r.data.mods.length} 个 MOD，冲突 ${r.data.conflict_count} 处`
      );
    }
    await this.render();
  },

  /**
   * 预演：选好目录后先看一遍会与谁冲突。
   * 目录为空/不存在时清掉预演结果，不要拿旧结果误导人。
   */
  async runPreview() {
    if (!this.srcDir) {
      this.preview = null;
      return;
    }
    const r = await Tauri.call("mods_preview", { srcDir: this.srcDir });
    if (r.ok) {
      this.preview = r.data;
      log(`mods: 预演冲突 ${r.data.conflict_count} 处`);
    } else {
      // 预演失败不弹错（可能只是目录选错了），清空即可，安装时会再报一次
      this.preview = null;
      log(`mods: 预演失败 ${r.err}`);
    }
  },

  // -- 渲染 -----------------------------------------------------------------

  async render() {
    if (!Store.gameRoot) return this.renderNeedGame();
    this.root.innerHTML = [
      this.introHtml(),
      this.loading ? this.loadingHtml() : "",
      this.err ? `<div class="seg">${noteHtml(this.err)}</div>` : "",
      this.state ? this.conflictCardHtml() : "",
      this.state ? this.installCardHtml() : "",
      this.state ? this.listCardHtml() : "",
    ].join("");
    this.bind();
  },

  renderNeedGame() {
    this.root.innerHTML = `
      <div class="seg">${emptyHtml(
        "⑧",
        "还没选游戏",
        "MOD 要装进游戏目录，所以得先告诉我要装到哪个游戏。",
        "去选游戏",
        "mGoDetect"
      )}</div>`;
    const b = $("#mGoDetect", this.root);
    if (b) b.addEventListener("click", () => setPage("detect"));
  },

  loadingHtml() {
    return `<div class="seg"><div class="card"><div class="row">
      <span class="spin"></span><span class="dim">正在读取已装 MOD…</span>
    </div></div></div>`;
  },

  cardWrap(title, hint, body) {
    return `<div class="seg"><div class="card">
      <div class="card-head">
        <div class="card-title">${esc(title)}</div>
        ${hint ? `<div class="card-hint">${esc(hint)}</div>` : ""}
      </div>
      ${body}
    </div></div>`;
  },

  /** 顶部说明：把「覆盖 + 可还原」这个代价讲在最前面。 */
  introHtml() {
    return `<div class="seg"><div class="card flat">
      <div class="card-head"><div class="card-title">装MOD 是怎么回事</div></div>
      <div class="body">
        <p>把别人做好的<strong>补丁文件夹</strong>整个盖到游戏目录上，就能用上它改的东西（换立绘、改剧情、加功能）。</p>
        <ul class="bullets">
          <li><strong>原文件会自动留底</strong>。被覆盖的文件都备份在 <span class="mono">${esc(
            shortPath((this.state && this.state.store_dir) || ".../stool_mods")
          )}</span>，随时能还原。</li>
          <li><strong>停用</strong> = 把原文件放回去，但记住这个 MOD（以后能再开）。<strong>卸载</strong> = 还原并彻底忘掉它。</li>
          <li><strong>别让两个 MOD 抢同一个文件</strong>。真抢了游戏可能变得很奇怪 ——
              下一张卡会替你提前查出来。</li>
        </ul>
        <p class="small dim">不想装 MOD 也可以：只是想换台词的话，「翻译文字」页能做汉化；
          想改数值用「游戏里改数值」页，那两样都不需要找别人做的补丁。</p>
      </div>
    </div></div>`;
  },

  /** 冲突告警：只有真有冲突时才画（进页面即由内核算好）。 */
  conflictCardHtml() {
    const n = this.state.conflict_count;
    if (n === 0) return "";
    const rows = this.state.conflicts
      .map(
        (c) => `<div class="row tight">
          <span class="tag danger">抢文件</span>
          <span class="mono small">${esc(c.rel)}</span>
          <span class="small dim">← ${esc(c.mods.join(" / "))}</span>
        </div>`
      )
      .join("");
    return this.cardWrap(
      `⚠ 有 ${n} 处文件被多个 MOD 同时改`,
      "游戏会按「后装的赢」来读，卸载一个可能把另一个的改动也还原掉",
      `<div class="body">
        <p>下面这些文件被 <strong>不止一个</strong>已启用的 MOD 覆盖了。建议只留一个：把其余的<strong>停用</strong>掉，冲突就会消失。</p>
        ${rows}
      </div>`
    );
  },

  /** 安装卡：选目录 → 预演冲突 → 确认安装。 */
  installCardHtml() {
    const pv = this.preview;
    const pvConflict = pv && pv.conflict_count > 0;
    const canInstall = !!this.srcDir && !this.busy;

    // 预演结果：只在选了目录后才显示
    let previewHtml = "";
    if (this.srcDir) {
      if (this.previewing) {
        previewHtml = `<div class="row tight mt2"><span class="spin"></span><span class="small dim">正在检查会不会和已有 MOD 抢文件…</span></div>`;
      } else if (pvConflict) {
        const rows = pv.conflicts
          .map(
            (c) => `<div class="row tight">
              <span class="tag danger">抢文件</span>
              <span class="mono small">${esc(c.rel)}</span>
              <span class="small dim">← ${esc(c.mods.join(" / "))}</span>
            </div>`
          )
          .join("");
        previewHtml = `<div class="note warn mt2">
          <strong>这个补丁和已装的 MOD 抢 ${pv.conflict_count} 个文件。</strong>
          直接装会被拦下。请二选一：先把冲突的 MOD 停用，或勾上下面的「允许冲突覆盖」强行覆盖。
          ${rows}
        </div>`;
      } else {
        previewHtml = `<div class="note ok mt2">✔ 检查过了，和已有 MOD 没有抢文件。</div>`;
      }
    }

    const body = `<div class="body">
      <div class="row">
        <label class="small dim" style="width:72px">补丁目录</label>
        <input class="field mono grow" id="mSrc" placeholder="点右边按钮选一个文件夹" value="${esc(
          shortPath(this.srcDir)
        )}" readonly />
        <button class="btn btn-ghost" id="mPick">选择补丁目录</button>
      </div>
      <div class="row">
        <label class="small dim" style="width:72px">名字</label>
        <input class="field grow" id="mName" placeholder="留空就用文件夹名" value="${esc(this.name)}" />
      </div>
      ${previewHtml}
      <div class="row tight mt2">
        <button class="btn btn-primary" id="mInstall" ${canInstall ? "" : "disabled"}>安装到游戏</button>
        <label class="small dim" style="display:flex;align-items:center;gap:6px">
          <input type="checkbox" id="mForce" ${this.force ? "checked" : ""} />
          允许冲突覆盖
        </label>
      </div>
      <p class="small dim2 mt2">勾选「允许冲突覆盖」= 明知会和别的 MOD 抢文件也要装。
        只有在确实要用新补丁替换掉旧补丁的同一个文件时才勾。</p>
    </div>`;
    return this.cardWrap("装一个新的", "选一个补丁文件夹，盖到游戏目录上", body);
  },

  /** 已装列表。 */
  listCardHtml() {
    const ms = this.state.mods;
    if (ms.length === 0) {
      return this.cardWrap(
        "已经装的",
        "",
        `<div class="body">${emptyHtml(
          "📦",
          "还没装过 MOD",
          "装过的 MOD 会列在这里，可以随时停用或卸载。想装的话去上面那张卡选一个补丁目录。",
          "选一个补丁目录",
          "mGoInstall"
        )}</div>`
      );
    }
    const rows = ms
      .map((m) => {
        const isOpen = !!this.open[m.name];
        const files = isOpen ? this.fileListHtml(m) : "";
        return `<div class="modrow">
          <div class="modhead">
            <button class="btn btn-ghost btn-sm" data-fold="${esc(m.name)}" title="看它改了哪些文件">
              ${isOpen ? "▾" : "▸"}
            </button>
            <span class="pill ${m.enabled ? "high" : "none"}">${m.enabled ? "启用中" : "已停用"}</span>
            <span class="modname">${esc(m.name)}</span>
            <span class="small dim modmeta">${m.files} 个文件${
              m.overwritten > 0 ? `，覆盖 ${m.overwritten} 个原文件` : "，全是新增"
            }</span>
            <span class="modacts">
              <button class="btn btn-ghost btn-sm" data-toggle="${esc(m.name)}">
                ${m.enabled ? "停用" : "启用"}
              </button>
              <button class="btn btn-danger btn-sm" data-uninstall="${esc(m.name)}">卸载</button>
            </span>
          </div>
          ${files}
        </div>`;
      })
      .join("");
    return this.cardWrap(
      `已经装的（${ms.length}）`,
      "停用 = 还原原文件但保留记录；卸载 = 还原并删除记录",
      `<div class="body">${rows}</div>`
    );
  },

  /** 折叠里列出这个 MOD 带了哪些文件。 */
  fileListHtml(m) {
    const list = m.file_list || [];
    if (list.length === 0) return "";
    const shown = list.slice(0, 200);
    const more =
      list.length > shown.length
        ? `<div class="small dim2">…还有 ${list.length - shown.length} 个</div>`
        : "";
    return `<div class="body"><div class="small dim mono">${shown
      .map((f) => esc(f))
      .join("<br>")}</div>${more}</div>`;
  },

  // -- 绑定 -----------------------------------------------------------------

  bind() {
    const on = (id, fn, ev) => {
      const e = $("#" + id, this.root);
      if (e) e.addEventListener(ev || "click", fn);
    };

    on("mPick", () => this.pickDir());
    on("mInstall", () => this.install());

    // 空状态里的「选一个补丁目录」：把上面那张卡的选择按钮滚进视野并聚焦，
    // 免得用户看完整页说明最后停在一个没有出口的空白上。
    const gi = $("#mGoInstall", this.root);
    if (gi)
      gi.addEventListener("click", () => {
        const pick = $("#mPick", this.root);
        if (!pick) return;
        pick.scrollIntoView({ block: "center", behavior: "smooth" });
        pick.focus();
      });

    // 名字只记值，不重绘 —— 重绘会把输入框的焦点与光标位置弄丢
    const nm = $("#mName", this.root);
    if (nm) nm.addEventListener("input", (e) => (this.name = e.target.value));

    const fc = $("#mForce", this.root);
    if (fc)
      fc.addEventListener("change", (e) => {
        this.force = !!e.target.checked;
        // 只切提示的重音色，不整页重绘
        this.paintForceNote();
      });

    // 停用 / 启用
    this.root.querySelectorAll("[data-toggle]").forEach((b) =>
      b.addEventListener("click", () => this.toggle(b.dataset.toggle))
    );
    // 卸载
    this.root.querySelectorAll("[data-uninstall]").forEach((b) =>
      b.addEventListener("click", () => this.uninstall(b.dataset.uninstall))
    );
    // 折叠「改了哪些文件」
    this.root.querySelectorAll("[data-fold]").forEach((b) =>
      b.addEventListener("click", () => {
        const n = b.dataset.fold;
        this.open[n] = !this.open[n];
        this.render();
      })
    );
  },

  /** 勾了「允许冲突覆盖」时把说明条变红，提醒这是危险动作。 */
  paintForceNote() {
    const notes = this.root.querySelectorAll(".note");
    notes.forEach((n) => {
      if (n.textContent.includes("允许冲突覆盖")) {
        n.className = this.force ? "note err mt2" : "note warn mt2";
      }
    });
  },

  // -- 动作 -----------------------------------------------------------------

  async pickDir() {
    const d = await Tauri.call("pick_folder", { title: "选择补丁文件夹" });
    if (!d) return;
    this.srcDir = d;
    this.name = this.name || "";
    this.previewing = true;
    this.preview = null;
    await this.render();
    await this.runPreview();
    this.previewing = false;
    await this.render();
  },

  async install() {
    if (!this.srcDir) return;
    this.busy = true;
    this.msg = "";
    await this.render();

    const r = await Tauri.call("mods_install", {
      srcDir: this.srcDir,
      name: this.name,
      force: this.force,
    });
    this.busy = false;

    if (!r.ok) {
      this.msg = r.err;
      toast("安装失败", "err");
      log(`mods: 安装失败 ${r.err}`);
    } else {
      this.state = r.data;
      this.msg = "";
      const installed = (r.data.mods || []).length;
      toast(`已安装（当前共 ${installed} 个 MOD）`, "ok");
      // 装完清掉表单，避免再点一次装重复的
      this.srcDir = "";
      this.name = "";
      this.force = false;
      this.preview = null;
    }
    await this.render();
  },

  async toggle(name) {
    const cur = (this.state.mods || []).find((m) => m.name === name);
    const want = cur ? !cur.enabled : true;
    const r = await Tauri.call("mods_toggle", { name, enabled: want });
    if (!r.ok) {
      this.msg = r.err;
      toast(r.err, "err");
    } else {
      this.state = r.data;
      toast(want ? `已启用 ${name}` : `已停用 ${name}`, "ok");
    }
    await this.render();
  },

  async uninstall(name) {
    const r = await Tauri.call("mods_uninstall", { name });
    if (!r.ok) {
      this.msg = r.err;
      toast(r.err, "err");
    } else {
      this.state = r.data;
      toast(`已卸载 ${name}，原文件已还原`, "ok");
    }
    await this.render();
  },
};
