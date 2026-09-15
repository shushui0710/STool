/* ---------------------------------------------------------------------------
   ① 选游戏。

   设计要点（docs/TAURI重构方案.md §5.2）：
   - 没选游戏时：整屏只做一件事，不给任何其他按钮，避免分心；
   - 选完：一句人话结论 + 「为什么这样判断」折叠 + 这几张能力卡直达下一个任务；
   - 引擎检测结果不再摆「分数/证据表」，只留结论与可展开的依据。
--------------------------------------------------------------------------- */

/** 一个能力都映射不出来时给的出路 —— 这几页本来就不看引擎（§2.2「没有死胡同」）。 */
const NO_ENGINE_CARDS = [
  { page: "runtime", t: "游戏里改数值", h: "直接搜内存改数值，不认引擎也能用" },
  { page: "preview", t: "看素材", h: "自己选一个目录浏览里面的文件" },
  { page: "tools", t: "工具箱", h: "体检 / 日志 / 导出诊断包" },
];

const DetectPage = {
  title: "选游戏",  sub: "告诉 STool 你要处理哪个游戏。认出来之后，后面每一步都会自动带上它。",

  root: null,
  result: null,
  err: "",
  unlisten: null,
  autoTried: false,

  actions() {
    return this.result ? `<button class="btn btn-ghost btn-sm" id="dChange">换一个游戏</button>` : "";
  },

  async mount(host) {
    this.root = host;
    log("detect: mount 进入");
    this.result ? this.renderResult() : this.renderEntry();
    this.bindDrops();

    // 自动化验证钩子：STOOL_GAME 指定目录时启动即检测（与点按钮走同一条路径）。
    if (!this.result && !this.autoTried) {
      this.autoTried = true;
      const r = await Tauri.call("env_game_root");
      log(`detect: env_game_root -> ok=${r.ok} data=${r.data || ""} err=${r.err || ""}`);
      if (r.ok && r.data) await this.detect(r.data);
    }
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

  /** 系统级拖放（Tauri 转发来的）。失败也不影响「点按钮」这条主路径。 */
  bindDrops() {
    const w = Tauri.currentWin();
    if (!w || !w.onDragDropEvent) return;
    try {
      w.onDragDropEvent((e) => {
        const z = $("#dropzone", this.root);
        if (!z) return;
        const p = e.payload || {};
        if (p.type === "over" || p.type === "enter") z.classList.add("over");
        else z.classList.remove("over");
        if (p.type === "drop") {
          const first = (p.paths || [])[0];
          if (first) this.detect(first);
        }
      }).then((un) => {
        this.unlisten = un;
      });
    } catch (_) {
      /* 拖动不可用：静默降级，点按钮照样能选 */
    }
  },

  // -- 入口：整屏只有一件事 -------------------------------------------------

  renderEntry() {
    this.root.innerHTML = `
      <div class="seg">
        <div class="say">
          <span class="say-mark">?</span>
          <div class="say-body">
            这一页只做一件事：让 STool 知道游戏在哪。<br>
            把游戏文件夹拖进来，或者点下面的按钮选一个。
          </div>
        </div>
      </div>
      <div class="seg">
        <div class="dropzone" id="dropzone">
          <div class="dropzone-icon">⊞</div>
          <div class="dropzone-title">把游戏文件夹拖到这里</div>
          <div class="dropzone-note">—— 或者 ——</div>
          <button class="btn btn-primary btn-lg" id="dPick">选择游戏文件夹</button>
          <div class="dropzone-note">
            选到游戏根目录就行：里面有游戏 .exe，或者有一个 www/ 子目录的那一层。
          </div>
        </div>
      </div>
      ${this.err ? `<div class="seg">${noteHtml(this.err)}</div>` : ""}
    `;
    $("#dPick", this.root).addEventListener("click", () => this.pick());
  },

  async pick() {
    const r = await Tauri.call("pick_folder", { title: "选择游戏文件夹" });
    if (!r.ok) return toast(r.err, "err");
    if (!r.data) return; // 用户取消，不做任何事
    await this.detect(r.data);
  },

  async detect(path) {
    const btn = $("#dPick", this.root);
    const done = btn ? busy(btn, "正在识别…") : null;
    const r = await Tauri.call("detect_game", { path });
    if (done) done();

    if (!r.ok) {
      this.err = r.err;
      log(`detect 失败: ${r.err}`);
      this.renderEntry();
      return;
    }
    this.err = "";
    this.result = r.data;
    Store.gameRoot = r.data.root;
    Store.detect = r.data;
    log(`detect: ${r.data.engine_name} score=${r.data.score} level=${r.data.level} confirmed=${r.data.confirmed} caps=${r.data.capabilities.length} root=${r.data.root}`);
    refreshChrome();
    this.renderResult();
  },

  // -- 结果：一句结论 + 依据 + 直达卡片 -------------------------------------

  /** 能力 → 主路径卡片。repack/decompile 属进阶，不上主路径（只折叠保留）。 */
  capCards(d) {
    const map = {
      extract: { page: "extract", t: "取出素材", h: "把图片、音乐、脚本拿出来" },
      text_extract: { page: "text", t: "翻译文字", h: "提出台词，翻完再塞回去" },
      text_import: { page: "text", t: "翻译文字", h: "把翻译好的文本写回游戏" },
      text_inject: { page: "text", t: "不改游戏文件汉化", h: "进游戏直接是中文，游戏文件一个都不动" },
      save: { page: "save", t: "改存档", h: "金币、等级、道具数量" },
      unlock: { page: "unlock", t: "解锁全CG", h: "打开所有回想与画廊" },
    };
    const seen = new Set();
    const out = [];
    for (const c of d.capabilities || []) {
      const m = map[c.id];
      if (!m || seen.has(m.page)) continue;
      seen.add(m.page);
      out.push(m);
    }
    return out;
  },

  renderResult() {
    const d = this.result;
    const levelText = { high: "把握很大", medium: "已确认", low: "疑似", none: "没认出来" }[d.level] || d.confidence;
    const cards = this.capCards(d);
    const alts = (d.alternatives || []).filter((a) => a.score > 0);

    const evidence = d.evidence && d.evidence.length
      ? `<ul class="bullets">${d.evidence.map((e) => `<li>${esc(e)}</li>`).join("")}</ul>`
      : `<div class="small dim">没有留下可展示的判据。</div>`;

    const altHtml = alts.length
      ? `<div class="mt3 small dim2">其他可能：</div>
         <ul class="bullets">${alts.map((a) => `<li>${esc(a.name)} · ${a.score} 分 · ${esc(a.confidence)}</li>`).join("")}</ul>`
      : "";

    const noteBlock = d.notes ? `<div class="note warn mt3">${esc(d.notes)}</div>` : "";

    const card = (c) => `<button class="action" data-go="${esc(c.page)}">
        <div class="action-title">${esc(c.t)}</div>
        <div class="action-hint">${esc(c.h)}</div>
      </button>`;

    // 认得出引擎 → 列它能做的事；认不出 → 不留死胡同，改列「不依赖引擎识别」的几条路
    // （§2.2「任何页面没有死胡同」）。这三页在界面上本来就不需要引擎：改数值走内存、
    // 看素材自己选目录、工具箱随时可用。
    const actionHtml = cards.length
      ? `<div class="actions">${cards.map(card).join("")}</div>`
      : noteHtml(
          "这个引擎目前没有内置的自动化处理方式。\n改法：下面这几个不依赖引擎识别，照样能用。",
          "warn"
        ) + `<div class="actions mt3">${NO_ENGINE_CARDS.map(card).join("")}</div>`;

    this.root.innerHTML = `
      <div class="seg">
        <div class="card">
          <div class="verdict">
            <div class="verdict-main">
              <div class="row tight">
                <span class="pill ${esc(d.level)}">${esc(levelText)}</span>
                <span class="small dim">匹配度 ${d.score} 分 · 用时 ${d.elapsed_ms} ms</span>
              </div>
              <div class="verdict-line mt2">${esc(d.confirmed ? "好东西，这个游戏能改" : "这个游戏我没什么把握")}</div>
              <div class="verdict-sub">${esc(d.verdict)}</div>
              <div class="verdict-path">${esc(d.root)}</div>
            </div>
          </div>
          <div class="card-foot">
            ${foldHtml("为什么这样判断", evidence + altHtml, false)}
          </div>
          ${noteBlock}
        </div>
      </div>

      <div class="seg">
        <div class="card-head">
          <div class="card-title">接下来能做</div>
          <div class="card-hint">点一下就到对应的页面，游戏不用再选一遍</div>
        </div>
        ${actionHtml}
      </div>
    `;

    this.root.querySelectorAll("[data-go]").forEach((b) => {
      b.addEventListener("click", () => {
        location.hash = "#" + b.dataset.go;
      });
    });
  },

  bindActions() {
    const b = $("#dChange");
    if (b) {
      b.addEventListener("click", () => {
        this.result = null;
        Store.detect = null;
        refreshChrome();
        this.renderEntry();
      });
    }
  },
};
