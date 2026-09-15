/* ---------------------------------------------------------------------------
   ⑦ 解锁全CG。

   这一页要同时满足三件事：
   1. **先看清楚**：进页面即出一份只读计划（识别依据 / 解锁动作 / 可用手段 /
      有没有自带全CG存档 / 会复制到哪个目录）—— 全走内核同一份策略表
      （`features::unlock`），界面不自己另判一遍，否则「界面说会走 A、内核走 B」。
   2. **再动手**：只有勾了「我确认要写入」才把 `apply=1` 传下去；缺省一律只读。
      覆盖写盘前内核自动留 `.stool.bak`。
   3. **能撤回**：结果卡里就地给出备份列表 + 「还原」，不用去命令行敲 `stool restore`。

   设计上刻意不做「按引擎分支」的界面 —— 手段是数据（`routes`），界面只管画。
   新增引擎时内核加一行策略表就行，这一页不用改。
--------------------------------------------------------------------------- */

const UnlockPage = {
  title: "解锁全CG",
  sub: "一键打开所有回想与画廊，不用一格格解锁。",

  root: null,
  plan: null, // UnlockPlan（只读计划）
  loading: true,
  err: "",

  // 选项
  apply: false, // 勾选后才真的写盘
  route: "", // "" = 自动
  saveDir: "",
  filter: "",

  running: false,
  prog: null,
  result: null, // OpResult
  backups: [], // 撤销用
  showBackups: false,
  unlisten: null,

  actions() {
    return "";
  },

  async mount(host) {
    this.root = host;
    // 每次进页面都重新采一次（用户可能刚跑过游戏、冒出新的存档目录）
    this.plan = null;
    this.result = null;
    this.backups = [];
    this.showBackups = false;
    this.loading = true;
    this.err = "";

    if (!Store.gameRoot) return this.renderNeedGame();

    await this.render(); // 先画骨架（含「正在读取…」）
    await this.loadPlan();
    return this;
  },

  unmount() {
    if (this.unlisten) {
      try {
        this.unlisten();
      } catch (_) {}
      this.unlisten = null;
    }
  },

  // -- 数据 -----------------------------------------------------------------

  async loadPlan() {
    const r = await Tauri.call("unlock_plan");
    this.loading = false;
    if (!r.ok) {
      this.err = r.err;
      log(`unlock: unlock_plan 失败 ${r.err}`);
    } else {
      this.plan = r.data;
      log(
        `unlock: plan engine=${r.data.engine_id} known=${r.data.known} ` +
          `routes=${r.data.routes.length} bundled=${r.data.bundled.length} ` +
          `saves=${r.data.save_dirs.length}`
      );
    }
    await this.render();
    if (this.plan && this.plan.bundled.length > 0) await this.loadBackups();
  },

  async loadBackups() {
    const r = await Tauri.call("unlock_backups");
    if (r.ok) {
      this.backups = r.data || [];
      log(`unlock: 备份 ${this.backups.length} 个`);
      await this.render();
    }
  },

  // -- 渲染 -----------------------------------------------------------------

  async render() {
    if (!Store.gameRoot) return this.renderNeedGame();
    this.root.innerHTML = [
      this.introHtml(),
      this.gameCardHtml(),
      this.loading ? this.loadingHtml() : "",
      this.err ? `<div class="seg">${noteHtml(this.err)}</div>` : "",
      this.plan ? this.planCardHtml() : "",
      this.plan ? this.runCardHtml() : "",
      this.result ? this.resultHtml() : "",
      this.plan ? this.undoHtml() : "",
    ].join("");
    this.bind();
    if (this.running) this.paintProgress(this.prog || { frac: 0, msg: "正在准备…" });
  },

  renderNeedGame() {
    this.root.innerHTML = `<div class="seg">${emptyHtml(
      "⑦",
      "还没选游戏",
      "解锁路线是按引擎选的（有的改注册表、有的改存档），所以得先知道这是什么游戏。",
      "去选游戏",
      "uGoDetect"
    )}</div>`;
    $("#uGoDetect", this.root).addEventListener("click", () => {
      location.hash = "#detect";
    });
  },

  introHtml() {
    return `
      <div class="seg">
        <div class="say">
          <span class="say-mark">?</span>
          <div class="say-body">
            先看清楚<strong>它打算怎么解</strong>，确认没动了不该动的东西，再决定要不要写入。<br>
            下面那份计划是<strong>只读</strong>的 —— 光是打开这一页不会改动任何文件。
          </div>
        </div>
      </div>`;
  },

  gameCardHtml() {
    const d = Store.detect;
    const engine = d ? d.engine_name : "（还没识别出来）";
    const p = this.plan;
    const known = p ? (p.known ? "已收录" : "未收录（走通用方案）") : "…";
    return `
      <div class="seg">
        <div class="card flat">
          <div class="row tight">
            <span class="pill high">当前游戏</span>
            <span class="tag">${esc(engine)}</span>
            <span class="tag">${esc(known)}</span>
          </div>
          <div class="verdict-path mt2">${esc(Store.gameRoot)}</div>
        </div>
      </div>`;
  },

  loadingHtml() {
    return `
      <div class="seg">
        <div class="card">
          <div class="row tight"><span class="spin"></span>
            <span class="small dim2">正在读取解锁路线…</span></div>
        </div>
      </div>`;
  },

  /** 只读计划：这就是 soon 页承诺的「列出现在能找到的解锁途径」。 */
  planCardHtml() {
    const p = this.plan;
    return `
      <div class="seg">
        <div class="card">
          <div class="card-head">
            <div class="card-title">打算怎么解（只读预览）</div>
            <div class="card-hint">不写盘 · 随时可重看</div>
          </div>

          <dl style="margin:0">
            <div class="defrow"><dt>识别依据</dt><dd>${esc(p.basis)}</dd></div>
            <div class="defrow"><dt>解锁动作</dt><dd>${esc(p.action)}</dd></div>
            ${
              p.note
                ? `<div class="defrow"><dt>小提示</dt><dd class="dim2">${esc(p.note)}</dd></div>`
                : ""
            }
            <div class="defrow"><dt>默认走法</dt>
              <dd><span class="tag">${esc(p.default_route_label)}</span></dd></div>
          </dl>

          <div class="mt3">
            <div class="small dim2">这个游戏能用的手段</div>
            <table class="tbl mt2">
              <thead><tr><th>手段</th><th>怎么做</th><th>能不能自动</th></tr></thead>
              <tbody>
                ${p.routes
                  .map(
                    (r) => `<tr>
                      <td>
                        ${r.recommended ? `<span class="pill high">推荐</span> ` : ""}
                        ${esc(r.label)}
                      </td>
                      <td class="small">${esc(r.desc)}</td>
                      <td class="small">${
                        r.automated
                          ? `<span class="pill medium">可以</span>`
                          : `<span class="pill none">只能给指引</span>`
                      }</td>
                    </tr>`
                  )
                  .join("")}
              </tbody>
            </table>
          </div>

          <div class="mt3">${this.discoveryHtml()}</div>
        </div>
      </div>`;
  },

  /** 自带全CG存档 + 目标存档目录：把「会从哪来、落到哪去」讲明白。 */
  discoveryHtml() {
    const p = this.plan;
    const hasBundle = p.bundled.length > 0;
    const hasSave = p.save_dirs.length > 0;

    const list = (arr, limit) => {
      const shown = arr.slice(0, limit);
      let h = shown.map((x) => `<div class="mono small">${esc(x)}</div>`).join("");
      if (arr.length > limit) {
        h += `<div class="small dim2">…另外还有 ${arr.length - limit} 个</div>`;
      }
      return h;
    };

    let bundleBlock;
    if (hasBundle) {
      bundleBlock = `
        <div class="note ok">
          发现 <strong>${p.bundled.length}</strong> 个「自带全CG存档」候选 ——
          这是最省事的一条路：把它按同名复制进存档目录，不用碰游戏文件。
          <div class="mt2">${list(p.bundled, 6)}</div>
        </div>`;
    } else {
      bundleBlock = `
        <div class="note info">
          没发现游戏自带的「全CG存档」。<br>
          这不代表没办法 —— 可以在上面「手段」里选一条，或手动指定存档目录后再试。<br>
          <span class="dim2">常见做法：从网盘分享包里找 <span class="mono">全CG存档/</span> 文件夹，
          放进游戏目录后回来重进这一页。</span>
        </div>`;
    }

    const saveBlock = hasSave
      ? `<div class="mt3">
           <div class="small dim2">存档会落到这里</div>
           <div class="mt2">${list(p.save_dirs, 4)}</div>
           <div class="row mt2">
             <button class="btn btn-ghost btn-sm" id="uOpenSaves">打开存档文件夹</button>
           </div>
         </div>`
      : `<div class="note warn mt3">
           没找到存档目录。多数游戏<strong>先运行一次</strong>才会生成存档目录 ——
           跑一下游戏、随便存个档，再回来重进这一页。
         </div>`;

    return `${bundleBlock}${saveBlock}`;
  },

  /** 动手区：勾选确认 → 执行；不勾只出只读报告。 */
  runCardHtml() {
    const run = this.running;
    const p = this.plan;

    const routeOpts = [
      `<option value="" ${this.route === "" ? "selected" : ""}>自动（推荐：按上面的默认走法）</option>`,
    ]
      .concat(
        p.routes.map(
          (r) =>
            `<option value="${esc(r.key)}" ${this.route === r.key ? "selected" : ""}>` +
            `${esc(r.label)}${r.automated ? "" : "（只能给指引）"}</option>`
        )
      )
      .join("");

    return `
      <div class="seg">
        <div class="card">
          <div class="card-head">
            <div class="card-title">动手解锁</div>
            <div class="card-hint">不勾下面的确认框 = 只出报告，不改任何东西</div>
          </div>

          <div class="note warn">
            真要写入时，STool 会在覆盖前把原文件备份成
            <span class="mono">&lt;文件名&gt;.stool.bak</span>，出问题可在本页下方一键还原。
          </div>

          <div class="row mt3">
            <label class="small" style="min-width:64px">解锁手段</label>
            <select class="field grow" id="uRoute" ${run ? "disabled" : ""}>${routeOpts}</select>
          </div>

          <div class="row mt2">
            <label class="small" style="min-width:64px">存档目录</label>
            <input class="field mono grow" id="uSaveDir" readonly
                   value="${esc(this.saveDir)}"
                   placeholder="自动寻找（推荐）">
            <button class="btn btn-ghost" id="uPickSave" ${run ? "disabled" : ""}>手动指定</button>
            ${this.saveDir ? `<button class="btn btn-ghost" id="uClearSave" ${run ? "disabled" : ""}>还原为自动</button>` : ""}
          </div>

          <label class="row tight mt3 small" style="cursor:pointer">
            <input type="checkbox" id="uApply" ${this.apply ? "checked" : ""} ${run ? "disabled" : ""}>
            <span>我确认要写入（已了解会覆盖存档，且已自动备份）</span>
          </label>

          <div id="uProg">${run ? this.progressHtml() : ""}</div>

          <div class="row mt3">
            <button class="btn ${this.apply ? "btn-danger" : "btn-primary"} btn-lg" id="uGo"
                    ${run ? "disabled" : ""}>
              ${this.apply ? "确认写入并解锁" : "只看报告（不写入）"}
            </button>
          </div>
          <div class="small dim mt3">
            第一遍建议先点「只看报告」—— 看清会复制哪些文件、落到哪个目录，再决定写入。
          </div>
        </div>
      </div>

      <div class="seg">${this.advancedHtml()}</div>`;
  },

  /** 进阶：Unity 注册表路线的键名过滤。收起来，绝大多数人不用管。 */
  advancedHtml() {
    const run = this.running;
    return foldHtml(
      "进阶：Unity 注册表键名过滤",
      `<div class="small dim2">
         只在走「改注册表键位」这条路（Unity 引擎）时才有用 ——
         用来把要改写的键名限定在含某段文字的范围内。
         留空 = 自动扫描 <span class="mono">Assembly-CSharp.dll</span> 找解锁相关键名。
       </div>
       <div class="row mt3">
         <input class="field mono grow" id="uFilter" value="${esc(this.filter)}"
                placeholder="留空 = 自动（推荐），例如 gallery" ${run ? "disabled" : ""}>
       </div>
       <div class="small dim mt2">
         拿不准就留空。填错顶多是「没找到要改的键」，不会改坏东西。
       </div>`
    );
  },

  resultHtml() {
    const r = this.result;
    const ok = r.success;
    return `
      <div class="seg">
        <div class="card">
          <div class="card-head">
            <div class="card-title">${ok ? "执行完成" : "没有成功"}</div>
            <div class="card-hint">${
              r.files_done > 0 ? `处理 ${r.files_done} 个文件` : "详见下方说明"
            }</div>
          </div>
          <div class="note ${ok ? "ok" : "err"}"><pre class="mono small" style="margin:0;white-space:pre-wrap">${esc(
            r.message
          )}</pre></div>
          <div class="row mt3">
            <button class="btn btn-ghost" id="uRefresh">重新读取计划</button>
          </div>
          <div class="small dim mt3">
            接下来：启动游戏，看画廊 / CG 回廊是不是全开了。若还缺项，说明这份存档不含全部条目 ——
            换一条手段再试。
          </div>
        </div>
      </div>`;
  },

  /** 撤销区：soon 页承诺的「写入后提供『撤销上一次』」。 */
  undoHtml() {
    if (this.backups.length === 0) return "";
    return `
      <div class="seg">
        <div class="card">
          <div class="card-head">
            <div class="card-title">还原 / 撤销</div>
            <div class="card-hint">发现 ${this.backups.length} 个备份</div>
          </div>
          <div class="small dim2">
            STool 每次覆盖文件前都会留一份 <span class="mono">.stool.bak</span>。
            还原会把备份写回原文件，<strong>备份本身保留</strong>，可以反复还原。
          </div>
          ${
            this.showBackups
              ? `<div class="mt3">${this.backups
                  .map(
                    (b, i) => `<div class="bakrow">
                      <span class="mono small grow" title="${esc(b)}">${esc(shortPath(b))}</span>
                      <button class="btn btn-ghost btn-sm" data-restore="${i}">还原</button>
                    </div>`
                  )
                  .join("")}</div>`
              : ""
          }
          <div class="row mt3">
            <button class="btn btn-ghost" id="uToggleBak">${
              this.showBackups ? "收起备份列表" : "查看备份列表"
            }</button>
            <button class="btn btn-ghost" id="uRefreshBak">刷新</button>
          </div>
        </div>
      </div>`;
  },

  // -- 进度 -----------------------------------------------------------------

  progressHtml() {
    return `
      <div class="mt3" id="uProgBody">
        <div class="row tight">
          <span class="small dim2" data-msg style="flex:1 1 auto;min-width:0">正在准备…</span>
          <span class="small mono" data-pct>0%</span>
        </div>
        <div class="bar mt2"><i style="width:0%"></i></div>
      </div>`;
  },

  paintProgress(p) {
    this.prog = p;
    const host = $("#uProg", this.root);
    if (!host || !host.firstElementChild) return;
    const pct = Math.max(0, Math.min(100, Math.round((p.frac || 0) * 100)));
    const bar = host.querySelector(".bar > i");
    if (bar) bar.style.width = pct + "%";
    const pv = host.querySelector("[data-pct]");
    if (pv) pv.textContent = pct + "%";
    const mv = host.querySelector("[data-msg]");
    if (mv) mv.textContent = clip(p.msg || "…", 64);
  },

  // -- 绑定 -----------------------------------------------------------------

  bind() {
    const on = (id, fn, ev) => {
      const e = $("#" + id, this.root);
      if (e) e.addEventListener(ev || "click", fn);
    };

    on("uGo", () => this.run());
    on("uRefresh", () => this.reload());

    // 勾选确认框只切按钮的样式与文案，不整页重绘（重绘会把折叠/下拉状态抖掉）
    const apply = $("#uApply", this.root);
    if (apply) {
      apply.addEventListener("change", (e) => {
        this.apply = !!e.target.checked;
        this.paintRunButton();
      });
    }

    const route = $("#uRoute", this.root);
    if (route) route.addEventListener("change", (e) => (this.route = e.target.value));

    // 键名过滤在折叠区里：只记值、不重绘 —— 重绘会把折叠合上、把输入焦点弄丢。
    const filter = $("#uFilter", this.root);
    if (filter) filter.addEventListener("input", (e) => (this.filter = e.target.value));

    on("uPickSave", () => this.pickSaveDir());
    on("uClearSave", async () => {
      this.saveDir = "";
      await this.render();
    });

    const openSaves = $("#uOpenSaves", this.root);
    if (openSaves) {
      openSaves.addEventListener("click", () => {
        const d = this.saveDir || (this.plan.save_dirs[0] || "");
        if (d) this.openFolder(d);
      });
    }

    on("uToggleBak", async () => {
      this.showBackups = !this.showBackups;
      await this.render();
    });
    on("uRefreshBak", () => this.loadBackups());

    // 每个备份一个「还原」按钮
    this.root.querySelectorAll("[data-restore]").forEach((b) =>
      b.addEventListener("click", () => this.restore(this.backups[+b.dataset.restore]))
    );
  },

  /** 只重画执行按钮的样式/文案 + 进度占位，保住下拉与勾选状态。 */
  paintRunButton() {
    const b = $("#uGo", this.root);
    if (!b) return;
    b.className = `btn ${this.apply ? "btn-danger" : "btn-primary"} btn-lg`;
    b.textContent = this.apply ? "确认写入并解锁" : "只看报告（不写入）";
    const host = $("#uProg", this.root);
    if (host) host.innerHTML = this.running ? this.progressHtml() : "";
  },

  // -- 动作 -----------------------------------------------------------------

  async reload() {
    this.loading = true;
    this.result = null;
    this.err = "";
    await this.render();
    await this.loadPlan();
  },

  async pickSaveDir() {
    const r = await Tauri.call("pick_folder", { title: "选择游戏存档目录" });
    if (!r.ok) return toast(r.err, "err");
    if (!r.data) return;
    this.saveDir = r.data;
    await this.render();
  },

  async openFolder(p) {
    const r = await Tauri.call("open_folder", { path: p });
    if (!r.ok) toast(r.err, "err");
  },

  async run() {
    if (this.running) return;
    this.running = true;
    this.prog = { frac: 0, msg: "正在准备…" };
    this.result = null;
    this.err = "";
    await this.render();
    await this.subscribe();

    const r = await Tauri.call("unlock_run", {
      apply: this.apply,
      route: this.route,
      saveDir: this.saveDir,
      filter: this.filter,
    });

    this.running = false;
    if (!r.ok) {
      this.err = r.err;
      log(`unlock: unlock_run 失败 ${r.err}`);
    } else {
      this.result = r.data;
      log(
        `unlock: 完成 success=${r.data.success} apply=${this.apply} ` +
          `files=${r.data.files_done}`
      );
    }
    // 落地过就刷新备份列表（撤销入口要立刻反映新备份）
    if (this.apply) await this.loadBackups();
    await this.render();

    if (this.result && this.result.success) {
      toast(this.apply ? `完成：处理 ${this.result.files_done} 个文件。` : "报告已生成（未写入）。");
    } else if (this.err) {
      toast("没成功，原因见页面上的提示。", "err");
    }
  },

  async restore(bak) {
    if (!bak) return;
    const r = await Tauri.call("unlock_restore", { bak });
    if (!r.ok) return toast(r.err, "err");
    toast(r.data || "已还原。", "ok");
    log(`unlock: restore -> ${r.data}`);
    await this.loadBackups();
  },

  async subscribe() {
    if (this.unlisten) {
      try {
        this.unlisten();
      } catch (_) {}
      this.unlisten = null;
    }
    this.unlisten = await Tauri.on("op:progress", (p) => this.paintProgress(p));
  },
};
