/* ---------------------------------------------------------------------------
   ② 取出素材（+ 进阶：重新打包）。

   设计要点（docs/TAURI重构方案.md §5.2）：
   - 主路径只有一件事：选个输出目录 →「取出全部素材」→ 报数量 + 打开文件夹；
   - 进度走 Rust 推来的事件流（不是轮询），几万个文件也不打满 IPC；
   - 「重新打包」属进阶（会写回游戏目录），折叠收起 + 需要显式勾选确认；
   - 内核不支持「只取某一类」，所以**不做**那个按钮 —— 宁缺勿假。
--------------------------------------------------------------------------- */

const ExtractPage = {
  title: "取出素材",
  // 副标题只讲「这是什么页」；「会不会动到游戏」这种安全说明放下面 .say 里说，
  // 两处讲同一句话 = 白占一层信息（§4.1 的四层层级不能有重复层）。
  sub: "把游戏里的图片、音乐、脚本拿出来，存成普通文件。",

  root: null,
  outDir: "",
  running: false,
  prog: null,
  result: null,
  // 上一次跑的是「取出」还是「重新打包」—— 两者的结果卡片完全不同，得分开渲染。
  lastKind: "",
  err: "",
  unlisten: null,
  autoTried: false,
  repackSrc: "",
  repackAck: false,

  actions() {
    return "";
  },

  async mount(host) {
    this.root = host;
    await this.render();

    // 自动化验证钩子：STOOL_OUT 指定输出目录时自动填上（与手点「选择目录」同一条路径）。
    if (!this.autoTried) {
      this.autoTried = true;
      const r = await Tauri.call("env_out_dir");
      if (r.ok && r.data && !this.outDir) {
        this.outDir = r.data;
        log(`extract: env_out_dir -> ${r.data}`);
        await this.render();
      }
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

  // -- 渲染 -----------------------------------------------------------------

  async render() {
    if (!Store.gameRoot) return this.renderNeedGame();

    const d = Store.detect;
    const engine = d ? d.engine_name : "（还没识别出来）";
    const run = this.running;

    this.root.innerHTML = `
      <div class="seg">
        <div class="say">
          <span class="say-mark">?</span>
          <div class="say-body">
            这一步只做一件事：把游戏打包文件里的内容解出来，放进你指定的目录。<br>
            游戏本身<strong>不会被改动</strong>，所以放错地方也不用担心。
          </div>
        </div>
      </div>

      <div class="seg">
        <div class="card flat">
          <div class="row tight">
            <span class="pill high">当前游戏</span>
            <span class="tag">${esc(engine)}</span>
          </div>
          <div class="verdict-path mt2">${esc(Store.gameRoot)}</div>
        </div>
      </div>

      <div class="seg">
        <div class="card">
          <div class="card-head">
            <div class="card-title">解到哪个目录</div>
            <div class="card-hint">建议放在游戏目录外面，别和游戏文件混在一起</div>
          </div>
          <div class="row">
            <input class="field mono grow" id="eOut" readonly
                   value="${esc(this.outDir)}" placeholder="还没选输出目录">
            <button class="btn btn-ghost" id="ePick" ${run ? "disabled" : ""}>选择输出目录</button>
          </div>

          <div id="eProg">${run ? this.progressHtml() : ""}</div>

          <div class="row mt3">
            <button class="btn btn-primary btn-lg" id="eGo" ${run || !this.outDir ? "disabled" : ""}>
              取出全部素材
            </button>
          </div>
          <div class="small dim mt3">
            没声音、没反应时先别急着点第二次：大游戏（几个 GB）第一次会比较慢。
          </div>
        </div>
      </div>

      ${this.result ? `<div class="seg">${this.lastKind === "repack" ? this.repackResultHtml() : this.resultHtml()}</div>` : ""}
      ${this.err ? `<div class="seg">${noteHtml(this.err)}</div>` : ""}

      <div class="seg">${this.advancedHtml()}</div>
    `;

    // 每个都判空再绑 —— 这几行是「render 内联绑定」，一旦某个 id 不存在就抛异常，
    // 后面所有绑定会被整块跳过（表现为「一堆按钮点了没反应」），所以别依赖「模板一定有」。
    const pick = $("#ePick", this.root);
    if (pick) pick.addEventListener("click", () => this.pickOut());
    const go = $("#eGo", this.root);
    if (go) go.addEventListener("click", () => this.run("extract"));

    const open = $("#eOpen", this.root);
    if (open) open.addEventListener("click", () => this.openFolder(this.result.out_dir));
    const again = $("#eAgain", this.root);
    if (again) again.addEventListener("click", () => this.run("extract"));
    const cancel = $("#eCancel", this.root);
    if (cancel) cancel.addEventListener("click", () => this.cancel());
    const openGame = $("#eOpenGame", this.root);
    if (openGame) openGame.addEventListener("click", () => this.openFolder(Store.gameRoot));

    const rPick = $("#rPick", this.root);
    if (rPick) rPick.addEventListener("click", () => this.pickRepackSrc());
    const rAck = $("#rAck", this.root);
    if (rAck) {
      rAck.addEventListener("change", (e) => {
        this.repackAck = !!e.target.checked;
        this.paintRepack();
      });
    }
    const rGo = $("#rGo", this.root);
    if (rGo) rGo.addEventListener("click", () => this.run("repack"));

    if (run) this.paintProgress(this.prog || { frac: 0, msg: "正在准备…" });

    // 备份列表：初始只有一个「查看备份」按钮，点了才读盘。
    // 结果卡片里有一个、进阶折叠里也有一个（重启后没有结果卡片，折叠里那个才是入口）。
    const bkOpts = {
      note: "原来打包文件里的内容会原样写回，备份本身留着，可以反复还原。",
      emptyNote: "这个游戏目录里还没有备份（重新打包过一次之后就有了）。",
    };
    const g = $("#bkGame", this.root);
    if (g) mountBackups(g, Store.gameRoot, bkOpts);
    const gf = $("#bkGameFold", this.root);
    if (gf) mountBackups(gf, Store.gameRoot, bkOpts);
  },

  renderNeedGame() {
    this.root.innerHTML = `<div class="seg">${emptyHtml(
      "①",
      "还没选游戏",
      "得先让 STool 知道游戏在哪，才能从里面取东西。",
      "去选游戏",
      "eGoDetect"
    )}</div>`;
    $("#eGoDetect", this.root).addEventListener("click", () => {
      location.hash = "#detect";
    });
  },

  resultHtml() {
    const r = this.result;
    return `
      <div class="card">
        <div class="card-head">
          <div class="card-title">取出 ${r.files_done} 个文件</div>
          <div class="card-hint mono">${esc(shortPath(r.out_dir))}</div>
        </div>
        <div class="row mt2">
          <button class="btn btn-primary" id="eOpen">打开输出文件夹</button>
          <button class="btn btn-ghost" id="eAgain">再取一次</button>
        </div>
        <div class="small dim mt3">
          接下来常见做法：去「看素材」浏览这些文件；或者去「翻译文字」处理脚本。
        </div>
      </div>
    `;
  },

  /**
   * 重新打包的结果卡片。和「取出素材」的结果长得不一样：
   * 这里没有输出目录可打开，重点是「写回游戏了，怎么退回去」。
   */
  repackResultHtml() {
    const r = this.result;
    return `
      <div class="card">
        <div class="card-head">
          <div class="card-title">重新打包完成</div>
          <div class="card-hint">已写回游戏目录</div>
        </div>
        <div class="note info mt2">
          原来的打包文件在<strong>第一次</strong>打包之前就已经备份成
          <span class="mono">&lt;文件名&gt;.stool.bak</span>。<br>
          想退回原来的样子，就在下面点「还原」。
        </div>
        <div class="mt3" id="bkGame"></div>
        <div class="row mt2">
          <button class="btn btn-ghost" id="eOpenGame">打开游戏目录</button>
        </div>
        <div class="small dim mt3">${esc(clip(r.message, 200))}</div>
      </div>
    `;
  },

  /** 进阶：封包回写。会写游戏目录，所以默认收起 + 要勾选确认。 */
  advancedHtml() {
    const can = !!this.repackSrc && this.repackAck && !this.running;
    return foldHtml(
      "进阶：把改过的文件塞回游戏（重新打包）",
      `<div class="note warn">
         这个动作会<strong>写回游戏目录</strong>。写之前 STool 会自动把原来的打包文件备份成
         <span class="mono">&lt;文件名&gt;.stool.bak</span>，出问题可以拿它还原。<br>
         大多数人用不到这一步 —— 只有你改了素材、想让游戏真的用上这些改动时才需要。
       </div>
       <div class="row mt3">
         <input class="field mono grow" id="rSrc" readonly
                value="${esc(this.repackSrc)}" placeholder="选源目录（一般是上面解出来的那个）">
         <button class="btn btn-ghost" id="rPick" ${this.running ? "disabled" : ""}>选择源目录</button>
       </div>
       <label class="row tight mt3 small" style="cursor:pointer">
         <input type="checkbox" id="rAck" ${this.repackAck ? "checked" : ""}>
         <span>我知道它会写回游戏目录（已自动备份，可还原）</span>
       </label>
       <div class="row mt3">
         <button class="btn btn-danger" id="rGo" ${can ? "" : "disabled"}>重新打包</button>
       </div>

       <div class="card-head mt3">
         <div class="card-title">备份与还原</div>
         <div class="card-hint">打包前自动留的底，想退回原来的打包文件就在这里</div>
       </div>
       <div id="bkGameFold" class="mt2"></div>`
    );
  },

  /** 只重画「重新打包」的可用状态，不整页重绘（否则折叠会被合上）。 */
  paintRepack() {
    const b = $("#rGo", this.root);
    if (b) b.disabled = !(this.repackSrc && this.repackAck && !this.running);
  },

  // -- 进度 -----------------------------------------------------------------

  progressHtml() {
    return `
      <div class="mt3" id="eProgBody">
        <div class="row tight">
          <span class="small dim2" data-msg style="flex:1 1 auto;min-width:0">正在准备…</span>
          <span class="small mono" data-pct>0%</span>
        </div>
        <div class="bar mt2"><i style="width:0%"></i></div>
        <div class="row mt2">
          <button class="btn btn-ghost btn-sm" id="eCancel">取消</button>
        </div>
      </div>`;
  },

  /** 进度事件到了只更新几个节点，不重绘整页（重绘会闪、也会丢焦点）。 */
  paintProgress(p) {
    this.prog = p;
    const host = $("#eProg", this.root);
    if (!host || !host.firstElementChild) return;
    const pct = Math.max(0, Math.min(100, Math.round((p.frac || 0) * 100)));
    const bar = host.querySelector(".bar > i");
    if (bar) bar.style.width = pct + "%";
    const pv = host.querySelector("[data-pct]");
    if (pv) pv.textContent = pct + "%";
    const mv = host.querySelector("[data-msg]");
    if (mv) mv.textContent = clip(p.msg || "…", 64);
  },

  // -- 动作 -----------------------------------------------------------------

  async pickOut() {
    const r = await Tauri.call("pick_folder", { title: "选择输出目录" });
    if (!r.ok) return toast(r.err, "err");
    if (!r.data) return; // 取消
    this.outDir = r.data;
    await this.render();
  },

  async pickRepackSrc() {
    const r = await Tauri.call("pick_folder", { title: "选择要打回去的源目录" });
    if (!r.ok) return toast(r.err, "err");
    if (!r.data) return;
    this.repackSrc = r.data;
    await this.render();
  },

  async openFolder(p) {
    const r = await Tauri.call("open_folder", { path: p });
    if (!r.ok) toast(r.err, "err");
  },

  async cancel() {
    await Tauri.call("cancel_task");
    toast("已请求取消，会在当前文件写完后停下。");
  },

  /** `kind` 为空 = 取出素材；"repack" = 重新打包。 */
  async run(kind) {
    if (this.running) return;
    if (kind === "repack" && !this.repackAck) return;

    this.running = true;
    this.prog = { frac: 0, msg: "正在准备…" };
    this.result = null;
    this.err = "";
    await this.render();

    // 先订阅再调用：否则任务太快、头几个进度事件会丢。
    await this.subscribe();

    const cmd = kind === "repack" ? "repack_assets" : "extract_assets";
    const args = kind === "repack" ? { srcDir: this.repackSrc } : { outDir: this.outDir };
    const r = await Tauri.call(cmd, args);

    this.running = false;
    if (!r.ok) {
      this.err = r.err;
      log(`extract: ${cmd} 失败 ${r.err}`);
    } else if (!r.data.success) {
      this.err = r.data.message;
      log(`extract: ${cmd} 内核判失败 ${r.data.message}`);
    } else {
      this.result = r.data;
      this.lastKind = kind;
      log(`extract: ${cmd} 完成 files=${r.data.files_done} out=${r.data.out_dir}`);
    }
    await this.render();
    if (this.result) toast(kind === "repack" ? "重新打包完成。" : `完成：取出 ${this.result.files_done} 个文件。`);
    else if (this.err) toast("没成功，原因见页面上的提示。", "err");
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
