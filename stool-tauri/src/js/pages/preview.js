/* ---------------------------------------------------------------------------
   ③ 看素材（左列表 + 右预览）。

   设计要点（docs/TAURI重构方案.md §5.2）：
   - 顶部一句话说明「点左边任意文件即可预览」，其余交给用户点；
   - 左侧列表**虚拟化**（素材目录常有几万张图；虚拟化省的是 DOM，IPC 由后端 3000 条上限兜住）；
   - 右侧按类型分流：图片 <img> / 音频 <audio> / 文本按**原编码**解出来显示；
   - 浏览器渲染不了的格式（.tga / .tif 这类）**提前说清楚**并给「用系统程序打开」，
     而不是塞给 <img> 一个坏链接让人对着空白发呆。

   扩展名分类与编码判定都在**内核** `features::preview`，这里不重复写表。
--------------------------------------------------------------------------- */

const PreviewPage = {
  title: "看素材",
  sub: "直接浏览已经取出来的图片、音乐和文本，不用另开程序。",

  root: null,
  dir: "",
  kind: "all",
  files: [],
  total: 0,
  scanning: false,
  /** 当前 dir 是否已经扫过（mount 时据此决定要不要补一次扫描） */
  scanned: false,
  sel: -1,
  payload: null,
  perr: "",
  busyPv: false,
  err: "",
  autoTried: false,

  ROW_H: 34,
  BUFFER: 6,

  actions() {
    return "";
  },

  async mount(host) {
    this.root = host;
    await this.render();

    // 自动化验证钩子：STOOL_OUT 指向「取出素材」的输出目录时，用它当默认浏览目录。
    if (!this.autoTried) {
      this.autoTried = true;
      const r = await Tauri.call("env_out_dir");
      if (r.ok && r.data && !this.dir) {
        this.dir = r.data;
        log(`preview: env_out_dir -> ${r.data}`);
      }
    }
    // 有目录但还没扫过就补一次 —— 目录可能来自上面的钩子，或来自预览页的预置参数。
    // 少了这一步，界面会显示一个"选了目录却没内容"的空白列表。
    if (this.dir && !this.scanned) await this.scan();
    return this;
  },

  // =========================================================================
  // 骨架
  // =========================================================================

  async render() {
    const scanning = this.scanning;

    this.root.innerHTML = `
      <div class="seg">
        <div class="say">
          <span class="say-mark">?</span>
          <div class="say-body">
            选到「取出素材」时的那个输出目录，然后在左边点任意文件。<br>
            图片直接显示，音频可以试听，文本会按<strong>原编码</strong>解出来（日文老游戏多是 Shift-JIS）。
          </div>
        </div>
      </div>

      <div class="seg">
        <div class="card">
          <div class="card-head">
            <div class="card-title">看哪个目录里的素材</div>
            <div class="card-hint">一般是「取出素材」的输出目录</div>
          </div>
          <div class="row">
            <input class="field mono grow" id="pvDir" readonly
                   value="${esc(this.dir)}" placeholder="还没选目录">
            <button class="btn btn-ghost" id="pvPick" ${scanning ? "disabled" : ""}>选择目录</button>
          </div>
          <div class="chipbar mt3">
            ${[["all", "全部"], ["image", "图片"], ["audio", "音频"], ["text", "文本"]]
              .map(
                ([k, t]) =>
                  `<button class="chip${this.kind === k ? " on" : ""}" data-kind="${k}" ${scanning ? "disabled" : ""}>${t}</button>`
              )
              .join("")}
          </div>
          <div class="small dim2 mt2" id="pvCount">${scanning ? "正在扫描…" : ""}</div>
        </div>
      </div>

      ${this.err ? `<div class="seg">${noteHtml(this.err)}</div>` : ""}

      ${
        this.dir
          ? `<div class="seg">
               <div class="split">
                 <div class="mlist" id="mlist"><div class="mlist-inner" id="mlistInner"></div></div>
                 <div class="pv" id="pvPane"></div>
               </div>
             </div>`
          : `<div class="seg">${emptyHtml(
              "🗂",
              "还没选目录",
              "选一个已经取出素材的目录，才能在这里浏览。",
              "去取出素材",
              "pvGoExtract"
            )}</div>`
      }
    `;

    $("#pvPick", this.root).addEventListener("click", () => this.pick());
    const go = $("#pvGoExtract", this.root);
    if (go) go.addEventListener("click", () => (location.hash = "#extract"));
    this.root.querySelectorAll("[data-kind]").forEach((b) => {
      b.addEventListener("click", () => {
        if (this.kind === b.dataset.kind) return;
        this.kind = b.dataset.kind;
        this.scan();
      });
    });

    const list = $("#mlist", this.root);
    if (list) {
      list.addEventListener("scroll", () => this.paintList(), { passive: true });
      list.addEventListener("click", (e) => {
        const row = e.target.closest(".mrow");
        if (row) this.select(Number(row.dataset.i));
      });
    }

    this.paintList();
    this.renderPv();
  },

  // =========================================================================
  // 列表（虚拟化：只渲染可见窗口）
  // =========================================================================

  paintList() {
    const list = $("#mlist", this.root);
    const inner = $("#mlistInner", this.root);
    if (!list || !inner) return;

    const total = this.files.length;
    const H = this.ROW_H;
    inner.style.height = total * H + "px";

    const top = list.scrollTop;
    const h = list.clientHeight || 460;
    const start = Math.max(0, Math.floor(top / H) - this.BUFFER);
    const end = Math.min(total, Math.ceil((top + h) / H) + this.BUFFER);

    const icons = { image: "🖼", audio: "🎵", text: "📝" };
    let html = "";
    for (let i = start; i < end; i++) {
      const f = this.files[i];
      const on = i === this.sel ? " on" : "";
      html += `<button class="mrow${on}" style="top:${i * H}px" data-i="${i}" title="${esc(f.rel)}">
        <span class="mrow-name">${icons[f.kind] || "📄"} ${esc(f.name)}</span>
        <span class="mrow-meta">${esc(clip(f.rel, 44))} · ${esc(f.size)}</span>
      </button>`;
    }
    inner.innerHTML = html;

    const cnt = $("#pvCount", this.root);
    if (cnt) {
      cnt.textContent = this.scanning
        ? "正在扫描…"
        : this.total > total
        ? `共 ${this.total} 个，列表先显示前 ${total} 个（用上面的筛选缩小范围）`
        : total
        ? `共 ${total} 个`
        : this.dir
        ? "这个目录里没找到可预览的素材。"
        : "";
    }
  },

  // =========================================================================
  // 右侧预览
  // =========================================================================

  renderPv() {
    const host = $("#pvPane", this.root);
    if (!host) return;

    if (!this.dir) {
      host.innerHTML = "";
      return;
    }
    if (!this.files.length) {
      host.innerHTML = `<div class="small dim2">这个筛选下没有可预览的素材。换个类型看看，或重新选目录。</div>`;
      return;
    }
    const f = this.files[this.sel];
    if (!f) {
      host.innerHTML = `<div class="small dim2">点左边任意文件即可预览。</div>`;
      return;
    }

    const meta = `<div class="row tight">
        <div style="flex:1 1 auto;min-width:0">
          <div class="quick-label">${esc(f.name)}</div>
          <div class="quick-key" style="margin-bottom:0" title="${esc(f.rel)}">${esc(clip(f.rel, 64))} · ${esc(f.size)}</div>
        </div>
        <span class="tag">${{ image: "图片", audio: "音频", text: "文本" }[f.kind] || "文件"}</span>
      </div>`;

    let body;
    if (this.busyPv) {
      body = `<div class="small dim2"><span class="spin"></span> 正在读取…</div>`;
    } else if (this.perr) {
      // 有一种错误是"预期内"的：扩展名像文本、内容却是密文（加密封包残留）。
      // 这不是故障，所以用 warn 语气说清楚 + 给系统程序打开的出路，而不是晾一句红字。
      body = `<div style="align-self:stretch;width:100%">${noteHtml(this.perr, "warn")}</div>`;
    } else if (!f.renderable) {
      body = `<div class="small dim2">这个格式浏览器显示不了，用下面「用系统程序打开」吧。</div>`;
    } else if (this.payload) {
      const p = this.payload;
      if (p.kind === "image") {
        // base64 字符集里没有引号/尖括号，直接进属性是安全的（也不该对几 MB 的串做逐字转义）
        body = `<img class="pv-img" src="${p.data_url}" alt="${esc(f.name)}">`;
      } else if (p.kind === "audio") {
        body = `<audio class="pv-audio" controls src="${p.data_url}"></audio>`;
      } else if (p.kind === "text") {
        body = `<pre class="pv-text">${esc(p.text)}</pre>`;
      } else {
        body = `<div class="small dim2">没有可显示的内容。</div>`;
      }
    } else {
      body = "";
    }

    const enc = this.payload && this.payload.encoding
      ? `<div class="small dim2">编码：<span class="mono">${esc(this.payload.encoding)}</span>${
          this.payload.truncated ? " · 文件较大，只显示开头一段" : ""
        }</div>`
      : "";

    host.innerHTML = `
      ${meta}
      <div class="pv-body">${body}</div>
      ${enc}
      <div class="row tight">
        <button class="btn btn-ghost btn-sm" id="pvOpen">用系统程序打开</button>
      </div>
    `;
    $("#pvOpen", host).addEventListener("click", () => this.openFile(f.path));
  },

  // =========================================================================
  // 动作
  // =========================================================================

  async pick() {
    const r = await Tauri.call("pick_folder", { title: "选择素材目录" });
    if (!r.ok) return toast(r.err, "err");
    if (!r.data) return; // 取消
    this.dir = r.data;
    this.scanned = false; // 换目录了，必须重扫
    await this.scan();
  },

  async scan() {
    if (!this.dir) return;
    this.scanned = true;
    this.scanning = true;
    this.sel = -1;
    this.payload = null;
    this.perr = "";
    this.err = "";
    await this.render();

    const r = await Tauri.call("scan_media", { dir: this.dir, kind: this.kind });
    this.scanning = false;
    if (!r.ok) {
      this.err = r.err;
      this.files = [];
      this.total = 0;
      log(`preview: scan 失败 ${r.err}`);
      await this.render();
      return;
    }
    this.files = r.data.files || [];
    this.total = r.data.total || 0;
    log(`preview: scan kind=${this.kind} -> ${this.files.length}/${this.total}（${r.data.elapsed_ms}ms）`);
    await this.render();
    if (this.files.length) {
      // 截图钩子（只在 preview.html 里会被赋值）：直接停在指定序号的文件上
      const want = window.__PV_PICK__;
      delete window.__PV_PICK__;
      await this.select(typeof want === "number" && want < this.files.length ? want : 0);
    }
  },

  async select(i) {
    this.sel = i;
    this.paintList();
    const f = this.files[i];
    if (f) await this.load(f);
  },

  async load(f) {
    this.payload = null;
    this.perr = "";
    this.busyPv = true;
    this.renderPv();

    const r = await Tauri.call("read_media", { path: f.path });
    this.busyPv = false;
    if (!r.ok) {
      this.perr = r.err;
      log(`preview: read 失败 ${r.err}`);
    } else {
      this.payload = r.data;
      log(`preview: read ${f.name} kind=${r.data.kind} enc=${r.data.encoding || "-"} ${r.data.size}`);
    }
    this.renderPv();
  },

  async openFile(path) {
    const r = await Tauri.call("open_file", { path });
    if (!r.ok) toast(r.err, "err");
  },
};
