# STool Tauri 重构方案（v1）

> 目标：把 STool 从「Rust + egui 自绘控件」重构为「Rust 内核 + Tauri/WebView2 界面」，
> 重点放在**好看**与**好用**；不必要的复杂度一律精简；页面结构按**任务**而非**技术机制**重排，
> 让不懂技术的用户也能自己走完流程。
>
> 前置结论：Tauri 在本机已实测跑通（WebView2 153.0.4234.32 已装，IPC 与资源加载均正常）。

---

## 1. 现状诊断：为什么现在「不好用也不好看」

把问题归到根上，只有三条，其余都是这三条的派生：

| 根问题 | 具体表现 | 为什么是根问题 |
|---|---|---|
| **导航按技术机制切分** | 顶级入口是「资源解包 / 文本·汉化 / 运行时修改 / 补丁·MOD」 | 用户得先知道「封包」「注入」这些词，才能判断该点哪个。不匹配「我想改个金币」这种真实意图 |
| **页面是「控件堆放」而非「任务流」** | 每页把参数、选项、按钮平铺，没有主次；顶部只有一句副标题 | 用户看不出「第一步做什么、结果会出现在哪、失败了怎么办」 |
| **视觉无层级** | egui 默认主题：靠分隔线+灰阶区分区块；颜色只用于零星状态字 | 没有视觉焦点 = 不知道先看哪里；「用途不明的区域」正是无层级的后果 |

量化现状（实测）：

- GUI 层 **4,496 行**、**10 个页面**；`runtime.rs` 单页 1,229 行（占 pages 的 42%）
- 核心层（`features/` `formats/` `engines/` `memapi` `settings` `cli`）**≈24,700 行**，
  对 UI 框架**零依赖**（`grep -rl egui src/features src/formats src/engines` 为空）
- → 结论：**重构只动 GUI 那 4,496 行，内核一行不改**。这是本次重构最大的有利条件。

---

## 2. 验收标准（可验证，不写空话）

### 2.1 美观性

| 标准 | 判定方式 |
|---|---|
| 全局只用 **1 个主色** 表达「主行动/选中」，语义色仅 4 个（成功/提醒/危险/中性） | 在 CSS 变量表里数，UI 上不得出现表外颜色 |
| 间距遵循 **8px 栅格**（4/8/12/16/24），卡片圆角、描边、内边距全站一致 | 截图核对，同层级卡片必须像素级一致 |
| 中文字体统一、无豆腐块；**等宽字体只用于路径与数值** | 截图核对 |
| 每个区块都有标题或一句说明，**不存在「用途不明的区域」** | 逐区块过一遍，答不出「这块干嘛的」就删或折叠 |
| 不靠阴影分层（工具类应用用描边更干净） | 代码里无 `box-shadow` |

### 2.2 易用性

| 标准 | 判定方式 |
|---|---|
| 最常见的三件事（改金币 / 解包取素材 / 解锁全CG）**≤3 次点击**到达 | 手动走一遍数点击 |
| 任何页面**没有死胡同**——空状态必须给出动作按钮 | 逐页看空状态 |
| 任何失败提示都含 **「原因 + 修法」** | 逐条看错误文案 |
| **0 处技术术语出现在一级界面**（封包/注入/JSON Pointer/lz-string 一律换人话） | 一级界面全文搜关键词 |
| ≥5,000 行列表滚动稳定；500 条搜索结果响应 < 100ms | 用 `verify/demo_save` 实测并记录 |
| 首次启动到可操作 < 1.5s | 秒表 |
| 所有写入类操作前告知**备份位置**，并提供「撤销上一次写入」 | 实际点一遍 |

### 2.3 工程

| 标准 | 判定方式 |
|---|---|
| 核心层**逻辑零分叉**（`features/` `formats/` `engines/` 不为 Tauri 另写一份实现） | `git diff --stat` + 逐处确认改动性质。<br>**措辞已修正**：原先写的是「核心层零改动」——字面做不到，因为 `stool-rs/Cargo.toml` 必须加 `[features] default=["gui"]` 并把 `eframe`/`rfd` 改成 `optional`，否则 Tauri 壳要重复编译整套 egui。真正的目标是「**两个 GUI 共用同一份实现**」，这比「一行不动」更好，它消灭了「同义两份实现」这个漂移源。逐处核对见验收报告 §5.2 |
| `cargo clippy --all-targets -- -D warnings` 零警告 + `cargo test --tests` 全过 | 门禁命令 |
| 交付为**单个 exe**；运行时依赖仅系统 WebView2（Win10 1803+/Win11 自带） | 干净机器上试跑 |
| CLI（`stool-cli`）功能不受影响 | 跑既有 CLI 冒烟 |

---

## 3. 视觉风格与交互体验方向

### 3.1 设计语言：克制的现代桌面工具

对标 **VS Code / GitHub Desktop / 1Password** 这一类的观感，
**不要**网页后台管理系统的「蓝白表格 + 大量边框」风。核心区别：桌面工具**信息密度高、笔画细、留白靠栅格不靠大间距**。

### 3.2 色彩

单一主色（承担主按钮、选中态、进度），背景三级、文本三级、语义色四个：

```
背景：窗口底 / 卡片 / 悬浮        （三级，靠亮度差而非阴影）
文本：主 / 次 / 弱                （弱用于说明文字与路径）
主色：唯一，用于主按钮 + 选中态
语义：成功 / 提醒 / 危险 / 中性    （沿用已建立的 C_OK C_WARN C_DANGER C_MUTED 口径）
值着色：文本 / 数值 / 布尔 / 容器   （等宽字体区专用，帮助长列表里快速分辨）
```

把 STool 已经收敛好的 `gui/util.rs` 的 `C_*` 语义色**平移**成 CSS 变量，
这样 egui 版与 Tauri 版**同一套配色口径**，A/B 对比才有意义。

### 3.3 排版

- 正文 13px / 行高 1.6；小节标题 15px；页标题 19px；说明文字 12px
- 字重只两档：400 / 500（**不用 600/700**，桌面工具过粗会显廉价）
- **等宽字体只给路径和数值** —— 这是工具类应用最重要的区分度来源
- 中文统一 `Microsoft YaHei`；Web 侧直接可用的系统字体栈，不再手动抓 `msyh.ttc`

### 3.4 间距与形状

8px 基准栅格；卡片内边距 12/16、卡片间距 12；圆角：卡片 10px、控件 7px；
描边统一 1px 低对比灰，不叠加阴影。

### 3.5 交互方向（八条）

1. **先给结果，再给控制**：主行动按钮永远在内容区顶部且**唯一**，次要动作用弱按钮。
2. **就地反馈**：改完立即在行内看到新值，不整表刷新、不弹窗。
3. **零术语**：一级界面不出现「封包 / 注入 / JSON Pointer / lz-string」，换成
   「打包文件 / 加脚本 / 字段位置 / 加密存档」。
4. **渐进披露**：进阶能力（反修改诊断、原始结构、手动 ID 定位）默认收进「进阶」折叠面板——
   **不删能力，但不上主路径**。
5. **无死胡同**：每个空状态都带动作按钮（如「先去选游戏」）。
6. **错误可修**：提示格式固定为「哪里错了 → 怎么改」，例：
   「这个存档是加密的，STool 解不开 → 用 GARbro 或 KrkrExtract 解密后再回来」。
7. **写入可回溯**：写前告知备份路径，写后提供「撤销上一次写入」。
8. **不惊吓**：只有会动原文件的操作才弹模态确认；其余用页面内提示条 + 底部 toast。

---

## 4. 面向非技术用户的布局与信息层级

### 4.1 四层信息层级

| 层 | 内容 | 设计约束 |
|---|---|---|
| **0 层：入口** | 没有选中游戏时，**整屏只做一件事**：「选择游戏文件夹」或「把文件夹拖进来」 | 不给任何其他按钮，避免分心 |
| **1 层：导航** | 左侧 = **任务**（动词短语），不是模块名 | 最多 7 项；每项带一句 6 字以内的解释 |
| **2 层：页内** | 固定三段式：**说明条 → 主行动区 → 结果区** | 每页顶部一句话说清「这页干什么、点哪、结果在哪」 |
| **3 层：细节** | 列表 / 日志 / 原始数据 | 默认折叠；只在有结果时出现 |

### 4.2 三段式的具体形态

```
┌ 说明条 ─────────────────────────────────────────────┐
│ 一句话说明这页能干什么 + 一句「怎么做」（可折叠成 ? ）  │
└─────────────────────────────────────────────────────┘
┌ 主行动区 ───────────────────────────────────────────┐
│ 唯一的主按钮（大、主色） + 必要的输入（≤3 个）          │
└─────────────────────────────────────────────────────┘
┌ 结果区 ─────────────────────────────────────────────┐
│ 表格/列表/图；有进度时在这里出进度条与取消              │
│ 出问题时在这里出「原因 + 修法」提示条                   │
└─────────────────────────────────────────────────────┘
```

### 4.3 左侧导航的文案（面向意图，不面向机制）

| 新导航 | 一句话解释 | 原页面 |
|---|---|---|
| **① 选游戏** | 告诉 STool 你要处理哪个游戏 | home |
| **② 取出素材** | 把游戏里的图片音乐拿出来 | extract |
| **③ 看素材** | 直接浏览拿出来的东西 | preview |
| **④ 翻译文字** | 提台词、翻译、再塞回去 | text |
| **⑤ 改存档** | 改金币、等级、道具数量 | save |
| **⑥ 游戏里改数值** | 游戏开着也能改，改完立即生效 | runtime |
| **⑦ 解锁全CG** | 一键打开所有回想与 CG | （从 extract 里独立出来） |
| **⑧ 装MOD** | 安装/停用玩家做的补丁 | mods |
| **⑨ 工具箱** | 设置、日志、帮助、体检 | settings + log + help + health + selfcheck |

---

## 5. 页面结构的具体调整思路

### 5.1 结构性改动（不是换皮）

| # | 改动 | 理由 |
|---|---|---|
| 1 | **合并「设置 / 日志 / 帮助 / 体检 / 自检」为「工具箱」** | 这五个都是「出问题才用」，不该各占一个顶级入口。顶级入口 10 → 9，且都是任务 |
| 2 | **「解锁全CG」独立成页** | 目的性最强、被问最多的功能，现在埋在解包页里 |
| 3 | **「运行时修改」改名「游戏里改数值」，并把它内部的 4 种方式收成 2 + 进阶** | 「反修改保护识别」对非技术用户是天书。默认只留「按名字改」与「搜数值改」 |
| 4 | **存档页加「常用字段速改」区** | 不想搜索的用户可以一键改金币/等级/道具，不必理解字段路径 |
| 5 | **「原始 JSON 结构」移出主页面** | 改为「进阶」里的按钮，开独立窗口。主页面不再有「用途不明的滚动区」 |
| 6 | **所有页面统一三段式** | 消除「控件堆放」感，建立稳定的阅读顺序 |
| 7 | **引擎检测结果从「表格+分数+证据」改为「一句结论 + 可展开的判定依据」** | 用户只想知道「这是啥游戏、能不能处理」，不想看打分 |

### 5.2 逐页改法

- **选游戏**：整屏拖放区 + 「或点此选择文件夹」。检测完给一张卡：
  「识别为 RPG Maker MV 游戏 · 可以做：取素材 / 翻译 / 改存档 / 解锁全CG」，四个按钮直达。
- **取出素材**：主按钮「取出全部素材」+ 输出目录输入。结果区给「取出 1,234 个文件 → 打开文件夹」。
  进阶折叠里才是「只取某一类 / 重新打包」。
- **看素材**：左侧文件列表（虚拟化）+ 右侧预览；顶部一句「点左边任意文件即可预览」。
- **翻译文字**：把「CSV 回填」与「JSON 注入」两条通道改成**两条并列的可选路径卡**，
  每张卡上写清「哪种情况选这个」；不用「通道」这种词。
- **改存档**：搜索优先（已实现）+ **常用字段速改** + 进阶（原始结构）。
- **游戏里改数值**：默认两张卡「按名字改（推荐）」与「搜数值改」；进阶收「反修改诊断 / 补丁包」。
- **解锁全CG**：一页一件事——列出「已找到的解锁途径」+ 一个主按钮；写明「改前备份」「怎么还原」。
- **装MOD**：两栏「已安装 / 可用」+ 主按钮「安装 MOD 文件夹」。
- **工具箱**：三个分区——「设置」「诊断（体检/自检/日志导出）」「帮助」。

---

## 6. 技术方案

### 6.1 目录与复用

```
stool-rs/                  # 内核不动（features/ formats/ engines/ cli）
stool-tauri/               # 新增：Tauri 壳
  src/                     # 前端（静态 HTML/CSS/JS，无打包器）
  src-tauri/               # Rust 侧：命令 + 状态
```

- 内核复用：把 `stool-rs/src/lib.rs` 的 `pub mod gui;` 改成 **feature 门控**
  （`#[cfg(feature = "gui")]`），Tauri 侧只依赖内核、不开 `gui` feature，
  这样 **egui 版与 Tauri 版共存**，不需要一次性替换。
- 命令边界：每个页面 1~2 个命令，返回**已算好的展示数据**（不给前端整棵 JSON 树）。
  进度类任务用 Tauri **事件流**推送，不要每 tick 一次 IPC。
- 前端：静态 HTML/CSS/JS + `withGlobalTauri`，**不引入 npm 打包器**（少一个工具链）。

### 6.2 构建（含一个必须写进文档的坑）

**不要在 Tauri 项目里直接用 `cargo build`。** 实测：

```
tauri/build.rs:  let dev = !custom_protocol;
```

不开 `custom-protocol` feature 时，**即使 release 构建也会被判成 dev 模式**，
Tauri 会去找前端 dev 服务器，结果是**一个静默的白窗口**（没有任何报错）。
Tauri CLI 在生产构建时会自动加 `--features tauri/custom-protocol`，所以用 CLI 的人遇不到。

- 正常路径：装 Tauri CLI（`npm i -D @tauri-apps/cli` 拿预编译二进制，比 `cargo install` 快），
  用 `npx tauri dev` / `npx tauri build`。
- 想脱离 CLI 裸 `cargo build` 跑：在 `Cargo.toml` 里直接写死
  `tauri = { version = "2", features = ["custom-protocol"] }`（代价是 dev 热重载失效）。
- 另外 `tauri-build` 在 Windows 上**强制要求 `icons/icon.ico`**，缺了会构建失败。

### 6.3 已实测的成本（本机数据）

| 项 | 实测值 |
|---|---|
| 首次构建（含下载编译全部依赖） | **约 10 分钟**（期间踩了 2 次本机 `os error 5` 文件锁，重跑即过） |
| 增量构建 | **59~90 秒** |
| release 单 exe 体积 | **8.75 MB** |
| 运行时依赖 | 系统 WebView2（本机已装 153.0.4234.32） |
| 额外工具链 | node/npm（用于 Tauri CLI）；前端若不用打包器则不需要 node_modules |
| 捕获窗口截图 | WebView2 走 DirectComposition，`PrintWindow`/`CopyFromScreen` 都可能抓到白/黑；验证要走「前端回报 + 日志」或 devtools |

---

## 7. 实施顺序与风险

### 顺序（每步都可独立验收）

1. **打地基**：`stool-tauri/` 骨架 + 内核 feature 门控 + 设计令牌（CSS 变量）+ 导航外壳 + 选游戏页
2. **改存档页**（已有 egui 版做对照，最容易验证）
3. **取出素材 / 看素材**
4. **解锁全CG**
5. **翻译文字**
6. **游戏里改数值**（最重，1,229 行要拆）
7. **装MOD / 工具箱**
8. 灰度对照 → 达到验收标准后把 egui 版降级为可选 feature

### 风险

| 风险 | 应对 |
|---|---|
| 两套 GUI 并行期维护成本翻倍 | 严格限制并行期：Tauri 版到「日常够用」就切，不等 100% 功能对齐 |
| 「精简」误删真实需求（如反修改诊断） | **只折叠不删除**；进阶面板保留全部能力 |
| 大结果集走 IPC 变慢 | 命令只返回**可见所需**的字段；列表分页/懒加载；进度走事件流 |
| WebView2 在老机器缺失 | 启动时检测并给出「去微软官网装 WebView2」的一句话指引 |
| 截图/自动化验证受阻 | 保留环境变量调试钩子（沿用 `STOOL_*` 口径）+ devtools 作为验证通道 |

---

## 8. 精简清单：明确要砍/要收的东西

**直接砍（无价值或纯技术自嗨）**
- 引擎检测的「分数/证据表格」→ 收成一句结论 + 可展开依据
- 页面里散写的颜色常量 → 全走设计令牌

**收进「进阶」（能力保留，不上主路径）**
- 原始 JSON 结构树
- 反修改保护识别（`guard`）与页保护分布
- 运行时补丁包 / 手动 ID 定位 / 自检与体检的详细报告

**保留但降低存在感**
- 日志页 → 工具箱里的「导出诊断包」
- 使用指南 → 每页的 `?` 就地帮助（不再单独占一个顶级入口）

---

## 9. 实施进展

### 9.1 已完成（打地基 → 改存档 → 取出素材 → 看素材 → 翻译文字 → 解锁全CG → 游戏里改数值 → 装MOD）

| 项 | 落地位置 |
|---|---|
| 内核 feature 门控 | `stool-rs/Cargo.toml` 加 `[features] gui = ["dep:eframe", "dep:rfd"]`（**默认开**）；`lib.rs` 里 `#[cfg(feature = "gui")] pub mod gui;`。egui 版与 Tauri 版并存，Tauri 侧不重复编译 egui |
| 值 ↔ 文本单一实现 | 原先只在 `gui/util.rs` 的三个函数（`parse_edit_text` / `value_edit_text` / `type_hint`）搬进 `features/saves.rs` 并 `pub`，`gui/util.rs` 改为转发 —— 避免两个界面各写一套、日后漂移 |
| **op 分发单点** | 新增 `engines::exec_op` + `engines::OpPaths`：CLI（`cli::run_op`）、egui（`gui::exec_op`）、Tauri（`cmd::run_op_core`）三处共用**同一份** `match op`。否则每加一个 `Op` 要改三处，漏一处不会有编译错误，只表现为「某个界面点了按钮没反应」 |
| `human_bytes` 复用 | `features/precheck.rs` 由 `pub(crate)` 提升为 `pub` |
| Tauri 应用骨架 | `stool-tauri/`：`src-tauri`（命令层 `cmd.rs` + 状态 `main.rs`）+ `src`（静态前端） |
| 设计令牌 | `stool-tauri/src/css/tokens.css` —— 全站唯一取色/取尺寸来源，含深色版（只换令牌值） |
| 任务式导航外壳 | `stool-tauri/src/js/app.js`：分「上手 / 深入」两组，8 项 + 工具箱 |
| 三段式页面骨架 | `.say` / `.do` / `.out`（`app.css`），所有页面共用 |
| ① 选游戏 | js/pages/detect.js + `cmd::detect_game`（真实走 `Registry::detect_all`） |
| ⑤ 改存档 | js/pages/save.js + `cmd::{save_locations,save_files,load_save,search_save,set_save_value,commit_save}`（搜索虚拟化、常用字段速改、写回前自动备份） |
| **② 取出素材** | `js/pages/extract.js` + `cmd::{extract_assets,repack_assets,cancel_task,open_folder}`。进度走 `op:progress` **事件流**（不是轮询）；封包回写收进「进阶」折叠，且要勾选确认才可点 |
| 命令层单测 | `cmd.rs` 的 `#[cfg(test)] mod tests`：对 `verify/` 真机样本验检测 / 存档 / 解包。解包的正向路径用 `verify/tail_test`（**未加密** xp3）——`verify/komoguri` 三个 xp3 全加密，只能验失败路径 |
| 验证通道 | `log_line` 写日志文件 + `STOOL_GAME` / `STOOL_SAVE` / `STOOL_QUERY` / `STOOL_OUT` 启动钩子（与手动点击同一条代码路径） |
| 离线看界面 | `preview.html`：用同一套前端 + 内存假后端，不需要 WebView2 就能看界面长什么样 |
| **③ 看素材** | `js/pages/preview.js` + `cmd::{scan_media,read_media,open_file}`。左列表**虚拟化**（`ROW_H=34` 窗口渲染）+ 右侧按类型分流 `<img>`/`<audio>`/`<pre>`；图片音频内联成 **data URL**（不走 asset 协议，避免放宽文件 scope 且能在 `preview.html` 里跑） |
| **文本编码判定** | `features::preview::decode_text`（**内核唯一实现**，egui 与 Tauri 共用）：BOM → **无 BOM UTF-16 嗅探** → 严格 UTF-8 → Shift-JIS → 判为「不是文本」。改前只认带 BOM 的 UTF-16，真机 `tail_test` 的无 BOM `.tjs` 会掉进 Shift-JIS 解成一屏半角片假名 |
| **乱码双重闸门** | `acceptable()`：替换字符占比 >1/8 **或** 半角片假名占比 >1/12 → 返回 `DecodedText::Binary`。加密封包解出的密文会被 Shift-JIS「成功」解码成 `ﾏ0沢Sｴ`（零替换字符），只靠替换字符闸拦不住 —— 真机实测正常文本半角片假名 1~4%、密文 12~33% |
| **④ 翻译文字** | `js/pages/text.js` + `cmd::{text_extract,text_import,text_stats,csv_stats,text_make_json,text_inject,text_uninject,text_mtl,mtl_save,pick_file}`。**两条通道**并排摆出、各自说清代价：①「提取 → 翻 CSV → 回填」（通用、会改游戏文件）+ ②「生成 JSON → 运行时注入」（只四种引擎、不动游戏文件）。引擎**不支持注入时不当故障**：给出原因 + 指向通道 ① |
| **机翻是预填不是全自动** | `text_mtl` 走内核 `features::translate::{translate_csv,OpenAiCompat}`（OpenAI 兼容端点，预设 DeepSeek / 智谱 / 本地 Ollama）。批量与并发取 `settings::Config` 的 `mtl_batch/mtl_jobs`，**命令层不再定一套数字** —— 两处各写一份会出现「设置改了没生效」。密钥存**设置文件**（DPAPI 加密）而非游戏目录，避免随汉化包泄露 |
| **⑦ 解锁全CG** | `js/pages/unlock.js` + `cmd::{unlock_plan,unlock_run,unlock_backups,unlock_restore}`。界面**不自己判路线**，一律转述内核 `features::unlock` 的同一份策略表（`spec_or_generic` / `find_bundled_saves` / `find_save_dirs` / `pick_route`）—— 否则「界面说会走 A、内核实际走 B」，用户按界面点了却报错。三件事对应 §9.2 占位页承诺：① 列途径（`unlock_plan` 只读，进页面即渲染）；② 写入前说明备份位置（`.stool.bak` + 结果卡明写）；③ 写入后可撤销（`unlock_backups` 列备份 + `unlock_restore` 一键还原，只接受 `.stool.bak` 结尾，否则该命令就是任意文件覆盖入口）。手段是**数据**不是分支：`UnlockRoute` 序列化成 `routes[]`，新增引擎只改内核策略表，这一页不用动 |
| **解锁的「只读默认」是硬要求** | `unlock_plan_core` 是进页面就调的函数，一旦顺手写盘，用户「只是看了看」就被改了存档 —— 这类 bug 不报错、只造成损失。单测 `unlock_plan_is_read_only` 钉住它：跑完必须原存档内容一字未变、且不产生 `.stool.bak`；落地路径另由 `unlock_run_apply_copies_and_backs_up` 验「复制 + 留底 + 可还原」 |
| **⑥ 游戏里改数值** | `js/pages/runtime.js` + `cmd.rs` 的 22 个 `runtime_*` 命令。四张卡：① 按名字改（RPG Maker MV/MZ，CDP 直连 `features::runtime`）② 搜数值改（Cheat Engine 式，`features::memscan`）③ 进阶（回滚诊断 `features::guard` + 页保护分布）④ 补丁包（KiriKiri `features::xp3patch`）。术语对齐 Cheat Engine：数值类型显示 **「4 字节 / 8 字节 / 小数（4 字节）/ 小数（8 字节）」**（不是「整数 32 位」），过滤显示「等于 / 变大 / 变小 / 变化了 / 没变」 |
| **⑥ 的「冻结」不另起线程** | 内核 `Scanner` 没有后台线程；`runtime_freeze` 只写一次并置 `frozen` 标志，之后由前端 `setInterval` 调 `runtime_freeze_tick` 周期性重写。好处：页面一 `unmount` 就 `clearInterval`，**不留下改别人内存的孤儿线程**（这是改内存工具最不能有的东西） |
| **⑥ 的 `AppState` 字段必须能进 `spawn_blocking`** | `tauri::State<'_, T>` 的生命周期活不过闭包，`scanner` / `mvmz` 两个会话字段一律包成 `Arc<Mutex<Option<…>>>`，进闭包前 `.clone()` —— 详见 §9.5 pitfall ⑨ |
| **⑧ 装MOD** | `js/pages/mods.js` + `cmd::{mods_state,mods_preview,mods_install,mods_toggle,mods_uninstall}`。内核 `features::mods` **一行未改**（它本来就有安装/卸载/启停/冲突检测四件事 + 5 个单测）。界面做的三件事：① 把「安装 = 覆盖原文件」这个**代价**写在按钮上面；② 冲突在**装之前**就摆出来（见下）；③ 把「停用」（还原但留记录）与「卸载」（还原并删记录）分成两个按钮 —— 这是两件不同的事 |
| **⑧ 冲突要在装之前看见** | 内核 `install_mod` 的冲突校验发生在**写盘之前**（所以安全），但用户只能等报错。多一步 `mods_preview`：选好补丁目录就先拿 `mods::would_conflict` 演一遍，把「这个补丁和已装的 MOD 抢 N 个文件」连同具体路径一起摊开，让人提前决定「先停用哪个」还是「勾允许冲突覆盖」。**界面不自己判冲突**（`conflict_count` / `conflicts[]` 都来自内核同一份 `mods::conflicts`），否则会出现「界面说没冲突、内核装的时候却拒绝」 |
| **⑨ 工具箱** | `js/pages/tools.js` + `cmd::{tools_settings,tools_save,tool_set,tool_download,tools_health,tools_selfcheck,tools_log,tools_export_diag}`。按 §5.1 第 1 条把「设置 / 日志 / 帮助 / 体检 / 自检」**合并成一页**（它们都是「出问题才用」，不该各占顶级入口），内部分三块用 `tabs` 切换：设置 / 诊断 / 帮助 |
| **⑨ 不加新逻辑，只做搬运** | 体检走内核 `features::health::check`、自检走 `features::selfcheck::{find_archives,check_archive,check_dir}`、诊断包走 `features::diagpack::export`、工具下载走 `features::tools_dl`、设置读写走 `settings::{load,save}` —— **命令层一行业务判断都没有**，只把内核的 `Report` / `Outcome` 摊成界面能直接渲染的 `CheckRow`。内核这六个文件本轮 `git diff` 全为空 |
| **⑨ 失败必带「修法」** | 内核 `precheck::Item` 的 `fix: Option<String>` 直接转成 `CheckRow.fix`，界面在每条问题下缩进渲染「修法：…」。单测 `check_rows_map_every_precheck_level` 钉住三档映射（`Ok→ok` / `Warn→warn` / `Fail→fail`）与「`Ok` 项不得带修法、`Fail` 项必须带」 |
| **⑨ 密钥永不回传前端** | 设置区需要显示「机翻密钥配没配」，但**绝不把密钥本体送过 IPC**：`SettingsOut` 只有 `mtl_key_set: bool`，没有密钥字段。改密钥一律去「翻译文字」页（同一个 `mtl_save`）。单测 `tools_settings_never_returns_the_mtl_key` 序列化整个结构后断言 JSON 里不存在密钥字段 |
| **⑨ 日志尾部的上下限** | `tools_log` 的 `tail` 参数夹到 `[50, 5000]` —— 太小没意义，太大一次 IPC 把几十 MB 文本推给前端会卡住渲染。`CheckOut` 同时给**结构化 `items[]`**（界面渲染）与**原始 `text`**（「复制结果」按钮），两者由同一份报告生成，不会不一致 |
| **⑨ 没选游戏时不摆「点了必然报错」的按钮** | 体检 / 自检都要先知道是哪个游戏（命令层走 `current_game`），而「工具箱」在导航的「其他」组里、任何时候都进得去。改前照摆两个按钮，点下去只换来一句 `还没选游戏`。改后诊断卡的检查区换成带「去选游戏」的空状态，并明说「日志与诊断包不依赖游戏，现在就能用」—— **降级的是能力，不是整页** |
| **空状态一律要给出口**（§2.2 逐条核出来的） | 全站 8 处 `emptyHtml` 里只有 `mods.js` 的「还没装过 MOD」是 `btnLabel=""`：动作卡虽然就在正上方，但空状态卡落在整页**最底部**，用户自上而下读完正好停在一片空白上。补了「选一个补丁目录」按钮（点了把上方 `#mPick` 滚进视野并聚焦）。另：`cmd.rs` 66 个 `Err` 站点里有 **1** 处缺「改法」（`runtime_patch_remove` 的「当前引擎没有补丁包这回事」），已补齐 —— 现在 **66/66 都带修法** |

### 9.2 尚未搬过来的页面

无。九个任务页（①~⑨）全部落地。

剩下的都是**非页面类**收尾：§2.3 要求的「打包成单个 exe 并在干净机器上试跑」、
以及 §2.2 里「首次启动到可操作 < 1.5s」「≥5000 行列表滚动稳定」这类要用秒表与真机量的指标。
另有几项与本次重构无关的历史待办留在任务列表里（如 `formats/pfs.rs` 的 Artemis PFS 解包）。

### 9.3 明确的下一步

**收尾三条已完成，结果见 [`docs/TAURI验收报告.md`](TAURI验收报告.md)。**

1. ~~端到端真机跑一遍~~ ✅ **已做**：环境变量钩子起真 release exe，`detect` 页在真样本上认出
   KiriKiri 140 分、`tools` 页取到设置 7 项；启动到可操作 **822 ms / 549 ms**（达标线 1500 ms）。
2. ~~打包~~ ✅ **已做**：**不需要 Tauri CLI / npm** —— `bundle.active=false` + `frontendDist:"../src"`
   + `custom-protocol` 写死在 `Cargo.toml`，裸 `cargo build --release` 即得单个
   `stool-tauri.exe`（**13.5 MB**），运行时只依赖系统 WebView2。
3. ~~按 §2 三张表逐条核~~ ✅ **已做**：§2.1 5/5 达标、§2.2 5 达标 / 2 部分、§2.3 3 达标 / 1 措辞需修正。
   核表过程中**修掉 3 个真缺口 + 1 个会静默发旧界面的构建坑**（见验收报告 §8/§9）。

**仍未做（需要你点头或需要真机交互）**：

1. **§5.1 技术术语**：`封包` 9 处 / `注入` 12 处（全集中在 `text.js` 一个概念）/ `解包` 3 处。
   这是**产品语气**的决定，只核未改，建议与成本已列在验收报告 §5.1。
2. **§5.2 一键撤销**：`写回存档` 与 `重新打包` 只写了备份位置、没有还原按钮（其余 4 条写入路径都有）。
   内核 `features::restore` 已就绪（`find_backups` / `restore_one`，237 行 4 个函数，CLI 的
   `stool restore` 就是薄转录），**补齐成本低**，但要同时加两页的 UI + 测试才算闭环，
   所以没有半做。
3. **两项需真机交互才量的指标**：`500 条搜索结果 <100ms`、`诊断包 zip 与 egui 版逐字节比对`。

### 9.4 验收方式（本机可复现）

| 层次 | 怎么验 | 命令 / 位置 |
|---|---|---|
| 内核门禁 | clippy 零警告 + 测试全绿 | `cd stool-rs && cargo clippy --all-targets -- -D warnings && cargo test --tests` |
| 命令层 | 对真机样本跑单测（检测 / 存档 / 解包 / 进展回调 / 素材扫描与解码 / 解锁计划与落地） | `cd stool-tauri/src-tauri && cargo test` |
| 前端语法 | 逐个文件解析 | `node --check src/js/**/*.js` |
| 界面 | **离线看**：同一套前端 + 内存假后端 | 打开 `stool-tauri/preview.html` |
| 界面截图 | headless Edge 渲 `preview.html`（绕开 WebView2 抓像素不可靠） | `msedge --headless=new --screenshot=D:/out.png "file:///D:/…/preview.html?kind=text#preview"` |
| 真机运行 | 环境变量钩子走与手点相同的代码路径 | `STOOL_PAGE=extract STOOL_GAME=<游戏> STOOL_OUT=<目录> stool-tauri.exe` |

> **截图两个坑**（都踩过）：
> ① `--screenshot` 的目标**必须是绝对路径**，给相对路径会报 `Failed to write file … 拒绝访问 (0x5)`，
> 而 Edge 退出码仍是 0 —— 不看文件是否真的生成，会以为截好了；
> ② `--user-data-dir` 要指向**项目内固定目录**（如 `shots/.edgeprof`），不要用 `mktemp -d`
> 造的临时目录，否则 headless 起不来、静默不写文件。
> 深浅两版要**各用一份干净 profile**：`localStorage` 里的主题会跨运行留存，
> 共用 profile 会让「深色版」截出来和亮色一模一样（MD5 都相同）。

> **为什么不把截图当验收手段**：WebView2 走 DirectComposition，`PrintWindow` / `CopyFromScreen`
> 都可能抓到白屏或黑屏；且本机反复 `taskkill` 会破坏 WebView2 的窗口类状态
> （`Failed to unregister class Chrome_WidgetWin_0. Error = 1411`），此后启动会静默失败。
> 所以界面验收以 `preview.html` 与「前端回报 + 日志」为准。

### 9.5 踩过的坑（按发现顺序，后面加页前先读这一节）

| # | 坑 | 现象 | 修法 |
|---|---|---|---|
| ① | `custom-protocol` feature 不加 | 窗口**全白**、不报错 | `Cargo.toml` 里 `default = ["custom-protocol", "tauri/custom-protocol"]` |
| ② | WebView2 抓像素不可靠 | 截图白屏 | 界面验收走 `preview.html`；真机走 `log_line` + `STOOL_TUI_LOG` |
| ③ | 非 `async` 命令跑主线程 | 一次读盘就把所有 IPC 冻住 | IO 命令一律 `async` + `tauri::async_runtime::spawn_blocking`（封装成 `offload()`） |
| ④ | `preview.html` 的预置脚本位置 | 改页面对象字段被 `mount()` 里那次 `render` 覆盖 | 预置脚本必须放**所有页面脚本之后**，且用 `setTimeout` 推迟到 mount 完成 |
| ⑤ | 深浅两版共用 `--user-data-dir` | 深色版截出来和亮色**一模一样（MD5 相同）** | 主题存 `localStorage`，两版各用一份干净 profile |
| ⑥ | JS 注释里写 `<script>` 字面量 | HTML 解析被那句注释劈开，脚本块断掉 | 注释里改写「脚本块」 |
| ⑦ | `--virtual-time-budget` 太小 | 异步 hook 没跑完就截图（截到空态） | 给到 `12000` |
| ⑧ | 截图 URL 漏了 `#page` | 截出来是默认页，和上一张完全相同 | 必须带 `#runtime` 这类 hash |
| ⑨ | `tauri::State` 进 `spawn_blocking` 闭包 | 编译期 E0597/E0521（生命周期活不过闭包） | `AppState` 里要进闭包的字段包成 `Arc<Mutex<Option<…>>>`，闭包前 `.clone()`；辅助函数签名相应从 `&State` 改成 `&Arc<Mutex<…>>` |
| ⑩ | 预览期引用假后端对象 | `ReferenceError` 静默中断 hook，截到默认态 | `preview.html` 里的 `MOCK` 是 **IIFE 局部变量**，别的脚本块拿不到；必须显式挂到 `window.__MOCK_*` |
| ⑪ | 数值类型下拉显示「4 字节（4 字节）」 | 标签本身已含单位，又拼了一次字节数 | 拼字节数前先判断 `label` 是否已含「字节」 |
| ⑫ | 列表行用 `.row`（flex-wrap）排「状态+名字+文件数+按钮组」 | 内容一多按钮被甩到第二行，看着像界面坏了 | 改用 `.modhead`（flex + `.modacts { margin-left:auto }`），按钮组固定贴右 |
| ⑬ | 用 `new Function` 校 `preview.html` 内联脚本时按行号切片 | 切多/切少一行就误报 `FAIL`，白查半天 | 用 `lastIndexOf('<script>')` + 其后第一个 `</script>` 精确取块，别手算行号 |
| ⑭ | 侧栏导航样式 `.nav-item` 拿来当页内分段控件 | 三个标签被排成**整列**（那是侧栏的样式），占满整屏 | 页内切换另用 `.tabs` / `.tab`（水平药丸 + `.on` 高亮），别复用导航类 |
| ⑮ | 一次消息里对**同一个文件**发两次 `Edit` | 只有后一次生效，前一次静默丢失（曾让 `window.__MOCK_TOOLS__` 定义凭空消失） | 同一文件的多处改动**合并成一次 Edit**，或分两条消息发 |
| ⑯ | 默认的 `build.rs` **不声明对前端资源的依赖** | 改完 `src/js/*.js` 直接 `cargo build --release`，构建脚本不重跑 → exe 里嵌的还是**上一版**前端；而内嵌资源是 brotli 压缩的，`grep -a <中文串> exe` 一个都查不到，光看「编译成功」发现不了 | `build.rs` 里补 `println!("cargo:rerun-if-changed=../src");`。<br>根因：`tauri-build` 只为 `tauri.conf.json` 与 `capabilities/` 打 `rerun-if-changed`（见 `target/<profile>/build/stool-tauri-*/output`），**不管 `frontendDist`**。<br>**判定办法**（别靠 grep）：比对时间戳 —— `<profile>/build/stool-tauri-*/out/tauri-codegen-assets/` 必须晚于 `src/js/` 的 mtime |

