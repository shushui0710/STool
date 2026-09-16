/* ---------------------------------------------------------------------------
   ⑨ 工具箱。

   这一页承担重构方案里「合并 设置 / 日志 / 帮助 / 体检 / 自检」的决定
   （docs/TAURI重构方案.md §5.1 第 1 条）——它们都是**出问题才用**，
   不该各占一个顶级入口。所以这里按「你要干嘛」分三块：

     1. 设置 —— 换个默认输出目录、补一个外部工具、填代理与机翻接口。
     2. 诊断 —— 出问题时按顺序点：体检（游戏能不能正常处理）→
        自检（封包读写会不会坏）→ 看日志 → 导出诊断包（发给人看）。
     3. 帮助 —— 一句话上手 + 常见问题，就地解决，不用去翻别的地方。

   两条硬约束：
   - **不回传密钥**。机翻密钥只有「配没配」这一个是非，界面永远拿不到密钥本体；
     要改的话在「翻译文字」页填，那里是同一个 `mtl_save`。
   - 「出问题才用」不等于「随便糊」。每个诊断项都必须带「原因 + 修法」，
     没有失败时也要给一句「都正常」的正面结论，不能是一片空白让人怀疑没跑。
--------------------------------------------------------------------------- */

const ToolsPage = {
  title: "工具箱",
  sub: "设置、诊断、帮助 —— 平时用不到，出问题才来。",

  root: null,
  tab: "diag", // "settings" | "diag" | "help" —— 默认停在诊断（出事最常来的是这）

  settings: null, // SettingsOut
  loading: true,
  err: "",
  busy: false,

  // 诊断区
  check: null, // CheckOut（体检或自检最近一次结果）
  checkKind: "", // "health" | "selfcheck"
  checking: false,
  log: null, // LogOut
  logTail: 300,
  logLoading: false,
  diagPath: "", // 导出后的 zip 路径

  // 设置区草稿（输入框只记值，不重绘，避免丢焦点）
  draftOut: "",
  draftProxy: "",
  draftBase: "",
  draftModel: "",

  actions() {
    return "";
  },

  async mount(host) {
    this.root = host;
    this.settings = null;
    this.loading = true;
    this.err = "";
    this.busy = false;
    this.check = null;
    this.checkKind = "";
    this.checking = false;
    this.log = null;
    this.diagPath = "";

    await this.render();
    await this.loadSettings();
    // 进页面顺手拉一次日志尾部，但**不**自动跑体检 —— 那是重活，得用户自己点
    await this.loadLog();
    return this;
  },

  unmount() {},

  // -- 数据 -----------------------------------------------------------------

  async loadSettings() {
    const r = await Tauri.call("tools_settings");
    this.loading = false;
    if (!r.ok) {
      this.err = r.err;
      log(`tools: tools_settings 失败 ${r.err}`);
    } else {
      this.settings = r.data;
      this.draftOut = r.data.output_dir || "";
      this.draftProxy = r.data.proxy || "";
      this.draftBase = r.data.mtl_base_url || "";
      this.draftModel = r.data.mtl_model || "";
      log(`tools: 设置已载入，外部工具 ${r.data.tools.length} 项`);
    }
    await this.render();
  },

  async loadLog() {
    this.logLoading = true;
    await this.render();
    const r = await Tauri.call("tools_log", { tail: this.logTail });
    this.logLoading = false;
    if (r.ok) this.log = r.data;
    else log(`tools: 读日志失败 ${r.err}`);
    await this.render();
  },

  // -- 渲染 -----------------------------------------------------------------

  async render() {
    this.root.innerHTML = [
      this.segTabsHtml(),
      this.err ? `<div class="seg">${noteHtml(this.err)}</div>` : "",
      this.tab === "settings" ? this.settingsTabHtml() : "",
      this.tab === "diag" ? this.diagTabHtml() : "",
      this.tab === "help" ? this.helpTabHtml() : "",
    ].join("");
    this.bind();
  },

  /** 三块用分段控件切。别做成三个页面 —— 它们是同一件事的三个面。 */
  segTabsHtml() {
    const t = (id, label) =>
      `<button class="tab${this.tab === id ? " on" : ""}" data-tab="${id}">${esc(
        label
      )}</button>`;
    return `<div class="seg"><div class="tabs">${t("settings", "设置")}${t(
      "diag",
      "诊断"
    )}${t("help", "帮助")}</div></div>`;
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

  // -- 设置 -----------------------------------------------------------------

  settingsTabHtml() {
    if (this.loading) return this.loadingHtml("正在读取设置…");
    if (!this.settings) return "";
    const s = this.settings;

    // 外部工具表：一行一个，右侧按需给「下载」与「保存」
    const toolRows = s.tools
      .map(
        (t) => `<div class="body" style="padding:var(--s2) 0;border-bottom:1px solid var(--line)">
          <div class="small"><strong>${esc(t.label)}</strong>
            <span class="dim"> · ${esc(t.hint)}</span>
            ${t.path ? `<span class="pill high" style="margin-left:6px">已配置</span>` : `<span class="pill none" style="margin-left:6px">未配置</span>`}
          </div>
          <div class="row tight mt2">
            <input class="field mono grow" data-tool="${esc(t.key)}"
              placeholder="${esc(t.downloadable ? "点「⬇ 下载」自动装，或手填完整路径" : "手填完整路径")}"
              value="${esc(t.path)}" />
            ${t.downloadable ? `<button class="btn btn-ghost btn-sm" data-dl="${esc(t.key)}">⬇ 下载</button>` : ""}
            <button class="btn btn-ghost btn-sm" data-toolsave="${esc(t.key)}">保存</button>
          </div>
        </div>`
      )
      .join("");

    const settingsBody = `<div class="body">
      <div class="row">
        <label class="small dim" style="width:96px">默认输出目录</label>
        <input class="field mono grow" id="tOut" placeholder="留空 = 每次现选" value="${esc(s.output_dir)}" />
      </div>
      <p class="small dim">「取出素材」时默认把文件放到这里。留空就每次让你自己选。</p>
      <div class="row tight mt3">
        <button class="btn btn-primary" id="tSaveBasic">保存</button>
        <span class="small dim">改完记得点保存</span>
      </div>
    </div>`;

    const mtlBody = `<div class="body">
      <div class="row">
        <label class="small dim" style="width:96px">接口地址</label>
        <input class="field mono grow" id="tBase" placeholder="https://api.deepseek.com/v1/chat/completions" value="${esc(s.mtl_base_url)}" />
      </div>
      <div class="row">
        <label class="small dim" style="width:96px">模型名</label>
        <input class="field grow" id="tModel" placeholder="deepseek-chat" value="${esc(s.mtl_model)}" />
      </div>
      <div class="row tight mt2">
        <span class="small">密钥：</span>
        ${
          s.mtl_key_set
            ? `<span class="pill high">已设置</span><span class="small dim">改密钥请去「翻译文字」页（同一份设置，密钥不回传到界面）</span>`
            : `<span class="pill none">未设置</span><span class="small dim">去「翻译文字」页填一次即可</span>`
        }
      </div>
      <div class="row tight mt3">
        <button class="btn btn-primary" id="tSaveMtl">保存接口与模型</button>
      </div>
    </div>`;

    const proxyBody = `<div class="body">
      <div class="row">
        <label class="small dim" style="width:96px">代理</label>
        <input class="field mono grow" id="tProxy" placeholder="http://127.0.0.1:7890" value="${esc(s.proxy)}" />
      </div>
      <p class="small dim">下载外部工具时走它。国内直连 GitHub 常常超时，填上代理就顺了。</p>
      <div class="row tight mt3"><button class="btn btn-primary" id="tSaveProxy">保存代理</button></div>
    </div>`;

    return [
      this.cardWrap("默认输出目录", "「取出素材」默认放这里", settingsBody),
      this.cardWrap(
        "外部工具",
        "认不出的格式要靠它们才能解，点「⬇ 下载」会自动装好并填路径",
        `<div class="body">${toolRows}</div>`
      ),
      this.cardWrap("机翻接口", "「翻译文字」里的机翻用它", mtlBody),
      this.cardWrap("代理", "下载工具时走它", proxyBody),
      this.cardWrap(
        "配置文件在哪",
        "",
        `<div class="body">
          <div class="small">所有设置都存在这个文件里，可以直接备份或拷到另一台机器：</div>
          <div class="mono small mt2">${esc(shortPath(s.config_path))}</div>
          <div class="row tight mt3">
            <button class="btn btn-ghost btn-sm" id="tOpenCfg">打开所在文件夹</button>
          </div>
        </div>`
      ),
    ].join("");
  },

  // -- 诊断 -----------------------------------------------------------------

  diagTabHtml() {
    const c = this.check;
    let checkHtml = "";
    if (this.checking) {
      checkHtml = `<div class="row tight mt2"><span class="spin"></span><span class="small dim">正在检查，稍等…</span></div>`;
    } else if (c) {
      const items = c.items
        .map((it) => {
          const color =
            it.level === "ok" ? "ok" : it.level === "warn" ? "warn" : "err";
          const mark = it.level === "ok" ? "✔" : it.level === "warn" ? "⚠" : "✘";
          return `<div class="row tight">
            <span class="pill ${it.level === "ok" ? "high" : it.level === "warn" ? "medium" : "danger"}">${mark}</span>
            <span class="grow"><strong>${esc(it.name)}</strong> <span class="small">${esc(it.detail)}</span></span>
          </div>${
            it.fix
              ? `<div class="small dim mt2" style="padding-left:34px">修法：${esc(it.fix)}</div>`
              : ""
          }`;
        })
        .join("");
      checkHtml = `<div class="note ${c.ok ? "ok" : "warn"} mt2">${esc(c.summary)}</div>
        <div class="body mt3">${items}</div>
        <div class="row tight mt3">
          <button class="btn btn-ghost btn-sm" id="tCopyCheck">复制结果</button>
        </div>`;
    }

    // 体检与自检都要先知道「是哪个游戏」（命令层走 `current_game`），
    // 所以没选游戏时**不给一个点了必然报错的按钮** —— 换成带出口的空状态。
    // 「运行日志」与「导出诊断包」不依赖游戏，照常可用。
    const checkBody = !Store.gameRoot
      ? `<div class="body">${emptyHtml(
          "🎮",
          "这两个检查得先知道是哪个游戏",
          "体检要读游戏的区域设置、日文字体、运行库和目录写权限，自检要拿它的封包来回解一遍，所以得先选游戏。下面的「运行日志」和「导出诊断包」不依赖游戏，现在就能用。",
          "去选游戏",
          "tGoDetect"
        )}</div>`
      : `<div class="body">
      <p>游戏跑不起来、改完不生效、解包报错 —— 先点这两个按钮，它们会告诉你是哪一环出了问题。</p>
      <div class="row tight mt2">
        <button class="btn btn-primary" id="tHealth">游戏体检</button>
        <button class="btn btn-ghost" id="tSelfcheck">封包自检</button>
      </div>
      <p class="small dim mt2">体检查区域设置 / 日文字体 / 运行库 / 写权限；
        自检把封包解包再重打包、逐条目比对，确认改它不会坏。</p>
      ${checkHtml}
    </div>`;

    // 日志**按天累积**：今天每次开 STool 都往同一个文件追加。所以把「本次启动」
    // 那一行标出来，用户一眼能分清哪段是自己这次干的、哪段是今天早些时候的 ——
    // 否则很容易以为出现了「莫名其妙的记录」（2026-09 实际反馈）。
    const bootIdx = (() => {
      const ls = (this.log && this.log.lines) || [];
      for (let i = ls.length - 1; i >= 0; i--) {
        if (ls[i].includes("] [INFO] STool ") && ls[i].includes(" 启动（日志:")) return i;
      }
      return -1;
    })();

    const logBody = `<div class="body">
      ${
        this.logLoading
          ? `<div class="row tight"><span class="spin"></span><span class="small dim">正在读日志…</span></div>`
          : this.log && this.log.lines.length
          ? `<div class="small dim">显示最后 ${this.log.lines.length} 行（共 ${this.log.total} 行）。
               这份日志<strong>按天累积</strong>：今天每次打开 STool、每次操作都追加进来，
               所以上面的时间可能早于你这次开窗口。</div>
             <div class="logbox mono small">${this.log.lines
               .map(
                 (l, i) =>
                   (i === bootIdx ? `<span class="dim2">—— 本次启动之后 ——</span><br>` : "") + esc(l)
               )
               .join("<br>")}</div>`
          : `<div class="small dim">${esc(
              this.log ? `没有日志（${shortPath(this.log.path)}）` : "还没有日志"
            )}</div>`
      }
      <div class="row tight mt3">
        <button class="btn btn-ghost btn-sm" id="tReloadLog">刷新</button>
        <button class="btn btn-ghost btn-sm" id="tMoreLog">多看 500 行</button>
        ${
          this.log && this.log.path
            ? `<button class="btn btn-ghost btn-sm" id="tOpenLogDir">打开日志文件夹</button>`
            : ""
        }
      </div>
    </div>`;

    const diagBody = `<div class="body">
      <p>诊断包 = <strong>日志 + 环境（密钥已脱敏）+ 引擎检测 + 备份清单</strong>，打成一个 zip。
        出问题时把它发给能帮你看的人，对方一看就明白。</p>
      <div class="row tight mt2">
        <button class="btn btn-primary" id="tExport">导出诊断包</button>
      </div>
      ${
        this.diagPath
          ? `<div class="note ok mt3">已导出：<span class="mono small">${esc(
              shortPath(this.diagPath)
            )}</span>
             <div class="row tight mt2"><button class="btn btn-ghost btn-sm" id="tOpenDiag">打开所在文件夹</button></div>
           </div>`
          : ""
      }
    </div>`;

    return [
      this.cardWrap("先做这两个检查", "出问题时按顺序点", checkBody),
      this.cardWrap("运行日志", "出问题时的现场记录", logBody),
      this.cardWrap("导出诊断包", "要找人帮忙时用", diagBody),
    ].join("");
  },

  // -- 帮助 -----------------------------------------------------------------

  helpTabHtml() {
    const sec = (title, lines) =>
      this.cardWrap(
        title,
        "",
        `<div class="body"><ul class="bullets">${lines
          .map((l) => `<li>${l}</li>`)
          .join("")}</ul></div>`
      );

    return [
      `<div class="seg"><div class="card flat">
        <div class="card-head"><div class="card-title">大致流程</div></div>
        <div class="body">
          <p>左边按顺序走一遍就行，每一步都会自动带上你选的那个游戏：</p>
          <ul class="bullets">
            <li><strong>选游戏</strong> → 告诉它你要处理哪个游戏（后面每一步都认它）</li>
            <li><strong>取出素材</strong> → 把游戏里的图片、音乐、脚本拿出来</li>
            <li><strong>翻译文字</strong> → 提台词、翻译、再塞回去（汉化走这条）</li>
            <li><strong>改存档</strong> → 改金币、等级、道具数量</li>
            <li><strong>游戏里改数值</strong> → 游戏开着也能改，改完立刻生效</li>
            <li><strong>解锁全CG</strong> → 一键打开所有回想与画廊</li>
            <li><strong>装MOD</strong> → 用别人做好的补丁</li>
          </ul>
        </div>
      </div></div>`,
      sec("常见问题", [
        "扫不到内存、打不开游戏进程 → 用管理员身份重新打开 STool",
        "MV/MZ 游戏连不上 → 先把手动开着的游戏关掉再试（别多开）",
        "解包后文件在哪 → 这个工具箱的「设置」里能看到默认输出目录，或直接在「取出素材」页点「打开文件夹」",
        "改完点保存没用 → 有些游戏改完要重启游戏；「改存档」页改完要点「写回存档」",
        "外部工具下载不动 → 在上面「设置 → 代理」里填上代理地址再试",
      ]),
      sec("安全与还原", [
        "任何会改写游戏文件的操作，都会先在原文件旁留一份 <span class='mono'>.stool.bak</span> 备份",
        "改错了想还原：在「解锁全CG」页有「撤销上一次」，或手动把 <span class='mono'>.stool.bak</span> 改回原名",
        "STool 只改你自己电脑上的游戏文件，不会覆盖游戏自带的官方补丁",
        "遇到游戏自带的反作弊（EAC / BattlEye / Vanguard 等），STool 只会提示你，不会去绕过它",
      ]),
    ].join("");
  },

  loadingHtml(text) {
    return `<div class="seg"><div class="card"><div class="row tight">
      <span class="spin"></span><span class="dim">${esc(text)}</span>
    </div></div></div>`;
  },

  // -- 绑定 -----------------------------------------------------------------

  bind() {
    const on = (id, fn, ev) => {
      const e = $("#" + id, this.root);
      if (e) e.addEventListener(ev || "click", fn);
    };

    this.root.querySelectorAll("[data-tab]").forEach((b) =>
      b.addEventListener("click", () => {
        this.tab = b.dataset.tab;
        this.render();
      })
    );

    if (this.tab === "settings") {
      // 输入框只记值，不重绘（重绘会丢焦点）
      const bind = (id, field) => {
        const e = $("#" + id, this.root);
        if (e) e.addEventListener("input", (ev) => (this[field] = ev.target.value));
      };
      bind("tOut", "draftOut");
      bind("tProxy", "draftProxy");
      bind("tBase", "draftBase");
      bind("tModel", "draftModel");

      on("tSaveBasic", () => this.saveBasic());
      on("tSaveProxy", () => this.saveProxy());
      on("tSaveMtl", () => this.saveMtl());
      on("tOpenCfg", () => this.openConfigDir());

      this.root.querySelectorAll("[data-toolsave]").forEach((b) =>
        b.addEventListener("click", () => {
          const key = b.dataset.toolsave;
          const inp = this.root.querySelector(`[data-tool="${key}"]`);
          this.saveTool(key, inp ? inp.value : "");
        })
      );
      this.root.querySelectorAll("[data-dl]").forEach((b) =>
        b.addEventListener("click", () => this.downloadTool(b.dataset.dl))
      );
    }

    if (this.tab === "diag") {
      on("tGoDetect", () => setPage("detect"));
      on("tHealth", () => this.runCheck("health"));
      on("tSelfcheck", () => this.runCheck("selfcheck"));
      on("tCopyCheck", () => this.copyCheck());
      on("tReloadLog", () => this.loadLog());
      on("tMoreLog", () => {
        this.logTail = Math.min(5000, this.logTail + 500);
        this.loadLog();
      });
      on("tOpenLogDir", () => this.openLogDir());
      on("tExport", () => this.exportDiag());
      on("tOpenDiag", () => this.openDiagDir());
    }
  },

  // -- 动作：设置 -----------------------------------------------------------

  async saveBasic() {
    this.busy = true;
    const r = await Tauri.call("tools_save", { outputDir: this.draftOut });
    this.busy = false;
    this.applySave(r, "设置已保存");
  },

  async saveProxy() {
    const r = await Tauri.call("tools_save", { proxy: this.draftProxy });
    this.applySave(r, "代理已保存");
  },

  async saveMtl() {
    const r = await Tauri.call("tools_save", {
      mtlBaseUrl: this.draftBase,
      mtlModel: this.draftModel,
    });
    this.applySave(r, "机翻接口已保存");
  },

  async saveTool(key, path) {
    const r = await Tauri.call("tool_set", { key, path });
    this.applySave(r, `已保存 ${key}`);
  },

  applySave(r, okMsg) {
    if (!r.ok) {
      toast("保存失败", "err");
      this.err = r.err;
      this.render();
      return;
    }
    this.err = "";
    this.settings = r.data;
    toast(okMsg, "ok");
    this.render();
  },

  async openConfigDir() {
    const p = this.settings && this.settings.config_path;
    if (!p) return;
    const dir = String(p).replace(/[\\/][^\\/]*$/, "");
    const r = await Tauri.call("open_folder", { path: dir });
    if (!r.ok) toast(r.err, "err");
  },

  async downloadTool(key) {
    const t = (this.settings.tools || []).find((x) => x.key === key);
    toast(`正在后台下载 ${t ? t.label : key}，完成后路径会自动填好`, "ok");
    const r = await Tauri.call("tool_download", { key });
    if (!r.ok) {
      this.err = r.err;
      toast("下载失败", "err");
      await this.render();
      return;
    }
    await this.loadSettings();
  },

  // -- 动作：诊断 -----------------------------------------------------------

  async runCheck(kind) {
    this.checking = true;
    this.checkKind = kind;
    this.check = null;
    await this.render();
    const r = await Tauri.call(kind === "health" ? "tools_health" : "tools_selfcheck");
    this.checking = false;
    if (!r.ok) {
      this.err = r.err;
      toast(kind === "health" ? "体检失败" : "自检失败", "err");
      log(`tools: ${kind} 失败 ${r.err}`);
    } else {
      this.err = "";
      this.check = r.data;
      log(`tools: ${kind} → ${r.data.ok ? "通过" : "有问题"}`);
    }
    await this.render();
  },

  copyCheck() {
    if (!this.check) return;
    const text = this.check.text || this.check.summary;
    navigator.clipboard.writeText(text).then(
      () => toast("已复制到剪贴板", "ok"),
      () => toast("复制失败，可手动选中文字", "err")
    );
  },

  async openLogDir() {
    if (!this.log || !this.log.path) return;
    const dir = String(this.log.path).replace(/[\\/][^\\/]*$/, "");
    const r = await Tauri.call("open_folder", { path: dir });
    if (!r.ok) toast(r.err, "err");
  },

  async exportDiag() {
    toast("正在打包诊断包…", "ok");
    const r = await Tauri.call("tools_export_diag", {});
    if (!r.ok) {
      this.err = r.err;
      toast("导出失败", "err");
      await this.render();
      return;
    }
    this.diagPath = r.data;
    this.err = "";
    toast("诊断包已导出", "ok");
    await this.render();
  },

  async openDiagDir() {
    if (!this.diagPath) return;
    const dir = String(this.diagPath).replace(/[\\/][^\\/]*$/, "");
    const r = await Tauri.call("open_folder", { path: dir });
    if (!r.ok) toast(r.err, "err");
  },
};
