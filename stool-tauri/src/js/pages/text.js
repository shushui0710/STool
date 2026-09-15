/* ---------------------------------------------------------------------------
   ④ 翻译文字（汉化主流程）。

   这一页有**两条通道**，界面必须让人一眼看出该走哪条，而不是列出六个按钮让人猜
   （docs/TAURI重构方案.md §4.1、§5.2）：

     通道 A · 改动游戏文件（提取 → 翻译 CSV → 回填）
       通用：任何引擎都能走。译文写进游戏资源文件，玩的时候直接是中文。
       代价：改的是游戏目录里的文件（写前自动备份 .stool.bak）。

     通道 B · 不改游戏文件（生成 JSON → 启动时替换文字）
       只支持 Ren'Py / RPG Maker MV-MZ / TyranoBuilder / HTML 游戏。
       不动游戏任何文件，玩游戏时在内存里替换。Ren'Py 兼容性最好。
       引擎不支持时**不当成错误**，而是明说「你可以走通道 A」。

   「机翻」按钮的定位：它是**帮你在 CSV 的译文列里预填一遍**，不是自动全流程。
   填完之后用户仍要（也应该）过目 —— 直接拿没校对过的机翻上，游戏体验会很差。
   所以页面上把这一步写清楚，不给「一键汉化」的错觉。

   内核能力全部在 `stool::features::{translate, inject, tpack, text}`，
   这里不重复实现任何东西（CSV 列约定 / 生效适配表都在内核）。
--------------------------------------------------------------------------- */

/** MTL（机器翻译）服务商预设。base_url 是 OpenAI 兼容端点，多数国内厂商都提供。 */
const MTL_PRESETS = [
  {
    id: "deepseek",
    name: "DeepSeek",
    base: "https://api.deepseek.com/v1",
    model: "deepseek-chat",
    note: "便宜、中文好，最常用。密钥在 platform.deepseek.com 申请。",
  },
  {
    id: "zhipu",
    name: "智谱 GLM",
    base: "https://open.bigmodel.cn/api/paas/v4",
    model: "glm-4-flash",
    note: "glm-4-flash 有免费额度，适合先跑一遍看效果。",
  },
  {
    id: "ollama",
    name: "本地 Ollama",
    base: "http://127.0.0.1:11434/v1",
    model: "qwen2.5:7b",
    note: "完全离线、不花钱，但要自己装 Ollama 并先拉好模型；慢一些。",
  },
];

const TextPage = {
  title: "翻译文字",
  sub: "把游戏里的台词提成表格，翻译完再送回游戏。",

  root: null,
  busy: false,
  err: "",
  toastMsg: "",

  // -- 通道 A：CSV 回填 --
  outDir: "",
  csvPath: "",
  csv: null, // CsvStats
  running: "",
  prog: null,
  result: null,
  unlisten: null,

  // -- 通道 B：不改游戏文件 ----------------------------------------------------
  stats: null, // TextStats
  jsonPath: "",

  // -- MTL 配置 --
  mtlBase: "",
  mtlKey: "",
  mtlModel: "",
  mtlGlossary: "",
  mtlOpen: false,
  mtlPreset: "",
  mtlRunning: false,

  autoTried: false,

  actions() {
    return "";
  },

  async mount(host) {
    this.root = host;
    await this.render();

    if (!this.autoTried) {
      this.autoTried = true;
      // 输出目录与「取出素材」共用一个钩子：汉化流程紧接取素材，
      // 让用户不用再选一次同一个目录。
      const r = await Tauri.call("env_out_dir");
      if (r.ok && r.data && !this.outDir) {
        this.outDir = r.data;
        this.csvPath = joinPath(r.data, "text.csv");
        log(`text: env_out_dir -> ${r.data}`);
        await this.render();
      }
    }

    if (Store.gameRoot) await this.loadStats();
    // 表格统计要在**注入状态之后**取，并在取到后重绘一次：
    // 否则页面停在「还没读表格」，用户得手动点一下才看到条数（看起来像功能没生效）。
    if (this.csvPath) {
      await this.loadCsv();
      await this.render();
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

  // =========================================================================
  // 渲染
  // =========================================================================

  async render() {
    if (!Store.gameRoot) return this.renderNeedGame();

    this.root.innerHTML = `
      <div class="seg">
        <div class="say">
          <span class="say-mark">?</span>
          <div class="say-body">
            汉化分两条路，<strong>能走哪条看引擎</strong>：<br>
            ① 通用路线 —— 把台词提成表格，翻译好再写回游戏；<br>
            ② 引擎支持时更好 —— 生成一张译文表，游戏启动时自动替换，<strong>不动游戏文件</strong>。
          </div>
        </div>
      </div>

      <div class="seg">
        <div class="card flat">
          <div class="row tight">
            <span class="pill high">当前游戏</span>
            <span class="tag">${esc(Store.detect ? Store.detect.engine_name : "（还没识别出来）")}</span>
          </div>
          <div class="verdict-path mt2">${esc(Store.gameRoot)}</div>
        </div>
      </div>

      ${this.err ? `<div class="seg">${noteHtml(this.err)}</div>` : ""}
      ${this.toastMsg ? `<div class="seg">${noteHtml(this.toastMsg, "ok")}</div>` : ""}

      <div class="seg">${this.channelAHtml()}</div>
      <div class="seg">${this.channelBHtml()}</div>
      ${this.advancedHtml()}
    `;

    this.bind();
    if (this.running) this.paintProgress(this.prog || { frac: 0, msg: "正在准备…" });
  },

  renderNeedGame() {
    this.root.innerHTML = `<div class="seg">${emptyHtml(
      "①",
      "还没选游戏",
      "得先让 STool 知道是哪个游戏、什么引擎，才知道能不能走「不改游戏文件」这条更省事的路。",
      "去选游戏",
      "tGoDetect"
    )}</div>`;
    $("#tGoDetect", this.root).addEventListener("click", () => {
      location.hash = "#detect";
    });
  },

  // -- 通道 A ---------------------------------------------------------------

  channelAHtml() {
    const run = this.running;
    const c = this.csv;

    const csvLine = c
      ? c.total > 0
        ? `共 <strong>${c.total}</strong> 条，已翻 <strong>${c.done}</strong> 条，还剩 ${c.todo} 条`
        : "表格是空的 —— 这个引擎可能没有可提取的脚本文本。"
      : this.csvPath
      ? "还没读表格。"
      : "还没提取过，先点下面的「提取文本」。";

    return `
      <div class="card">
        <div class="card-head">
          <div class="card-title">通道 ① · 提取 → 翻译 → 回填（通用）</div>
          <div class="card-hint">任何引擎都能走；会改动游戏目录里的文件（写前自动备份）</div>
        </div>

        <div class="row">
          <input class="field mono grow" id="tOut" readonly
                 value="${esc(this.outDir)}" placeholder="输出目录（放提取出来的表格）">
          <button class="btn btn-ghost" id="tPickOut" ${run ? "disabled" : ""}>选择输出目录</button>
        </div>

        <div class="row mt2">
          <input class="field mono grow" id="tCsv" readonly
                 value="${esc(this.csvPath)}" placeholder="文本表格（text.csv）">
          <button class="btn btn-ghost" id="tPickCsv" ${run ? "disabled" : ""}>选择表格</button>
        </div>

        <div class="small dim2 mt2">${csvLine}</div>

        <div id="tProg">${run ? this.progHtml() : ""}</div>

        <div class="row mt3" style="flex-wrap:wrap">
          <button class="btn btn-primary" id="tExtract" ${run || !this.outDir ? "disabled" : ""}>
            1. 提取文本
          </button>
          <button class="btn" id="tOpenCsv" ${c && c.exists ? "" : "disabled"}>2. 打开表格去翻译</button>
          <button class="btn" id="tMtlab" ${c && c.exists && !run ? "" : "disabled"}>用机翻先填一遍</button>
          <button class="btn btn-danger" id="tImport" ${run || !(c && c.exists) ? "disabled" : ""}>
            3. 翻译回填
          </button>
        </div>

        <div class="small dim mt3">
          「翻译回填」会把表格译文列写回游戏文件。写之前原文件会备份成
          <span class="mono">&lt;文件名&gt;.stool.bak</span>，出问题可以还原。
        </div>

        ${c && c.sample.length ? this.csvPreviewHtml(c) : ""}
      </div>
    `;
  },

  csvPreviewHtml(c) {
    const rows = c.sample
      .map(
        (r) => `<tr>
          <td class="mono dim2">${esc(clip(r.file, 22))}</td>
          <td>${esc(clip(r.source, 40))}</td>
          <td class="${r.done ? "" : "dim2"}">${r.done ? esc(clip(r.target, 40)) : "（还没翻）"}</td>
        </tr>`
      )
      .join("");
    return foldHtml(
      `表格预览（前 ${c.sample.length} 条）`,
      `<table class="tbl">
         <thead><tr><th>文件</th><th>原文</th><th>译文</th></tr></thead>
         <tbody>${rows}</tbody>
       </table>
       <div class="small dim2 mt2">
         列的顺序是 <span class="mono">id, file, key, 原文, 译文</span> ——
         请<strong>只填译文列</strong>，别的列别动（回填靠它们定位到游戏里的哪一句）。
       </div>`,
      !!this._openCsv
    );
  },

  // -- 通道 B ---------------------------------------------------------------

  channelBHtml() {
    const s = this.stats;
    if (!s) {
      return `
        <div class="card">
          <div class="card-head">
            <div class="card-title">通道 ② · 不改游戏文件</div>
            <div class="card-hint">正在查询这个引擎支不支持…</div>
          </div>
          <div class="small dim2"><span class="spin"></span> 查询中…</div>
        </div>`;
    }

    if (!s.inject_supported) {
      // 不支持**不是故障**：直接把原因和该走哪条路写出来，不给红字报警吓人。
      return `
        <div class="card">
          <div class="card-head">
            <div class="card-title">通道 ② · 不改游戏文件（这个引擎不行）</div>
            <div class="card-hint">不影响汉化 —— 走上面的通道 ① 就行</div>
          </div>
          <div class="note info">${esc(s.inject_limits)}</div>
        </div>`;
    }

    const st = s.installed
      ? `<span class="pill high">已生效</span>
         <span class="small dim2">共 ${s.entries} 条译文正在生效</span>`
      : `<span class="pill none">未生效</span>
         <span class="small dim2">游戏里现在还是原文</span>`;

    return `
      <div class="card">
        <div class="card-head">
          <div class="card-title">通道 ② · 不改游戏文件（推荐）</div>
          <div class="card-hint">${esc(s.inject_mechanism)}</div>
        </div>

        <div class="row tight mt2">${st}</div>

        <div class="row mt3">
          <input class="field mono grow" id="tJson" readonly
                 value="${esc(this.jsonPath || s.json_path)}"
                 placeholder="译文表（JSON）—— 键是原文，值是译文">
          <button class="btn btn-ghost" id="tPickJson" ${this.busy ? "disabled" : ""}>选择</button>
        </div>

        <div class="row mt3" style="flex-wrap:wrap">
          <button class="btn" id="tMakeJson" ${this.busy || !(this.csv && this.csv.exists) ? "disabled" : ""}>
            ① 从表格生成译文表
          </button>
          <button class="btn btn-primary" id="tInject" ${this.busy ? "disabled" : ""}>② 让译文生效</button>
          <button class="btn btn-ghost" id="tUninject" ${this.busy || !s.installed ? "disabled" : ""}>
            ③ 撤回（还原成原文）
          </button>
        </div>

        ${foldHtml(
          "这个方式有什么限制",
          `<div class="small dim2">${esc(s.inject_limits)}</div>
           <div class="small dim2 mt2">
             撤回会把 STool 加进去的东西撤掉，游戏恢复成没处理过的样子（有备份兜底）。
           </div>`
        )}
      </div>`;
  },

  // -- 进阶：封包 / 术语表 / MTL 配置 ---------------------------------------

  advancedHtml() {
    const presetOpts = MTL_PRESETS.map(
      (p) => `<option value="${p.id}"${this.mtlPreset === p.id ? " selected" : ""}>${esc(p.name)}</option>`
    ).join("");

    return (
      `<div class="seg">${foldHtml(
        "机翻设置（DeepSeek / 智谱 / 本地 Ollama）",
        `<div class="small dim2">
           机翻只是<strong>帮你把译文列先填上</strong>，填完仍要自己过一遍 ——
           机翻直接塞进游戏，读起来会很别扭。
         </div>

         <div class="row mt3">
           <select class="field grow" id="tPreset">${presetOpts}</select>
           <button class="btn btn-ghost btn-sm" id="tUsePreset">套用</button>
         </div>
         <div class="small dim2 mt2" id="tPresetNote"></div>

         <div class="row mt3">
           <input class="field mono grow" id="tBase" value="${esc(this.mtlBase)}"
                  placeholder="接口地址，如 https://api.deepseek.com/v1">
           <span class="tag">API 地址</span>
         </div>
         <div class="row mt2">
           <input class="field mono grow" id="tModel" value="${esc(this.mtlModel)}"
                  placeholder="模型名，如 deepseek-chat">
           <span class="tag">模型</span>
         </div>
         <div class="row mt2">
           <input class="field mono grow" id="tKey" type="password" value="${esc(this.mtlKey)}"
                  placeholder="密钥（只存在本机，不会外传）">
           <span class="tag">密钥</span>
         </div>

         <div class="mt3">
           <div class="small dim2">术语表（可选）：每行 <span class="mono">原文=译文</span>，
             比如 <span class="mono">勇者=勇者</span>、<span class="mono">王都=王都</span>。
             填了它，机翻会优先照这个来，人名地名不会一集一个译法。</div>
           <textarea class="field mono mt2" id="tGlossary" rows="4"
                     placeholder="勇者=勇者&#10;王都アルテナ=王都阿尔特纳">${esc(this.mtlGlossary)}</textarea>
         </div>

         <div class="row mt3">
           <button class="btn btn-primary" id="tSaveMtl" ${this.mtlRunning ? "disabled" : ""}>保存设置</button>
           <button class="btn" id="tRunMtl"
                   ${this.mtlRunning || !(this.csv && this.csv.exists) ? "disabled" : ""}>
             开始机翻当前表格
           </button>
           ${this.mtlRunning ? `<span class="small dim2"><span class="spin"></span> 正在机翻…</span>` : ""}
         </div>`,
        // `_openMtl` 只被预览页设置（截图用），真实运行永远为假。
        !!this._openMtl
      )}</div>`
    );
  },

  // =========================================================================
  // 事件绑定
  // =========================================================================

  bind() {
    const on = (id, fn) => {
      const b = $(id, this.root);
      if (b) b.addEventListener("click", fn);
    };

    // 通道 A
    on("#tPickOut", () => this.pickOut());
    on("#tPickCsv", () => this.pickCsv());
    on("#tExtract", () => this.runText("extract"));
    on("#tImport", () => this.runText("import"));
    on("#tOpenCsv", () => this.openCsv());
    on("#tMtlab", () => this.toggleMtl(true));

    // 通道 B
    on("#tPickJson", () => this.pickJson());
    on("#tMakeJson", () => this.makeJson());
    on("#tInject", () => this.inject());
    on("#tUninject", () => this.uninject());

    // MTL
    on("#tSaveMtl", () => this.saveMtl());
    on("#tRunMtl", () => this.runMtl());
    const preset = $("#tPreset", this.root);
    if (preset) {
      preset.addEventListener("change", () => {
        this.mtlPreset = preset.value;
        this.paintPresetNote();
      });
      // 首次打开时同步一次说明文字
      if (!this._noteInit) {
        this._noteInit = true;
        this.paintPresetNote();
      }
    }
    const useP = $("#tUsePreset", this.root);
    if (useP) {
      useP.addEventListener("click", () => {
        const p = MTL_PRESETS.find((x) => x.id === (this.mtlPreset || preset?.value));
        if (!p) return;
        this.mtlBase = p.base;
        this.mtlModel = p.model;
        this.mtlPreset = p.id;
        this.render();
        toast(`已套用 ${p.name} 的地址和模型，填上密钥即可。`);
      });
    }

    // 取消按钮（进度条里）
    on("#tCancel", () => this.cancel());
  },

  paintPresetNote() {
    const n = $("#tPresetNote", this.root);
    if (!n) return;
    const p = MTL_PRESETS.find((x) => x.id === this.mtlPreset);
    n.textContent = p ? p.note : "";
  },

  // =========================================================================
  // 通道 A 动作
  // =========================================================================

  async pickOut() {
    const r = await Tauri.call("pick_folder", { title: "选择输出目录" });
    if (!r.ok) return toast(r.err, "err");
    if (!r.data) return;
    this.outDir = r.data;
    if (!this.csvPath) this.csvPath = joinPath(r.data, "text.csv");
    await this.render();
  },

  async pickCsv() {
    const r = await Tauri.call("pick_file", { title: "选择文本表格（CSV）", ext: "csv" });
    if (!r.ok) return toast(r.err, "err");
    if (!r.data) return;
    this.csvPath = r.data;
    await this.loadCsv();
  },

  async openCsv() {
    if (!this.csvPath) return;
    const r = await Tauri.call("shell_open", { path: this.csvPath });
    if (!r.ok) toast(r.err, "err");
  },

  async runText(kind) {
    if (this.running) return;
    this.running = kind;
    this.prog = { frac: 0, msg: "正在准备…" };
    this.result = null;
    this.err = "";
    this.toastMsg = "";
    await this.render();

    // 先订阅再调用：任务可能很快，头几个进度事件不等订阅就会丢。
    await this.subscribe();

    const cmd = kind === "extract" ? "text_extract" : "text_import";
    const r = await Tauri.call(cmd, { outDir: this.outDir, csvPath: this.csvPath });

    this.running = "";
    if (!r.ok) {
      this.err = r.err;
      log(`text: ${cmd} 失败 ${r.err}`);
    } else if (!r.data.success) {
      this.err = r.data.message;
      log(`text: ${cmd} 内核判失败 ${r.data.message}`);
    } else {
      this.result = r.data;
      if (r.data.csv) this.csv = r.data.csv;
      log(`text: ${cmd} 完成 files=${r.data.files_done}`);
    }
    await this.render();
    if (!this.err && this.result) {
      toast(this.result.message || "完成。");
      this.toastMsg = this.result.message || "";
    }
    if (this.err) toast("没成功，原因见页面上的提示。", "err");
    // 回填后注入状态可能变了（译文表条目数），刷新一次
    if (kind === "import") await this.loadStats();
  },

  async loadCsv() {
    if (!this.csvPath) {
      this.csv = null;
      return;
    }
    const r = await Tauri.call("csv_stats", { path: this.csvPath });
    if (!r.ok) {
      // 表格还没生成不算错误，页面上的文案已经说了「还没提取过」
      this.csv = null;
      log(`text: csv_stats 未就绪 ${r.err}`);
    } else {
      this.csv = r.data;
    }
  },

  // =========================================================================
  // 通道 B 动作
  // =========================================================================

  async loadStats() {
    const r = await Tauri.call("text_stats");
    if (!r.ok) {
      this.err = r.err;
      log(`text: text_stats 失败 ${r.err}`);
    } else {
      this.stats = r.data;
      if (!this.jsonPath && r.data.json_path) this.jsonPath = r.data.json_path;
      // 默认把译文表放在游戏目录里，注入时内核也要从这里找
      if (!this.jsonPath && Store.gameRoot) this.jsonPath = joinPath(Store.gameRoot, "stool_translate.json");
      log(
        `text: stats supported=${r.data.inject_supported} installed=${r.data.installed} entries=${r.data.entries}`
      );
    }
    await this.render();
  },

  async pickJson() {
    const r = await Tauri.call("pick_file", { title: "选择译文表（JSON）", ext: "json" });
    if (!r.ok) return toast(r.err, "err");
    if (!r.data) return;
    this.jsonPath = r.data;
    await this.render();
  },

  async makeJson() {
    if (this.busy) return;
    this.busy = true;
    this.err = "";
    await this.render();
    const r = await Tauri.call("text_make_json", { csvPath: this.csvPath, jsonPath: this.jsonPath });
    this.busy = false;
    if (!r.ok) {
      this.err = r.err;
    } else {
      this.csv = r.data;
      toast(`译文表已生成：共 ${r.data.total} 条，其中 ${r.data.done} 条带译文。`);
    }
    await this.render();
  },

  async inject() {
    if (this.busy) return;
    this.busy = true;
    this.err = "";
    await this.render();
    const r = await Tauri.call("text_inject", { jsonPath: this.jsonPath });
    this.busy = false;
    if (!r.ok) {
      this.err = r.err;
    } else {
      this.toastMsg = r.data.message;
      toast(r.data.message || "已生效。");
    }
    await this.loadStats();
  },

  async uninject() {
    if (this.busy) return;
    this.busy = true;
    this.err = "";
    await this.render();
    const r = await Tauri.call("text_uninject");
    this.busy = false;
    if (!r.ok) {
      this.err = r.err;
    } else {
      this.toastMsg = r.data.message;
      toast(r.data.message || "已撤回。");
    }
    await this.loadStats();
  },

  // =========================================================================
  // 机翻
  // =========================================================================

  toggleMtl(open) {
    // 只展开「机翻设置」折叠块并把视线带过去，不自动开跑 ——
    // 用户还没填密钥就点，只会拿到一条失败信息，反而像功能坏了。
    const fold = this.root.querySelector("details.fold");
    if (open && fold) {
      fold.open = true;
      fold.scrollIntoView({ block: "nearest", behavior: "smooth" });
    }
    toast("展开「机翻设置」填好地址与密钥，再点「开始机翻当前表格」。");
  },

  collectMtl() {
    const g = (id) => {
      const e = $(id, this.root);
      return e ? e.value.trim() : "";
    };
    this.mtlBase = g("#tBase") || this.mtlBase;
    this.mtlModel = g("#tModel") || this.mtlModel;
    this.mtlKey = g("#tKey") || this.mtlKey;
    this.mtlGlossary = g("#tGlossary") || this.mtlGlossary;
  },

  async saveMtl() {
    this.collectMtl();
    // 配置落在**设置文件**里（settings::Config），不写在游戏目录 ——
    // 密钥跟着游戏目录跑会让用户换游戏就得重填，而且可能被一起打包发出去。
    const r = await Tauri.call("mtl_save", {
      base: this.mtlBase,
      key: this.mtlKey,
      model: this.mtlModel,
      glossary: this.mtlGlossary,
    });
    if (!r.ok) {
      this.err = r.err;
    } else {
      toast("机翻设置已保存。");
    }
    await this.render();
  },

  async runMtl() {
    if (this.mtlRunning) return;
    this.collectMtl();
    if (!this.csvPath) return toast("先提取文本，生成表格。", "err");
    if (!this.mtlBase || !this.mtlModel) {
      return toast("先填接口地址和模型名（可以点上面的预设「套用」）。", "err");
    }

    this.mtlRunning = true;
    this.err = "";
    await this.render();

    const r = await Tauri.call("text_mtl", {
      csvPath: this.csvPath,
      base: this.mtlBase,
      key: this.mtlKey,
      model: this.mtlModel,
      glossary: this.mtlGlossary,
    });

    this.mtlRunning = false;
    if (!r.ok) {
      this.err = r.err;
      log(`text: mtl 失败 ${r.err}`);
    } else {
      toast(`机翻完成：填了 ${r.data.done} 条译文。请打开表格过目一遍。`);
      this.toastMsg = `机翻完成：填了 ${r.data.done} 条译文。请打开表格过目一遍。`;
    }
    await this.loadCsv();
    await this.render();
  },

  // =========================================================================
  // 进度
  // =========================================================================

  progHtml() {
    return `
      <div class="mt3" id="tProgBody">
        <div class="row tight">
          <span class="small dim2" data-msg style="flex:1 1 auto;min-width:0">正在准备…</span>
          <span class="small mono" data-pct>0%</span>
        </div>
        <div class="bar mt2"><i style="width:0%"></i></div>
        <div class="row mt2">
          <button class="btn btn-ghost btn-sm" id="tCancel">取消</button>
        </div>
      </div>`;
  },

  paintProgress(p) {
    this.prog = p;
    const host = $("#tProg", this.root);
    if (!host || !host.firstElementChild) return;
    const pct = Math.max(0, Math.min(100, Math.round((p.frac || 0) * 100)));
    const bar = host.querySelector(".bar > i");
    if (bar) bar.style.width = pct + "%";
    const pv = host.querySelector("[data-pct]");
    if (pv) pv.textContent = pct + "%";
    const mv = host.querySelector("[data-msg]");
    if (mv) mv.textContent = clip(p.msg || "…", 64);
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

  async cancel() {
    await Tauri.call("cancel_task");
    toast("已请求取消，会在当前文件写完后停下。");
  },
};

/** 拼接路径（前端只管显示，真正的拼接在内核）。 */
function joinPath(dir, name) {
  const d = String(dir || "").replace(/[\\/]+$/, "");
  return d ? d + "\\" + name : name;
}
