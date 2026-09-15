/* ---------------------------------------------------------------------------
   ⑥ 游戏里改数值。

   这一页要满足 soon 占位页承诺的三件事：
     1. 按名字改（推荐）—— MV/MZ 调试协议，直接改金币 / 变量 / 开关 / 物品；
     2. 搜数值改 —— 通用内存扫描（Cheat Engine 等价），任何引擎都能用；
     3. 进阶：反修改保护诊断、补丁包。

   两条硬规矩（都是实测踩出来的）：
   * **文案对齐 Cheat Engine**：数值类型说「4 字节 / 8 字节 / 小数 / 文本」，
     不说内核的「整数 32 位」—— 目标用户是从 CE 过来的（类型表由后端给，
     展示文案在 `cmd::value_types`，界面对齐即可）。
   * **锁值不在后端开线程**：后端只把 `frozen` 置位，真正的周期写入由本页的
     `setInterval` 驱动 `runtime_freeze_tick`。这样切走页面 / 关页面锁值自然停，
     不会留下一个在后台悄悄写内存的孤儿线程。
--------------------------------------------------------------------------- */

const RuntimePage = {
  title: "游戏里改数值",
  sub: "游戏开着也能改，改完立刻生效，不用退出重开。",

  root: null,

  // 通用选项表（类型 / 过滤），进页面取一次
  opts: null,
  optsErr: "",

  // ---- 方式一：MV/MZ ----
  mv: null, // MvmzStatus
  mvState: null, // MvmzState（连上之后）
  mvKind: "gold", // gold | variable | switch | item
  mvId: "",
  mvVal: "",
  mvBusy: false,
  mvMsg: "",
  mvFilter: "", // 变量一览的过滤词

  // ---- 方式二：通用内存扫描 ----
  procs: [],
  procFilter: "",
  showProcs: false,
  pid: 0,
  ty: "i32",
  scanValue: "",
  filter: "exact",
  scan: null, // ScanState
  scanBusy: false,
  scanMsg: "",
  riskAck: false,
  freezeTimer: null,
  freezeValue: "",

  // ---- 方式二·五：诊断 ----
  diagAddr: "",
  diagProbe: "",
  diag: null,
  diagBusy: false,
  diagMsg: "",
  regions: null,
  regionsBusy: false,
  regionsMsg: "",

  // ---- 方式三：补丁包 ----
  patch: null,
  patchSrc: "",
  patchName: "",
  patchBusy: false,
  patchMsg: "",
  patchChangedOnly: false,

  unlisten: null,

  actions() {
    return "";
  },

  async mount(host) {
    this.root = host;
    // 每次进页面都把上一轮的临时状态清掉；会话本身留在后端（用户可能只是切走看别的）
    this.mvMsg = "";
    this.scanMsg = "";
    this.diagMsg = "";
    this.regionsMsg = "";
    this.patchMsg = "";
    this.optsErr = "";

    if (!Store.gameRoot) return this.renderNeedGame();

    await this.render(); // 先画骨架
    await this.loadOptions();
    // 有会话就把现状读回来（切走再回来不该丢进度）
    const st = await Tauri.call("runtime_state");
    if (st.ok) this.scan = st.data.active ? st.data : null;
    if (this.scan && this.scan.frozen) this.startFreezeTimer();
    await this.loadMvmzStatus();
    await this.loadPatchList();
    await this.render();
    return this;
  },

  unmount() {
    // 切走页面 = 停锁值心跳（后端会话本身保留，回来还能接着用）
    this.stopFreezeTimer();
    if (this.unlisten) {
      try {
        this.unlisten();
      } catch (_) {}
      this.unlisten = null;
    }
  },

  // -- 数据 -----------------------------------------------------------------

  async loadOptions() {
    const r = await Tauri.call("runtime_options");
    if (!r.ok) {
      this.optsErr = r.err;
      log(`runtime: runtime_options 失败 ${r.err}`);
      return;
    }
    this.opts = r.data;
    log(`runtime: types=${r.data.types.length} filters=${r.data.filters.length}`);
  },

  async loadMvmzStatus() {
    const r = await Tauri.call("runtime_mvmz_status");
    if (!r.ok) {
      this.mv = { applicable: false, note: r.err, exe: "", port_open: false, port: 0 };
      return;
    }
    this.mv = r.data;
  },

  async loadPatchList() {
    const r = await Tauri.call("runtime_patch_list");
    if (!r.ok) {
      this.patch = { supported: false, note: r.err, patches: [], next_name: "" };
      return;
    }
    this.patch = r.data;
    if (!this.patchName && r.data.next_name) this.patchName = r.data.next_name;
  },

  // -- 渲染 -----------------------------------------------------------------

  async render() {
    if (!Store.gameRoot) return this.renderNeedGame();
    if (!this.opts) {
      this.root.innerHTML = `
        <div class="seg"><div class="card">
          <div class="row tight"><span class="spin"></span>
            <span class="small dim2">正在读取数值类型…</span></div>
        </div></div>
        ${
          this.optsErr
            ? `<div class="seg">${noteHtml(this.optsErr)}</div>`
            : ""
        }`;
      return;
    }
    this.root.innerHTML = [
      this.introHtml(),
      this.mvCardHtml(),
      this.scanCardHtml(),
      this.advanceHtml(),
    ].join("");
    this.bind();
    this.paintFreezeState();
  },

  renderNeedGame() {
    this.root.innerHTML = `<div class="seg">${emptyHtml(
      "⑥",
      "还没选游戏",
      "改运行中的内存要先知道改哪个进程，所以得先告诉 STool 是哪个游戏。",
      "去选游戏",
      "rtGoDetect"
    )}</div>`;
    const b = $("#rtGoDetect", this.root);
    if (b) b.addEventListener("click", () => (location.hash = "#detect"));
  },

  introHtml() {
    return `
      <div class="seg">
        <div class="say">
          <span class="say-mark">?</span>
          <div class="say-body">
            游戏开着也能改，<strong>改完立刻生效</strong>，不用退出重开、也不用碰存档文件。<br>
            两条路：<strong>按名字改</strong>（RPG Maker MV/MZ，最省事）和
            <strong>搜数值改</strong>（任何引擎都能用，写法跟 Cheat Engine 一样）。
          </div>
        </div>
      </div>`;
  },

  // ---- 方式一：按名字改（推荐）------------------------------------------

  mvCardHtml() {
    const m = this.mv;
    if (!m) {
      return this.cardWrap(
        "① 按名字改（推荐）",
        "RPG Maker MV / MZ 专用",
        `<div class="row tight"><span class="spin"></span><span class="small dim2">正在探测…</span></div>`
      );
    }
    if (!m.applicable) {
      // 「不支持」也要给出路：指向下面的通用内存扫描
      return this.cardWrap(
        "① 按名字改（推荐）",
        "这个游戏用不了",
        `<div class="note info">${esc(m.note)}</div>`
      );
    }
    const s = this.mvState;
    const run = this.mvBusy;
    return this.cardWrap(
      "① 按名字改（推荐）",
      "RPG Maker MV / MZ 专用",
      `
      ${
        !s
          ? `<div class="note ${m.port_open ? "ok" : "info"}">${esc(m.note)}</div>
             <div class="row mt3">
               <button class="btn btn-primary" id="mvConnect" ${run ? "disabled" : ""}>
                 ${run ? "连接中…" : "启动并连接"}
               </button>
               <button class="btn btn-ghost" id="mvConnectOnly" ${run ? "disabled" : ""}>只连接</button>
             </div>
             <div class="small dim mt2">
               已经自己把游戏开着了、只是没带调试参数？那就先关掉游戏，再点「启动并连接」。
             </div>`
          : this.mvEditorHtml(s, run)
      }
      ${this.mvMsg ? `<div class="small mt3 ${this.mvMsgErr ? "text-danger" : "dim"}">${esc(this.mvMsg)}</div>` : ""}
      `
    );
  },

  mvEditorHtml(s, run) {
    if (!s.ready) {
      return `
        <div class="note info">${esc(s.note)}</div>
        <div class="row mt3">
          <button class="btn btn-ghost" id="mvRefresh" ${run ? "disabled" : ""}>刷新</button>
          <button class="btn btn-ghost" id="mvDisconnect" ${run ? "disabled" : ""}>断开</button>
        </div>`;
    }

    const kindOpt = [
      ["gold", "金币"],
      ["variable", "变量"],
      ["switch", "开关"],
      ["item", "物品数量"],
    ]
      .map(
        ([k, label]) =>
          `<option value="${k}" ${this.mvKind === k ? "selected" : ""}>${label}</option>`
      )
      .join("");

    const needId = this.mvKind !== "gold";
    const switchHint =
      this.mvKind === "switch"
        ? `<span class="small dim2">（true = 开 / false = 关）</span>`
        : "";

    return `
      <div class="row tight">
        <span class="pill high">已连接</span>
        <span class="tag">${esc(s.note)}</span>
      </div>

      <div class="row mt3">
        <label class="small" style="min-width:64px">改什么</label>
        <select class="field" id="mvKind" ${run ? "disabled" : ""} style="max-width:150px">${kindOpt}</select>
        ${
          needId
            ? `<label class="small" style="min-width:48px">编号</label>
               <input class="field mono" id="mvId" style="max-width:110px"
                      value="${esc(this.mvId)}" placeholder="12" ${run ? "disabled" : ""}>`
            : ""
        }
        <label class="small" style="min-width:40px">新值</label>
        <input class="field mono" id="mvVal" style="max-width:150px"
               value="${esc(this.mvVal)}" placeholder="99999" ${run ? "disabled" : ""}>
        ${switchHint}
        <button class="btn btn-primary" id="mvWrite" ${run ? "disabled" : ""}>
          ${run ? "写入中…" : "写入游戏"}
        </button>
      </div>

      <div class="row mt3">
        <button class="btn btn-ghost btn-sm" id="mvRefresh" ${run ? "disabled" : ""}>刷新数据</button>
        <button class="btn btn-ghost btn-sm" id="mvDisconnect" ${run ? "disabled" : ""}>断开</button>
      </div>

      <div class="mt4">${this.mvListsHtml(s)}</div>`;
  },

  /** 变量 / 开关 / 物品一览。点「选」把 id + 值填进上面的编辑器。 */
  mvListsHtml(s) {
    const f = this.mvFilter.trim().toLowerCase();
    const hit = (name, id) =>
      !f || name.toLowerCase().includes(f) || String(id).includes(f);

    // 变量：数量可能上千，这里按过滤词截断显示（前端不做全量 DOM）
    const vars = s.variables.filter((v) => hit(v.name, v.id));
    const LIMIT = 60;
    const varRows = vars
      .slice(0, LIMIT)
      .map(
        (v) => `<div class="defrow">
          <dt class="mono">#${v.id}</dt>
          <dd>
            <span class="mono">${esc(v.value)}</span>
            ${v.name ? `<span class="dim2 small"> · ${esc(v.name)}</span>` : ""}
            <button class="btn btn-ghost btn-sm" data-pick-var="${v.id}">选</button>
          </dd>
        </div>`
      )
      .join("");

    const items = s.items.filter((i) => hit(i.name, i.id));
    const itemRows = items
      .slice(0, LIMIT)
      .map(
        (i) => `<div class="defrow">
          <dt class="mono">#${i.id}</dt>
          <dd>
            <span class="mono">× ${i.count}</span>
            ${i.name ? `<span class="dim2 small"> · ${esc(i.name)}</span>` : ""}
            <button class="btn btn-ghost btn-sm" data-pick-item="${i.id}" data-pick-count="${i.count}">选</button>
          </dd>
        </div>`
      )
      .join("");

    const swRows = s.switches
      .filter((x) => hit(x.name, x.id))
      .slice(0, LIMIT)
      .map(
        (x) => `<div class="defrow">
          <dt class="mono">#${x.id}</dt>
          <dd>
            <span class="pill high">开</span>
            ${x.name ? `<span class="dim2 small"> · ${esc(x.name)}</span>` : ""}
            <button class="btn btn-ghost btn-sm" data-pick-sw="${x.id}">选</button>
          </dd>
        </div>`
      )
      .join("");

    const more = (arr) =>
      arr.length > LIMIT ? `<div class="small dim2">…还有 ${arr.length - LIMIT} 个，用上面的过滤词缩小范围</div>` : "";

    return `
      <div class="row tight">
        <input class="field grow" id="mvFilter" value="${esc(this.mvFilter)}"
               placeholder="过滤：变量名 / 开关名 / 编号">
      </div>

      <div class="mt3">
        ${foldHtml(
          `变量（${vars.length} 个，点「选」填入编辑器）`,
          varRows + more(vars),
          true
        )}
        ${foldHtml(
          `开关·已开启（${s.switches.filter((x) => hit(x.name, x.id)).length} 个）`,
          swRows + more(s.switches.filter((x) => hit(x.name, x.id)))
        )}
        ${foldHtml(
          `持有物品（${items.length} 种）`,
          itemRows + more(items)
        )}
      </div>`;
  },

  // ---- 方式二：通用内存扫描 ----------------------------------------------

  scanCardHtml() {
    const st = this.scan;
    const run = this.scanBusy;
    const tyOpts = this.opts.types
      .map((t) => {
        // 标签已经写了字节数（「4 字节」「小数（8 字节）」）时不再重复拼，
        // 否则会显示成「4 字节（4 字节）」；文本类型没有字节数，只显示标签。
        const showBytes = t.bytes > 0 && !/字节/.test(t.label);
        return (
          `<option value="${esc(t.key)}" ${this.ty === t.key ? "selected" : ""}>` +
          `${esc(t.label)}${showBytes ? `（${t.bytes} 字节）` : ""}</option>`
        );
      })
      .join("");
    const curTy = this.opts.types.find((t) => t.key === this.ty) || this.opts.types[0];
    const filOpts = this.opts.filters
      .map(
        (f) =>
          `<option value="${esc(f.key)}" ${this.filter === f.key ? "selected" : ""}>${esc(f.label)}</option>`
      )
      .join("");

    const body = `
      <div class="note info">
        用法跟 Cheat Engine 一样：记住游戏里当前的数值（比如金币 <span class="mono">100</span>）→
        <strong>首次扫描</strong> → 回游戏让数值变化 → <strong>再次扫描</strong>（选「变了 / 变大了 / 变小了」）→
        剩下的地址里写新值。数值类型通常先用「4 字节」。
      </div>

      <div class="row mt3">
        <label class="small" style="min-width:64px">目标进程</label>
        <input class="field grow" id="scProcFilter" value="${esc(this.procFilter)}"
               placeholder="输入游戏名过滤，例如 game">
        <button class="btn btn-ghost" id="scProcs" ${run ? "disabled" : ""}>刷新进程列表</button>
      </div>

      ${this.procListHtml()}

      <div class="row mt3">
        <label class="small" style="min-width:64px">数值类型</label>
        <select class="field" id="scTy" style="max-width:220px" ${run ? "disabled" : ""}>${tyOpts}</select>
        <span class="small dim2">${esc(curTy ? curTy.hint : "")}</span>
      </div>

      <div class="row mt3">
        <label class="small" style="min-width:64px">数值</label>
        <input class="field mono" id="scValue" style="max-width:160px"
               value="${esc(this.scanValue)}" placeholder="${this.ty === "i64" ? "999999999" : "100"}"
               ${run ? "disabled" : ""}>
        <button class="btn btn-primary" id="scFirst" ${run || !this.scan ? "disabled" : ""}>
          ${run ? "扫描中…" : "首次扫描"}
        </button>
        <label class="small" style="min-width:40px">过滤</label>
        <select class="field" id="scFilter" style="max-width:130px" ${run ? "disabled" : ""}>${filOpts}</select>
        <button class="btn" id="scNext" ${run || !this.scan || !this.scan.first_done ? "disabled" : ""}>
          再次扫描
        </button>
      </div>

      ${this.scanResultHtml(st, run)}
    `;

    return this.cardWrap("② 搜数值改", "任何引擎都能用 · 写法同 Cheat Engine", body);
  },

  procListHtml() {
    if (!this.showProcs) return "";
    const f = this.procFilter.trim().toLowerCase();
    const list = this.procs.filter((p) => !f || p.name.toLowerCase().includes(f) || String(p.pid).includes(f));
    if (list.length === 0) {
      return `<div class="note warn mt3">没有匹配的进程。
        <div class="small dim2 mt2">游戏开着的吗？名字记不全就只输一小段（例如 <span class="mono">gam</span>）。
        实在找不到，可以先在任务管理器里看游戏进程叫什么。</div>
        <div class="small dim2">若列表整体为空，点「刷新进程列表」重扫一次。</div>
      </div>`;
    }
    const rows = list
      .slice(0, 40)
      .map(
        (p) => `<div class="bakrow">
          <span class="mono small grow" title="${esc(p.label)}">${esc(p.label)}</span>
          <button class="btn btn-ghost btn-sm" data-pid="${p.pid}">选它</button>
        </div>`
      )
      .join("");
    const more = list.length > 40 ? `<div class="small dim2">…还有 ${list.length - 40} 个，用过滤词缩小范围</div>` : "";
    return `<div class="mt3">${rows}${more}</div>`;
  },

  scanResultHtml(st, run) {
    if (!st) {
      return `<div class="small dim2 mt3">
        还没选进程。选好之后按钮才会亮 —— 进程列表要手动刷一次才有内容。</div>`;
    }
    const ro =
      st.readonly_hits > 0
        ? `<div class="note warn mt2">
             有 <strong>${st.readonly_hits}</strong> 处命中所在页不可直接写（只读 / Guard）：
             普通「写入」会失败，请勾「强制写入」再写。
           </div>`
        : "";

    const rows = st.list
      .map(
        (h) => `<div class="bakrow">
          <span class="mono small grow">${esc(h.addr)}</span>
          <span class="mono small">${esc(h.value)}</span>
          <button class="btn btn-ghost btn-sm" data-diag="${esc(h.addr)}">诊断</button>
        </div>`
      )
      .join("");
    const trunc = st.truncated
      ? `<div class="small dim2">只显示前 ${st.list.length} 条（共 ${st.hits} 条）—— 用「再次扫描」把范围缩小。</div>`
      : "";

    return `
      <div class="row tight mt4">
        <span class="pill ${st.frozen ? "high" : "medium"}">${esc(st.proc_name || "已选进程")}</span>
        <span class="tag">PID ${st.pid}</span>
        <span class="tag">${esc(st.ty_label)}</span>
        <span class="tag">命中 ${st.hits}</span>
      </div>
      ${ro}

      <div class="row mt3">
        <label class="small" style="min-width:64px">写入值</label>
        <input class="field mono" id="scFreezeValue" style="max-width:160px"
               value="${esc(this.freezeValue || this.scanValue)}" placeholder="99999">
        <button class="btn btn-primary" id="scWriteAll"
                ${run || !st.first_done || !this.riskAck ? "disabled" : ""}>写入所有命中</button>
        <button class="btn btn-danger" id="scForceAll"
                ${run || !st.first_done || !this.riskAck ? "disabled" : ""}>强制写入（解除页保护）</button>
      </div>

      <div class="row mt3">
        <button class="btn" id="scFreeze" ${run || !st.first_done || !this.riskAck ? "disabled" : ""}>
          ${st.frozen ? "解锁数值" : "锁定数值"}
        </button>
        <button class="btn btn-ghost" id="scUndo" ${run || !st.can_undo ? "disabled" : ""}>撤销写入</button>
        <button class="btn btn-ghost" id="scRefresh" ${run ? "disabled" : ""}>刷新命中值</button>
        <span class="small dim2" id="scFreezeTag">${
          st.frozen ? `● 已锁定（每 ${st.freeze_period_ms} 毫秒写回一次）` : ""
        }</span>
      </div>

      <label class="row tight mt3 small" style="cursor:pointer">
        <input type="checkbox" id="scAck" ${this.riskAck ? "checked" : ""}>
        <span>我已了解写入内存的风险（可能让游戏状态或当前存档异常）；勾一次之后不用再确认</span>
      </label>

      ${this.scanMsg ? `<div class="small mt3 ${this.scanMsgErr ? "text-danger" : "dim"}">${esc(this.scanMsg)}</div>` : ""}

      <div class="mt3">
        ${st.first_done ? foldHtml(`命中列表（${st.hits} 条）`, rows + trunc, true) : `<div class="small dim2">还没扫描。选好进程、填上游戏里当前的数值，点「首次扫描」。</div>`}
      </div>`;
  },

  // ---- 方式二·五 + 方式三：进阶 ------------------------------------------

  advanceHtml() {
    const st = this.scan;
    const canDiag = !!st && st.first_done;
    const d = this.diag;
    const run = this.diagBusy || this.regionsBusy;

    const facts = d
      ? `<dl style="margin:0" class="mt3">
          <div class="defrow"><dt>结论</dt><dd>${esc(d.headline)}</dd></div>
          <div class="defrow"><dt>回滚判定</dt>
            <dd><span class="pill ${d.reverted ? "danger" : "high"}">${esc(d.verdict)}</span></dd></div>
          <div class="defrow"><dt>页保护</dt>
            <dd>${esc(d.page)}${d.page_blocked ? ` <span class="pill medium">不可直接写</span>` : ""}</dd></div>
          <div class="defrow"><dt>同值副本</dt>
            <dd>${d.mirrors} 个${d.mirror_truncated ? "（已截断）" : ""}</dd></div>
          ${d.rewrite ? `<div class="defrow"><dt>写入被改写</dt><dd>${esc(d.rewrite)}</dd></div>` : ""}
          ${
            d.source_addr
              ? `<div class="defrow"><dt>数据源</dt><dd class="mono">${esc(d.source_addr)}</dd></div>`
              : ""
          }
          ${
            d.writers.length
              ? `<div class="defrow"><dt>还原指令落点</dt>
                   <dd class="mono small">${d.writers.map(esc).join("  ")}</dd></div>`
              : ""
          }
          ${
            d.modules.length
              ? `<div class="defrow"><dt>保护机制</dt>
                   <dd>${d.modules
                     .map((m) => `<span class="tag">${esc(m.name)}（${esc(m.family)}）</span>`)
                     .join(" ")}</dd></div>`
              : ""
          }
          ${
            d.anti_debug.length
              ? `<div class="defrow"><dt>反调试导入</dt>
                   <dd class="small dim2">${d.anti_debug.slice(0, 4).map(esc).join(" · ")}</dd></div>`
              : ""
          }
          ${
            d.read_only
              ? `<div class="defrow"><dt>提示</dt>
                   <dd class="text-danger">只读模式：没拿到可写句柄，未做写入探测。请以管理员身份运行 STool 后重试。</dd></div>`
              : ""
          }
        </dl>
        <div class="mt3"><div class="small dim2">应对方案（按可行性排序）</div>
          ${
            d.plans.length
              ? d.plans.map((p, i) => this.planHtml(p, i)).join("")
              : `<div class="small dim2 mt2">没有生成方案 —— 通常说明该地址本来就是普通可写内存，直接写就行。</div>`
          }
        </div>`
      : "";

    const regionsBlock = this.regions
      ? `<div class="mt3">
          <div class="row tight">
            <span class="tag">可写 ${this.regions.writable}</span>
            <span class="tag">只读 ${this.regions.readonly}</span>
            <span class="tag">可执行 ${this.regions.execute}</span>
            <span class="tag">可写可执行 ${this.regions.rwx}</span>
            <span class="tag">Guard ${this.regions.guard}</span>
            <span class="tag">共 ${esc(this.regions.total_bytes)}</span>
          </div>
          ${
            this.regions.rwx > 0
              ? `<div class="note warn mt2">有 ${this.regions.rwx} 个「可写且可执行」页（正常程序极少）——
                   常见于加壳或自修改代码。</div>`
              : ""
          }
          <div class="mt2">
            ${foldHtml(
              `区域明细（${this.regions.total_rows} 条，显示前 ${this.regions.rows.length}）`,
              `<table class="tbl"><thead><tr><th>基址</th><th>大小</th><th>保护</th><th>类型</th><th>合并</th></tr></thead>
               <tbody>${this.regions.rows
                 .map(
                   (r) =>
                     `<tr><td class="mono small">${esc(r.base)}</td><td class="small">${esc(r.size)}</td>
                      <td class="small">${esc(r.protect)}</td><td class="small">${esc(r.kind)}</td>
                      <td class="small">${r.merged}</td></tr>`
                 )
                 .join("")}</tbody></table>`
            )}
          </div>
        </div>`
      : "";

    const p = this.patch;
    const patchBlock = !p
      ? `<div class="row tight"><span class="spin"></span><span class="small dim2">正在读取补丁包…</span></div>`
      : !p.supported
      ? `<div class="note info">${esc(p.note)}</div>`
      : `
        <div class="small dim2">
          游戏按 <span class="mono">Data 目录 → data.xp3 → patch.xp3 → patch2.xp3 → …</span> 搜索资源，
          越靠后优先级越高。把改好的文件（保持封包内原始相对路径）打成下一个空号的
          <span class="mono">patchN.xp3</span>，引擎就会优先加载它 ——
          几 GB 的 data.xp3 一个字节都不用动，删掉即还原。
        </div>
        <div class="row mt3">
          <label class="small" style="min-width:64px">改动目录</label>
          <input class="field mono grow" id="ptSrc" value="${esc(this.patchSrc)}"
                 placeholder="取出素材的输出目录">
          <button class="btn btn-ghost" id="ptPick" ${this.patchBusy ? "disabled" : ""}>选择目录</button>
        </div>
        <div class="row mt3">
          <label class="small" style="min-width:64px">补丁包名</label>
          <input class="field mono" id="ptName" style="max-width:180px" value="${esc(this.patchName)}"
                 placeholder="${esc(p.next_name)}">
          <button class="btn btn-primary" id="ptBuild" ${this.patchBusy ? "disabled" : ""}>
            ${this.patchBusy ? "打包中…" : "生成补丁包"}
          </button>
        </div>
        <label class="row tight mt2 small" style="cursor:pointer">
          <input type="checkbox" id="ptChanged" ${this.patchChangedOnly ? "checked" : ""}>
          <span>只打包改动（自动和原封包逐字节比对，未改动的不占体积）</span>
        </label>
        ${
          p.patches.length
            ? `<div class="mt3"><div class="small dim2">已有补丁包</div>
                ${p.patches
                  .map(
                    (x) => `<div class="bakrow">
                      <span class="mono small grow">${esc(x.name)}${x.ours ? " （本工具建的）" : ""}</span>
                      <span class="small dim2">${esc(x.size)}</span>
                      ${
                        x.ours
                          ? `<button class="btn btn-ghost btn-sm" data-patch-rm="${esc(x.name)}">移除</button>`
                          : ""
                      }
                    </div>`
                  )
                  .join("")}</div>`
            : `<div class="small dim2 mt3">还没有补丁包。</div>`
        }
        ${this.patchMsg ? `<div class="small mt3 dim">${esc(this.patchMsg)}</div>` : ""}`;

    return `
      <div class="seg">
        <div class="card">
          <div class="card-head">
            <div class="card-title">③ 进阶：改了没生效怎么办</div>
            <div class="card-hint">平时用不到，出问题才来</div>
          </div>

          <div class="small dim2">
            游戏有反修改保护时，表现为「写了没反应 / 过一瞬又变回去」。
            这里会临时写一个探测值、测它多久被还原（随后立刻恢复原值），再顺带查页保护、
            同值副本、加壳 / 反作弊模块，并在代码段搜「还原指令」的落点。
          </div>

          <div class="row mt3">
            <label class="small" style="min-width:64px">地址</label>
            <input class="field mono" id="dgAddr" style="max-width:200px"
                   value="${esc(this.diagAddr)}" placeholder="0x7FF6A000" ${run ? "disabled" : ""}>
            <label class="small" style="min-width:56px">探测值</label>
            <input class="field mono" id="dgProbe" style="max-width:110px"
                   value="${esc(this.diagProbe)}" placeholder="留空=自动" ${run ? "disabled" : ""}>
            <button class="btn btn-primary" id="dgRun" ${run || !canDiag ? "disabled" : ""}>
              ${this.diagBusy ? "诊断中…" : "诊断保护机制"}
            </button>
            <button class="btn btn-ghost" id="dgRegions" ${run || !this.scan ? "disabled" : ""}>
              ${this.regionsBusy ? "遍历中…" : "页保护分布"}
            </button>
          </div>
          ${
            !canDiag
              ? `<div class="small dim2 mt2">先在上面「② 搜数值改」做一次扫描，地址可以从命中列表点「诊断」自动填过来。</div>`
              : ""
          }
          ${this.diagMsg ? `<div class="small mt2 ${this.diagMsgErr ? "text-danger" : "dim"}">${esc(this.diagMsg)}</div>` : ""}
          ${this.regionsMsg ? `<div class="small mt2 dim">${esc(this.regionsMsg)}</div>` : ""}
          ${facts}
          ${regionsBlock}
        </div>
      </div>

      <div class="seg">
        <div class="card">
          <div class="card-head">
            <div class="card-title">④ 补丁包（KiriKiri）</div>
            <div class="card-hint">不改原封包，删掉即还原</div>
          </div>
          ${patchBlock}
        </div>
      </div>`;
  },

  planHtml(p, i) {
    const cls = { high: "high", medium: "medium", low: "none", none: "danger" }[p.level] || "none";
    const btn = this.planButtonHtml(p);
    return `<div class="mt3">
      <div class="row tight">
        <span class="pill ${cls}">${esc(p.feasibility)}</span>
        <strong class="small">${i + 1}. ${esc(p.title)}</strong>
        ${btn}
      </div>
      <div class="small dim2">${esc(p.detail)}</div>
      ${p.steps.map((s) => `<div class="small dim2">· ${esc(s)}</div>`).join("")}
    </div>`;
  },

  planButtonHtml(p) {
    if (p.auto === "freeze") {
      const ms = p.auto_period_ms || 100;
      return `<button class="btn btn-ghost btn-sm" data-plan="${esc(p.auto)}" data-period="${ms}" data-addr="${esc(p.auto_addr)}">按此周期锁值（${ms} 毫秒）</button>`;
    }
    if (p.auto === "force_write") {
      return `<button class="btn btn-ghost btn-sm" data-plan="force_write" data-addr="${esc(p.auto_addr)}">强制写入该地址</button>`;
    }
    if (p.auto === "write_source") {
      return `<button class="btn btn-ghost btn-sm" data-plan="write_source" data-addr="${esc(p.auto_addr)}">写入这个数据源</button>`;
    }
    return "";
  },

  cardWrap(title, hint, body) {
    return `
      <div class="seg">
        <div class="card">
          <div class="card-head">
            <div class="card-title">${esc(title)}</div>
            ${hint ? `<div class="card-hint">${esc(hint)}</div>` : ""}
          </div>
          ${body}
        </div>
      </div>`;
  },

  // ---- 绑定 ---------------------------------------------------------------

  bind() {
    const on = (id, fn, ev) => {
      const e = $("#" + id, this.root);
      if (e) e.addEventListener(ev || "click", fn);
    };
    const onInput = (id, set) => {
      const e = $("#" + id, this.root);
      if (e) e.addEventListener("input", (ev) => set(ev.target.value));
    };

    // 方式一
    on("mvConnect", () => this.mvConnect(true));
    on("mvConnectOnly", () => this.mvConnect(false));
    on("mvDisconnect", () => this.mvDisconnect());
    on("mvRefresh", () => this.mvRefresh());
    on("mvWrite", () => this.mvWrite());
    const mvKind = $("#mvKind", this.root);
    if (mvKind)
      mvKind.addEventListener("change", async (e) => {
        this.mvKind = e.target.value;
        await this.render(); // 切「金币」时要收起编号框
      });
    onInput("mvId", (v) => (this.mvId = v));
    onInput("mvVal", (v) => (this.mvVal = v));
    onInput("mvFilter", (v) => (this.mvFilter = v));

    // 一览里的「选」
    this.root.querySelectorAll("[data-pick-var]").forEach((b) =>
      b.addEventListener("click", () => {
        this.mvKind = "variable";
        this.mvId = b.dataset.pickVar;
        this.render();
      })
    );
    this.root.querySelectorAll("[data-pick-item]").forEach((b) =>
      b.addEventListener("click", () => {
        this.mvKind = "item";
        this.mvId = b.dataset.pickItem;
        this.mvVal = b.dataset.pickCount;
        this.render();
      })
    );
    this.root.querySelectorAll("[data-pick-sw]").forEach((b) =>
      b.addEventListener("click", () => {
        this.mvKind = "switch";
        this.mvId = b.dataset.pickSw;
        this.mvVal = "true";
        this.render();
      })
    );

    // 方式二
    on("scProcs", () => this.loadProcs());
    onInput("scProcFilter", (v) => (this.procFilter = v));
    const ty = $("#scTy", this.root);
    if (ty)
      ty.addEventListener("change", async (e) => {
        this.ty = e.target.value;
        // 换类型 = 旧命合作废，后端会新建会话
        if (this.pid) await this.openScanner();
        else await this.render();
      });
    onInput("scValue", (v) => (this.scanValue = v));
    onInput("scFreezeValue", (v) => (this.freezeValue = v));
    const fil = $("#scFilter", this.root);
    if (fil) fil.addEventListener("change", (e) => (this.filter = e.target.value));
    const ack = $("#scAck", this.root);
    if (ack)
      ack.addEventListener("change", async (e) => {
        this.riskAck = !!e.target.checked;
        await this.render(); // 勾了之后「写入」按钮才亮
      });
    on("scFirst", () => this.scanRun(true));
    on("scNext", () => this.scanRun(false));
    on("scWriteAll", () => this.writeAll(false));
    on("scForceAll", () => this.writeAll(true));
    on("scFreeze", () => this.toggleFreeze());
    on("scUndo", () => this.undo());
    on("scRefresh", () => this.refreshHits());

    this.root.querySelectorAll("[data-pid]").forEach((b) =>
      b.addEventListener("click", () => {
        this.pid = +b.dataset.pid;
        this.showProcs = false;
        this.openScanner();
      })
    );
    // 命中列表的「诊断」：填到诊断框并立刻跑
    this.root.querySelectorAll("[data-diag]").forEach((b) =>
      b.addEventListener("click", () => {
        this.diagAddr = b.dataset.diag;
        this.diagRun();
      })
    );

    // 方式二·五
    onInput("dgAddr", (v) => (this.diagAddr = v));
    onInput("dgProbe", (v) => (this.diagProbe = v));
    on("dgRun", () => this.diagRun());
    on("dgRegions", () => this.loadRegions());

    // 方案里的可执行按钮
    this.root.querySelectorAll("[data-plan]").forEach((b) =>
      b.addEventListener("click", () => this.runPlan(b.dataset.plan, b.dataset.period, b.dataset.addr))
    );

    // 方式三
    onInput("ptSrc", (v) => (this.patchSrc = v));
    onInput("ptName", (v) => (this.patchName = v));
    const chg = $("#ptChanged", this.root);
    if (chg) chg.addEventListener("change", (e) => (this.patchChangedOnly = !!e.target.checked));
    on("ptPick", () => this.pickPatchSrc());
    on("ptBuild", () => this.buildPatch());
    this.root.querySelectorAll("[data-patch-rm]").forEach((b) =>
      b.addEventListener("click", () => this.removePatch(b.dataset.patchRm))
    );
  },

  // ---- 方式一动作 ---------------------------------------------------------

  async mvConnect(launch) {
    this.mvBusy = true;
    this.mvMsg = launch ? "正在用调试参数启动游戏并连接…（游戏窗口弹出属正常）" : "正在连接调试端口…";
    this.mvMsgErr = false;
    await this.render();
    const r = await Tauri.call("runtime_mvmz_connect", { launch });
    this.mvBusy = false;
    if (!r.ok) {
      this.mvMsg = r.err;
      this.mvMsgErr = true;
      log(`runtime: mvmz_connect 失败 ${r.err}`);
    } else {
      this.mvState = r.data;
      this.mvMsg = r.data.ready ? "已连上，开始改吧。" : "";
    }
    await this.render();
  },

  async mvDisconnect() {
    await Tauri.call("runtime_mvmz_disconnect");
    this.mvState = null;
    this.mvMsg = "已断开。";
    this.mvMsgErr = false;
    await this.render();
  },

  async mvRefresh() {
    this.mvBusy = true;
    await this.render();
    const r = await Tauri.call("runtime_mvmz_refresh");
    this.mvBusy = false;
    if (!r.ok) {
      this.mvMsg = r.err;
      this.mvMsgErr = true;
    } else {
      this.mvState = r.data;
      this.mvMsg = "";
    }
    await this.render();
  },

  async mvWrite() {
    if (this.mvBusy) return;
    this.mvBusy = true;
    this.mvMsg = "";
    this.mvMsgErr = false;
    await this.render();
    const r = await Tauri.call("runtime_mvmz_set", {
      kind: this.mvKind,
      id: this.mvId ? +this.mvId : 0,
      value: this.mvVal,
    });
    this.mvBusy = false;
    if (!r.ok) {
      this.mvMsg = r.err;
      this.mvMsgErr = true;
      log(`runtime: mvmz_set 失败 ${r.err}`);
    } else {
      this.mvState = r.data;
      this.mvMsg = "已写入 —— 回游戏看一眼，数值应该立刻变了。";
    }
    await this.render();
  },

  // ---- 方式二动作 ---------------------------------------------------------

  async loadProcs() {
    const r = await Tauri.call("runtime_processes", { filter: this.procFilter });
    if (!r.ok) return toast(r.err, "err");
    this.procs = r.data || [];
    this.showProcs = true;
    log(`runtime: 进程 ${this.procs.length} 个`);
    await this.render();
  },

  async openScanner() {
    const r = await Tauri.call("runtime_open", { pid: this.pid, ty: this.ty });
    if (!r.ok) {
      this.scanMsg = r.err;
      this.scanMsgErr = true;
      toast("打开进程失败，原因见页面上提示。", "err");
    } else {
      this.scan = r.data;
      this.scanMsg = "";
      this.scanMsgErr = false;
      log(`runtime: 会话 pid=${r.data.pid} ty=${r.data.ty}`);
    }
    await this.render();
  },

  async scanRun(first) {
    if (this.scanBusy) return;
    if (!this.scanValue.trim()) {
      return toast("先在「数值」里填游戏里现在的数值。", "err");
    }
    this.scanBusy = true;
    this.scanMsg = first ? "首次扫描中 —— 要遍历整个可写内存，稍等…" : "再次扫描中…";
    this.scanMsgErr = false;
    await this.render();
    const r = await Tauri.call("runtime_scan", {
      first,
      value: this.scanValue,
      filter: this.filter,
    });
    this.scanBusy = false;
    if (!r.ok) {
      this.scanMsg = r.err;
      this.scanMsgErr = true;
    } else {
      this.scan = r.data;
      this.scanMsg = `扫描完成：还剩 ${r.data.hits} 处。`;
      if (first) this.freezeValue = this.scanValue;
    }
    await this.render();
  },

  async writeAll(force) {
    if (this.scanBusy) return;
    const v = this.freezeValue || this.scanValue;
    if (!v.trim()) return toast("先在「写入值」里填要写成的数值。", "err");
    this.scanBusy = true;
    this.scanMsg = force ? "正在强制写入（临时解除页保护）…" : "正在写入…";
    this.scanMsgErr = false;
    await this.render();
    const r = await Tauri.call("runtime_write", { value: v, addr: null, force });
    this.scanBusy = false;
    if (!r.ok) {
      this.scanMsg = r.err;
      this.scanMsgErr = true;
      toast("写入失败，原因见页面上提示。", "err");
    } else {
      this.scan = r.data;
      this.scanMsg = force ? "强制写入完成。" : "写入完成 —— 回游戏看一眼。";
    }
    await this.render();
  },

  async undo() {
    this.scanBusy = true;
    await this.render();
    const r = await Tauri.call("runtime_undo");
    this.scanBusy = false;
    if (!r.ok) {
      this.scanMsg = r.err;
      this.scanMsgErr = true;
    } else {
      this.scan = r.data;
      this.scanMsg = "已撤销上一次写入（恢复为写入前的值）。";
      this.scanMsgErr = false;
    }
    await this.render();
  },

  async refreshHits() {
    this.scanBusy = true;
    await this.render();
    const r = await Tauri.call("runtime_state");
    this.scanBusy = false;
    if (r.ok && r.data.active) this.scan = r.data;
    await this.render();
  },

  async toggleFreeze() {
    if (!this.scan) return;
    const on = !this.scan.frozen;
    const v = this.freezeValue || this.scanValue;
    if (on && !v.trim()) return toast("先在「写入值」里填要锁定的数值。", "err");
    const r = await Tauri.call("runtime_freeze", { on, value: v, periodMs: 200 });
    if (!r.ok) return toast(r.err, "err");
    this.scan = r.data;
    if (on) this.startFreezeTimer();
    else this.stopFreezeTimer();
    await this.render();
  },

  /**
   * 锁值心跳。后端只置标志，真正的周期写入由这里驱动 ——
   * 切走页面 / 关页面（`unmount`）就自然停，不留孤儿线程。
   */
  startFreezeTimer() {
    this.stopFreezeTimer();
    const period = (this.scan && this.scan.freeze_period_ms) || 200;
    this.freezeTimer = setInterval(async () => {
      const v = this.freezeValue || this.scanValue;
      if (!v.trim()) return;
      const r = await Tauri.call("runtime_freeze_tick", { value: v });
      if (!r.ok || r.data === false) this.stopFreezeTimer();
    }, Math.max(100, period));
  },

  stopFreezeTimer() {
    if (this.freezeTimer) {
      clearInterval(this.freezeTimer);
      this.freezeTimer = null;
    }
  },

  paintFreezeState() {
    const tag = $("#scFreezeTag", this.root);
    if (!tag) return;
    const on = this.scan && this.scan.frozen;
    tag.textContent = on ? `● 已锁定（每 ${this.scan.freeze_period_ms} 毫秒写回一次）` : "";
  },

  // ---- 诊断动作 -----------------------------------------------------------

  async diagRun() {
    if (this.diagBusy) return;
    if (!this.diagAddr.trim()) return toast("先填一个地址，或从命中列表点「诊断」。", "err");
    this.diagBusy = true;
    this.diagMsg = "诊断中 —— 会临时写入一个探测值（随后立刻恢复原值），稍等…";
    this.diagMsgErr = false;
    await this.render();
    const r = await Tauri.call("runtime_diag", { addr: this.diagAddr, probe: this.diagProbe });
    this.diagBusy = false;
    if (!r.ok) {
      this.diag = null;
      this.diagMsg = r.err;
      this.diagMsgErr = true;
      log(`runtime: diag 失败 ${r.err}`);
    } else {
      this.diag = r.data;
      this.diagMsg = "";
    }
    await this.render();
  },

  async loadRegions() {
    this.regionsBusy = true;
    this.regionsMsg = "正在只读遍历内存区域…";
    await this.render();
    const r = await Tauri.call("runtime_regions");
    this.regionsBusy = false;
    if (!r.ok) {
      this.regionsMsg = r.err;
      this.regions = null;
    } else {
      this.regions = r.data;
      this.regionsMsg = "";
    }
    await this.render();
  },

  /** 方案里的按钮：锁值 / 强制写入某地址 / 写入数据源。 */
  async runPlan(kind, period, addr) {
    const v = this.diagProbe || this.freezeValue || this.scanValue;
    if (kind === "freeze") {
      // 诊断方案里的「锁值」锁的是**诊断过的那个地址**；后端锁的是全部命中，
      // 所以这里如实告知，不假装是单地址锁。
      const r = await Tauri.call("runtime_freeze", {
        on: true,
        value: v,
        periodMs: +(period || 200),
      });
      if (!r.ok) return toast(r.err, "err");
      this.scan = r.data;
      this.freezeValue = v;
      this.startFreezeTimer();
      await this.render();
      toast("已按报告周期开始锁定（锁的是当前所有命中）。");
      return;
    }
    if (kind === "force_write" || kind === "write_source") {
      const r = await Tauri.call("runtime_write", { value: v, addr: addr || null, force: kind === "force_write" });
      if (!r.ok) return toast(r.err, "err");
      this.scan = r.data;
      await this.render();
      toast(kind === "force_write" ? "强制写入完成。" : `已写入数据源 ${addr}。`);
    }
  },

  // ---- 补丁包动作 ---------------------------------------------------------

  async pickPatchSrc() {
    const r = await Tauri.call("pick_folder", { title: "选择改动目录（取出素材的输出目录）" });
    if (!r.ok) return toast(r.err, "err");
    if (!r.data) return;
    this.patchSrc = r.data;
    await this.render();
  },

  async buildPatch() {
    this.patchBusy = true;
    this.patchMsg = "正在打包…";
    await this.render();
    const r = await Tauri.call("runtime_patch_build", {
      srcDir: this.patchSrc,
      name: this.patchName,
      changedOnly: this.patchChangedOnly,
    });
    this.patchBusy = false;
    if (!r.ok) {
      this.patchMsg = r.err;
      toast("打包失败，原因见页面上提示。", "err");
    } else {
      this.patchMsg = r.data;
      toast("补丁包已生成。");
    }
    await this.loadPatchList();
    await this.loadPatchList(); // 保证 next_name 刷新后再渲染一次
    await this.render();
  },

  async removePatch(name) {
    const r = await Tauri.call("runtime_patch_remove", { name });
    if (!r.ok) return toast(r.err, "err");
    this.patchMsg = r.data;
    toast(r.data);
    await this.loadPatchList();
    await this.render();
  },
};
