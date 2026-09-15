# STool Tauri 版 · 验收报告

> 对应 `docs/TAURI重构方案.md` §2 的三张验收表 + §9.3 的三条收尾项。
> 本报告只写**能复现的东西**：每条结论都带命令或文件位置，不写「体验良好」这类空话。
>
> **后续变更（2026-09，已超出本报告范围）**：验收通过后，**旧 egui 版被整体删除**
> —— `stool-rs/src/gui/`、`stool` 二进制、`eframe`/`rfd` 依赖与 `[features] gui` 门控、
> `shots/capture.ps1` / `crop.ps1` 及全部 egui 时期截图均已移除。现在内核只出
> `stool-cli.exe`，界面只有 `stool-tauri.exe`。因此下文凡涉及「两版并存 / 共用 /
> feature 门控」的表述，请按「迁出前的状态」理解；形态以 `docs/ARCHITECTURE.md` §1 为准。
> 删除后重跑门禁：`stool-rs` **286 passed / 0 failed**、`stool-tauri` **51 passed / 0 failed**，
> 两侧 clippy 均零警告。

---

## 0. 结论先行

| 表 | 条数 | 达标 | 部分达标 | 未达标 |
|---|---|---|---|---|
| §2.1 美观性 | 5 | **5** | 0 | 0 |
| §2.2 易用性 | 7 | **7** | 0 | 0 |
| §2.3 工程 | 4 | **4** | 0 | 0 |

- **交付物**：单个 `stool-tauri.exe`，**13.4 MB**（14,052,352 字节），运行时只依赖系统 WebView2。
- **启动到可操作**：冷启（该 exe 首次运行）**822 ms**；热启 **549 ms**。达标线 1500 ms。
- **门禁**：`clippy --all-targets -- -D warnings` 退出 0；`cargo test --tests` **51 passed / 0 failed**。
- 本轮在核表过程中**修掉 3 个真缺口 + 1 个会静默发旧界面的构建坑**（§4）。
- §5 里最初挂着的 2 条「部分达标」（技术术语 / 一键撤销）**已在本轮改完**，见 §5.1 与 §5.4。

---

## 1. 验收环境

| 项 | 值 |
|---|---|
| 平台 | Windows 10/11（Win32）+ Git Bash + MSVC VC143 |
| 工具链 | rustup stable-x86_64-pc-windows-msvc；`CARGO_INCREMENTAL=0` |
| 真机样本 | `verify/tail_test`（未加密 KiriKiri `.xp3`，189 条目 / 184.3 MB）—— 见 `.gitignore`，不入库 |
| 存档样本 | `verify/demo_save/*.json` |
| 前端 | 静态 `stool-tauri/src/**`，编译期内嵌进 exe（brotli 压缩） |
| 界面验收手段 | `stool-tauri/preview.html` + headless Edge 截图（**不用** WebView2 抓像素，理由见方案 §9.4） |

---

## 2. §2.1 美观性（5/5 达标）

| # | 标准 | 结论 | 证据 |
|---|---|---|---|
| 1 | 全局 1 个主色 + 4 个语义色 | ✅ | `src/css/tokens.css`：`--accent` 一组；语义色恰好 4 个 `--ok / --warn / --danger / --muted`（各带 `-soft`）。深浅两套同构（`:root` 与 `[data-theme=dark]`） |
| 2 | 间距走 8px 栅格，圆角/描边/内边距全站一致 | ✅ | `--s1..--s6 = 4/8/12/16/24/32`。圆角按角色分层且同类取值一致：`--r-card` 10px（卡片）/ `--r-ctl` 7px（控件）/ 8px（卡片内次级容器，7 个同级元素统一）/ 6px（提示条）/ 999px（药丸）。<br>**小建议**：8px 这一层还是裸值，可再收一个 `--r-inner`（不影响观感，纯一致性） |
| 3 | 中文统一、无豆腐块；等宽只用于路径与数值 | ✅ | `--font: "Microsoft YaHei UI", "Microsoft YaHei", system-ui…`；`--mono` 只落在 `.mono` / `.vrow-key` / `.vrow-val` / `.field.mono` / `.field-num`。全仓 **0 处** `@font-face` / `fonts.googleapis` / `.ttc` / `.ttf` —— 不再手动抓 `msyh.ttc` |
| 4 | 每个区块都有标题或一句说明 | ✅ | 9 页逐张截图核对：每张卡都有 `card-title`（+ 多数带 `card-hint`）；无孤立区块 |
| 5 | 不靠阴影分层 | ✅ | `grep -rn box-shadow src/` → **0 处**。分层靠 `--line` 1px 描边 + 三级背景亮度差 |

---

## 3. §2.2 易用性（7 达标）

| # | 标准 | 结论 | 证据 |
|---|---|---|---|
| 1 | 三件常做事 ≤3 次点击到达 | ✅ | 点击路径（游戏已选的前提下）：<br>· **改金币**：侧栏「改存档」→ 点字段行 → 改值 + 「改成这个值」→「写回存档」= 到达 1 击 / 完成 3 击<br>· **解包取素材**：侧栏「取出素材」→「选择输出目录」→「开始取出」= 3 击（`STOOL_OUT` 已设时只需 2 击）<br>· **解锁全CG**：侧栏「解锁全CG」→「解锁」= 2 击（路线由内核策略表自动选定） |
| 2 | 无死胡同：空状态必须给动作按钮 | ✅ **本轮修掉 2 处** | 全站 8 处 `emptyHtml` 逐处核：唯一一处 `btnLabel=""`（`mods.js` 的「还没装过 MOD」）已补「选一个补丁目录」按钮（点了把上方 `#mPick` 滚进视野并聚焦）。<br>另修：**没选游戏时，工具箱诊断卡不再摆两个点了必然报错的按钮**，换成带「去选游戏」的空状态，并明说「日志与诊断包不依赖游戏，现在就能用」——降级的是能力不是整页（设置/帮助两页无游戏时照常可用） |
| 3 | 失败提示必含「原因 + 修法」 | ✅ | `cmd.rs` 共 **66 个 `Err` 站点，66 个都带「改法」**（本轮补上最后 1 处：`runtime_patch_remove` 的不支持分支）。体检/自检的结构化项由内核 `precheck::Item.fix` 直供，界面缩进渲染成「修法：…」 |
| 4 | **0 处技术术语出现在一级界面** | ✅ **已达标（本轮改完）** | 见 §5.1 |
| 5 | ≥5000 行列表滚动稳定；500 条搜索 <100ms | ⚠️ 前半 ✅ / 后半**未实测** | 两处大列表都是**自写窗口渲染**：`save.js::paintRows` 与 `preview.js` 用固定行高 `ROW_H=30` + `BUFFER=8`，只渲染可见窗口（`inner.style.height = total*H` 撑滚动条），滚动监听带 `{passive:true}`，O(可见行) 而不是 O(总行)。搜索结果由内核侧限 500 条并在界面注明。<br>**未实测**「500 条 <100ms」——需要在真机上用秒表/DevTools 量，本轮没做（不编造数字） |
| 6 | 首次启动到可操作 < 1.5s | ✅ | 见 §6，实测 **822 ms / 549 ms** |
| 7 | 写入前告知备份位置，并提供「撤销上一次写入」 | ✅ **达标 6/6** | 告知备份位置 **6/6 全有**；一键撤销 **6/6**：<br>✅ 解锁全CG（`unlock_backups` + `unlock_restore`）<br>✅ 翻译注入（移除注入）/ 回填（`.stool.bak`）<br>✅ 装MOD（停用 / 卸载两按钮）<br>✅ 改内存（`runtime_undo`）<br>✅ **写回存档**：写回成功后原地出现「还原到改之前」（`restore_one`），并列出该目录全部备份（`restore_list`）<br>✅ **重新打包**：结果卡里直接给「还原」，进阶折叠里也有常驻入口 |

---

## 4. §2.3 工程（4 达标）

| # | 标准 | 结论 | 证据 |
|---|---|---|---|
| 1 | 核心层零改动 | ✅ **按「逻辑零分叉」口径达标** | 见 §5.2：事实是「**逻辑零分叉**」，不是「一行没动」（方案 §2.3 第 1 条措辞已同步改掉） |
| 2 | clippy 零警告 + 测试全过 | ✅ | `cargo clippy --all-targets -- -D warnings` → 退出 0；`cargo test --tests` → **51 passed; 0 failed** |
| 3 | 单个 exe，仅依赖系统 WebView2 | ✅ | `stool-tauri/src-tauri/target/release/stool-tauri.exe` = **14,052,352 字节**（13.4 MB）。`tauri.conf.json` 的 `bundle.active=false` + `frontendDist:"../src"` + `custom-protocol` 写死在 `Cargo.toml` → 裸 `cargo build --release` 即得单文件（**不需要 Tauri CLI / npm**） |
| 4 | CLI（`stool-cli`）功能不受影响 | ✅ | 当时两条都验了：<br>① `cargo build --release`（默认 feature，含 egui）✅<br>② `cargo build --release --no-default-features --bin stool-cli` ✅（**证明 `gui` 门控成立** —— 关掉 eframe/rfd 也能编出 CLI，这正是 Tauri 壳复用的前提）<br>③ 真机冒烟见 §7<br>**现状更新**：`gui` 门控与 egui 版已删除，`stool-rs` 只剩内核 + `stool-cli`，`cargo build --release` 直接产出 `stool-cli.exe` |

---

## 5. 偏离项（本轮已全部处理）

### 5.1 「0 处技术术语出现在一级界面」——本轮已改完

统计口径：剥离 JS/HTML 注释后统计（即**会被渲染或被逻辑引用**的出现次数，不含注释）。

**改前**：

| 术语 | 处数 | 分布 |
|---|---|---|
| 注入 | **12** | 全部在 `js/pages/text.js`（「通道 ② · 运行时注入」这一个概念） |
| 封包 | **9** | `runtime.js` 3、`tools.js` 3、`extract.js` 2、`detect.js` 1 |
| 解包 | 3 | `js/pages/tools.js`（外部工具的功能说明） |
| JSON Pointer | 1 | `js/pages/save.js`（在**折叠的**「进阶：这是什么文件、字段位置怎么看」里，不算一级界面） |
| lz-string / 反编译 | **0** | — |

**本轮改动（只动「用户要在那里做选择」的两处）**：

| 位置 | 改前 | 改后 |
|---|---|---|
| `text.js` 卡片标题 | 通道 ② · 运行时注入（推荐 · 不改游戏文件） | **通道 ② · 不改游戏文件（推荐）** |
| `text.js` 状态/按钮 | 已注入 / 未注入；② 注入；③ 移除注入（还原） | **已生效 / 未生效；② 让译文生效；③ 撤回（还原成原文）** |
| `text.js` 折叠标题 | 这个引擎的注入有什么限制 | **这个方式有什么限制** |
| `text.js` 副标题（`text.js:167` 空状态） | 能不能走「运行时注入」 | **能不能走「不改游戏文件」** |
| `extract.js:57` 首屏说明 | 把游戏**封包**里的文件解出来 | 把游戏**打包文件**里的内容解出来 |
| `extract.js` 进阶提示 | 原**封包** / 把改过的文件塞回游戏 | **原来的打包文件** / （保留「重新打包」这个动作名） |
| `detect.js` 能力卡 | 运行时汉化 | **不改游戏文件汉化** |

**保留的术语及理由**（不是漏改）：

- `runtime.js` / `tools.js` 里的「封包」：那里必须**给一个对象命名**（补丁包 `patchN.xp3`、自检要「把封包解了再打回来」），换成「打包文件」反而更含糊。
- 内核返回值里的「注入」（如 `注入完成：96 条翻译已挂载…`）：**界面只转述内核文案**，不在 UI 层重写 —— 否则同一次操作在 CLI 与 GUI 会看到两种说法。
- `save.js` 的「JSON Pointer」：留在**折叠的**「进阶」里，且同屏给了人话解释（`/party/_gold` 形如…）。

**额外修掉的一处死胡同**（核表时顺手发现）：`detect.js:178` 认不出引擎时提示「在『取出素材』里选『**自定义封包**』手动试」——**界面上根本没有这个入口**。已改为给出三个真能点的卡：游戏里改数值 / 看素材 / 工具箱（可复现：`preview.html?engine=unknown#detect`，截图 `43-detect-unknown.png`）。

### 5.2 「核心层零改动」——措辞需修正为「逻辑零分叉」

`git diff --stat -- stool-rs/src/{features,formats,engines}` 显示确实有改动（650+ / 201-）。逐处看过后，**没有一处是为 Tauri 另写一份实现**：

| 改动 | 性质 |
|---|---|
| `stool-rs/Cargo.toml` 加 `[features] default=["gui"]`，`eframe`/`rfd` 改 `optional` | **为了让 Tauri 壳不重复编译整套 egui** —— 这是迁移期的硬前提，无法绕开。**该改动已被后续的 egui 删除再次抹平**：门控、可选依赖、`stool` 二进制都已移除 |
| `features/preview.rs` 把「文本分类表 / 编码判定」下沉到内核 | **共享化**：改前只有 egui 侧有，Tauri 要用就得抄一份；下沉后界面与 CLI 取同一份（漂移的表现就是「界面说 A、内核做 B」）。界面侧那份已随 egui 版删除，现在内核这份是唯一实现 |
| `features/saves.rs`、`features/mod.rs`（新增 `guard` 模块）等 | 属早前几轮的工作，与本次重构无关 |

所以准确表述是：**内核逻辑零分叉 —— 界面与 CLI 共用同一份实现；唯一的强制改动是 `Cargo.toml` 的 feature 门控（现已随 egui 版删除回退）。** 这比「一行不动」更好，因为它消灭了「同义两份实现」这个漂移源。建议把 §2.3 第 1 条的措辞改掉（已在方案文档中更新）。

### 5.3 两项需要真机交互才能量的指标（本轮未做，不编数字）

- 「500 条搜索结果响应 < 100ms」—— 需要真机 + DevTools 录一次。
- 「诊断包 zip 内容与旧版一致」—— 需要两个版本各导一次包再比对（本轮只验了 `tools_export_diag` 走的是与 CLI 同一个内核函数 `features::diagpack::export`；egui 版删除后这条只剩「与 CLI 比对」这一种做法）。

### 5.4 「一键撤销」——本轮已补完最后两处（写回存档 / 重新打包）

内核其实**早就有**这套能力：`features::restore`（`find_backups` / `restore_one` / `apply_repack` / `archive_toggle`，237 行，4 个单测），CLI 的 `stool restore` 也只是薄薄一层转述。缺的只是**命令层与界面**，本轮补齐。

**新增命令层**（`stool-tauri/src-tauri/src/cmd.rs`，共 2 个命令 + 2 个纯函数）：

| 命令 | 入参 | 说明 |
|---|---|---|
| `restore_list` | `dir` | 列出该目录下所有 `<原文件>.stool.bak`，复用已有的 `FileRow`（名称/大小/时间） |
| `restore_one` | `bak` | 把备份内容写回原文件；**备份保留**，可反复还原 |

两个命令都是 `async` + `offload()`（走 `spawn_blocking`）—— 遍历目录/复制大文件会阻塞主线程，非 `async` 会冻结整个 IPC（坑②）。参数校验抽成纯函数（`backup_rows_of` / `backup_path_checked`）以便同步单测，符合本文件既有的测试写法。新增 **4 个单测**（`restore_list_finds_backups_with_size_and_age` / `restore_list_rejects_missing_dir_with_a_fix` / `restore_one_rejects_paths_that_are_not_backups` / `restore_one_puts_the_old_content_back_and_keeps_the_backup`），门禁从 47 涨到 **51 passed**。

**为什么 `restore_list` 要调用方传目录**：重新打包会命中**多个**封包（Artemis 遍历全部 `.pfs`），而 `OpResult` 只回一条文本、不带目标路径列表。所以「列出备份」按目录查，由调用方决定查哪儿 —— 存档页传存档所在目录，解包页传游戏目录。

**两处的备份语义不同，界面文案必须跟着不同**（这是本轮最容易写错的地方）：

| 场景 | 内核写入方式 | 备份代表什么 | 界面措辞 |
|---|---|---|---|
| 写回存档（`SaveDoc::save`） | 每次 `fs::copy` **无条件覆盖** `.stool.bak` | **上一次写之前**的样子 | 「**还原到改之前**」 |
| 重新打包（`settings::backup_once`） | **已存在就不覆盖** | **第一次打包前**的原始文件 | 「退回**原来的**打包文件」 |

> `backup_once` 必须不覆盖的原因写在 `settings.rs:432-437`：回填/重封包/注入可以反复执行，第二次若无条件覆盖，备份内容就会从「原始文件」变成「上一次改过的文件」，原始状态永久丢失、`restore` 也救不回来。

**界面**（共用 `ui.js` 里新加的 `mountBackups()`，两页不重复写一份）：

| 页面 | 入口 |
|---|---|
| 改存档 | ① 写回成功后，写回卡片里**原地**出现「还原到改之前」+ 备份路径；② 新增折叠「备份与还原（改坏了用这里）」，列出该存档目录的全部备份 |
| 取出素材 | ① 重新打包成功后，结果卡**特意换了一套渲染**（`repackResultHtml`）—— 打包没有「输出文件夹」可打开，重点是「怎么退回去」；② 进阶折叠里常驻「备份与还原」入口（重启后没有结果卡，这里才是入口） |

**可复现**（离线截图，无需真机）：

```bash
msedge.exe --headless=new --disable-gpu --hide-scrollbars --window-size=1180,1900 \
  --virtual-time-budget=15000 --user-data-dir=D:/STool/shots/.ep_x \
  --screenshot=D:/STool/shots/tauri/36-save-undo.png \
  "file:///D:/STool/stool-tauri/preview.html?save=undo#save"
```

对应截图：`36-save-undo.png`（写回后就地还原条）、`37-save-backups.png`（备份列表）、`38-save-backup-empty.png`（空状态给出路）、`39-extract-repack-done.png`（打包结果卡的还原入口）、`40-extract-backups.png`（进阶折叠里的常驻入口）、`42-save-undo-dark.png`（深色版）。

**新增预览参数**：`?save=undo` / `?save=bak` / `?save=bakempty` / `?repack=done` / `?repack=bak` / `?engine=unknown`。

---

## 6. 真机端到端跑（§9.3 第 1 条）

用 §9.4 的环境变量钩子起**真实 release exe**（走与手点完全相同的代码路径），前端回报写进 `STOOL_TUI_LOG`：

```
STOOL_PAGE=detect STOOL_GAME=D:\STool\verify\tail_test STOOL_TUI_LOG=shots\tauri-run-detect.log stool-tauri.exe
```

**`detect` 页日志（原文）**

```
app.js 启动完成；__TAURI__=ok
app.js detect: mount 进入
app.js detect: env_game_root -> ok=true data=D:\STool\verify\tail_test err=
app.js detect: KiriKiri / KAG score=140 level=high confirmed=true caps=4 root=D:\STool\verify\tail_test
app.js 页面已挂载: detect / 标题="选游戏"
```

**`tools` 页日志（原文）**

```
app.js 启动完成；__TAURI__=ok
app.js tools: 设置已载入，外部工具 7 项
app.js 页面已挂载: tools / 标题="工具箱"
```

**`save` 页日志（本轮补，验证新加的备份/还原命令层没把启动链路搞坏）** ——
`STOOL_PAGE=save STOOL_SAVE=D:\STool\verify\demo_save\mv_save.json STOOL_QUERY=gold`：

```
app.js 启动完成；__TAURI__=ok
app.js save: mount 进入
app.js save: renderPicker 完成
app.js trace: env_save_path 返回 ok=true data=D:\STool\verify\demo_save\mv_save.json
app.js save: format=JSON 明文存档 writable=true top=11 path=D:\STool\verify\demo_save\mv_save.json
app.js save search "gold" scope=all -> 1 行
app.js 页面已挂载: save / 标题="改存档"
```

**这 8 行证明了整条链路**：Tauri 运行时起来 → WebView2 起来 → 前端脚本跑完 → IPC 通 → 命令层读环境变量 → **内核在真样本上认出引擎（KiriKiri 140 分，与 CLI 一致）** → 工具箱页取到设置（7 项工具）。`save` 的 6 行同理，且**走的是真实存档文件**（`top=11` 是内核解析出来的，不是前端编的）。

> 注：exe **不会自己关**（本轮实测要 `timeout` 收）。上一轮的「约 3 秒自行关闭」是外层 PowerShell 调 `CloseMainWindow` 的结果，不是 exe 的行为 —— 已改正。

**启动耗时**（从 `Start-Process` 到日志出现「页面已挂载」）：

| 页 | 耗时 | 说明 |
|---|---|---|
| detect | **822 ms** | 该 exe 首次运行，含 WebView2 首次初始化 |
| tools | **549 ms** | 已有 OS 文件缓存 |

两次都**优雅退出**（`CloseMainWindow` 生效，没有走强制结束）—— 避开方案 §9.4 提到的「反复 taskkill 破坏 WebView2 窗口类」那个坑。

---

## 7. CLI 冒烟（§2.3 第 4 条）

`stool-cli` 在同一个真样本上：

```
$ stool-cli detect verify/tail_test
★ KiriKiri / KAG [kirikiri] 140 分 · 高置信 — 1 个 .xp3 封包; 18 个 .tjs + 0 个 .ks 明文脚本（未打包）; Config.tjs / Startup.tjs 引擎配置

$ stool-cli doctor verify/tail_test
⚠ 主程序: 根目录没有 .exe
    修法: 可能选到了数据目录；请选择含游戏主程序的文件夹
✔ 系统区域（非 Unicode 程序）: 简体中文（ACP=936）
✔ 日文字体: 已安装 MS Gothic / Yu Gothic
✔ 写权限: 游戏目录可写
✔ 体检完成：1 项提示，未发现阻断性问题。

$ stool-cli selfcheck verify/tail_test/unencrypted.xp3 -o <临时目录>
✔ 封包: unencrypted.xp3 · XP3（KiriKiri） · 189 条目 · 184.3 MB
✔ 解包: 189 个条目全部成功解出
✔ 重打包: 已重建（13.6 MB）
✔ 往返比对: 189 个条目逐一一致（大小 + FNV-1a 64 哈希）
✔ 耗时: 7274 ms
✔ 无损：解包→重打包→回读逐条目一致。
```

`detect` 给出 **140 分**，与 Tauri GUI 日志里的 `score=140` **完全一致** —— CLI 与界面共用同一份内核的直接证据。`doctor` 的 ⚠ 项自带「修法」，与工具箱页渲染的是同一份内核报告。

---

## 8. 本轮改了什么

| # | 文件 | 改动 | 为什么 |
|---|---|---|---|
| 1 | `stool-tauri/src-tauri/src/cmd.rs` | `runtime_patch_remove` 的不支持分支补「改法」 | 66 个 `Err` 站点里唯一没带的，补完 **66/66** |
| 2 | `stool-tauri/src/js/pages/tools.js` | 诊断卡按 `Store.gameRoot` 分支：没选游戏时换成带「去选游戏」的空状态 | 修「摆一个点了必然报错的按钮」；设置/帮助两页不受影响 |
| 3 | `stool-tauri/src/js/pages/mods.js` | 「还没装过 MOD」空状态补按钮（滚动 + 聚焦到 `#mPick`） | 全站唯一一处无出口的空状态 |
| 4 | `stool-tauri/src-tauri/build.rs` | 加 `rerun-if-changed=../src` | **修一个会静默发旧界面的坑**，见 §9 |
| 5 | `stool-tauri/preview.html` | 加 `?nogame=1`、`?mods=empty` 两个预览参数 | 让上面 2、3 两处能被截图验收 |
| 6 | `.gitignore` | 去掉重复的 `shots/.edgeprof*/` 块，补 `shots/.ep_*/` | 清理 + 覆盖新截图 profile 目录 |
| 7 | 清理残留 | 删 3 张 MD5 全同的 `_t*.png`、1 个 0 字节 `_dbg.html` | 收尾卫生 |
| 8 | `stool-tauri/src-tauri/src/cmd.rs` | 新增 `restore_list` / `restore_one` 两个命令 + 2 个纯函数 + 4 个单测 | 补上「写回存档 / 重新打包」缺的一键撤销（§5.4） |
| 9 | `stool-tauri/src-tauri/src/main.rs` | 注册 `restore_list` / `restore_one` | 不注册 = 前端调用直接报「命令不存在」 |
| 10 | `stool-tauri/src/js/ui.js` | 新增 `dirOf` / `mountBackups` / `undoBarHtml` / `bindUndo` | 两页共用一份备份列表渲染，不重复写 |
| 11 | `stool-tauri/src/js/pages/save.js` | 写回成功后原地给「还原到改之前」；新增「备份与还原」折叠 | §5.4 |
| 12 | `stool-tauri/src/js/pages/extract.js` | 打包结果卡换成 `repackResultHtml`（带还原入口）；进阶折叠加常驻备份入口；首屏「封包」→「打包文件」 | §5.4 + §5.1 |
| 13 | `stool-tauri/src/js/pages/text.js` | 去术语：卡片标题/状态/按钮/折叠标题/空状态 | §5.1 |
| 14 | `stool-tauri/src/js/pages/detect.js` | 去术语（「运行时汉化」→「不改游戏文件汉化」）；认不出引擎时给出 3 张**真能点**的卡（原来指向一个不存在的入口） | §5.1 |
| 15 | `stool-tauri/preview.html` | 加 `?save=*` / `?repack=*` / `?engine=unknown` 预览参数 + `restore_list` / `restore_one` 假后端；`text_inject` / `text_uninject` 文案对齐内核真实返回 | 让上面几处能被截图验收；假后端要像真后端 |

新增截图（均已逐张核对内容）：`shots/tauri/33-tools-nogame.png`、`34-mods-empty.png`、`35-tools-nogame-settings.png`、`36-save-undo.png`、`37-save-backups.png`、`38-save-backup-empty.png`、`39-extract-repack-done.png`、`40-extract-backups.png`、`41-text-channel.png`、`42-save-undo-dark.png`、`43-detect-unknown.png`。

---

## 9. 本轮发现的最有价值的一个坑：改了前端，exe 里还是旧的

**现象**：改完 `src/js/*.js` 直接 `cargo build --release`，构建**成功**，exe 时间戳也是新的 —— 但界面没变。

**根因**：`tauri-build` 只声明了对 `tauri.conf.json` 和 `capabilities/` 的 `rerun-if-changed`，**完全不管 `frontendDist`**。构建脚本不重跑 → 内嵌资源还是上一版。实测证据：

```
源文件            src/js/pages/tools.js        11:10:17
内嵌资源生成      .../out/tauri-codegen-assets/  11:09:04   ← 早了 73 秒，嵌的是旧的
```

**为什么很难发现**：内嵌资源是 **brotli 压缩**的，`grep -a <中文串> exe` **一个都查不到**。看着「编译成功」根本不会怀疑。

**修法**：`build.rs` 里补 `println!("cargo:rerun-if-changed=../src");`。

**判定新旧的办法**（别靠 grep）：比对 `<profile>/build/stool-tauri-*/out/tauri-codegen-assets/` 与 `src/js/` 的 mtime；或按下面这招硬证：

```
① 解开内嵌资源，查只有新版才有的中文串 → 命中
② 用「压缩后前 48 字节」当指纹在 exe 里搜：
     旧 mods.js（11:09）→ False   新 mods.js（11:17）→ True
```

已记入方案 §9.5 坑⑯。

---

## 10. 复现命令

```bash
# 命令层门禁
cd D:/STool/stool-tauri/src-tauri
cargo clippy --all-targets -- -D warnings
cargo test --tests

# 前端语法（逐个文件）
cd D:/STool/stool-tauri
for f in $(find src/js -name '*.js'); do node --check "$f"; done

# 单 exe
cd D:/STool/stool-tauri/src-tauri && cargo build --release
# 产物：target/release/stool-tauri.exe（约 13.4 MB）

# 内嵌资源是否为最新（必须晚于 src/js/ 的 mtime）
stat -c '%y %n' D:/STool/stool-tauri/src/js/pages/tools.js \
                D:/STool/stool-tauri/src-tauri/target/release/build/stool-tauri-*/out/tauri-codegen-assets/
```

> **为什么只能比 mtime**：`tauri-codegen-assets/` 里的文件名是内容哈希、内容是**压缩后**的，
> 所以既对不上 `sha256sum <源文件>`、也 grep 不到任何中文串（实测：新旧文案各查一遍，命中数都是 0）。
> 唯一可用的判据就是**目录 mtime 晚于所有前端源文件**。本轮实测：
> 最新前端 `detect.js` 12:08:51 → 资产目录 12:09:14 → exe 12:11:56，链条成立。

```bash
# 真机端到端（会开一个窗口；不自己关，用 timeout 收）
STOOL_PAGE=tools STOOL_GAME='D:\STool\verify\tail_test' \
STOOL_TUI_LOG='D:\STool\shots\tauri-run-tools.log' \
  timeout 30 D:/STool/stool-tauri/src-tauri/target/release/stool-tauri.exe

# 存档页：打开真实演示存档 + 自动搜一次（本轮新命令层就是这样验的）
STOOL_PAGE=save STOOL_SAVE='D:\STool\verify\demo_save\mv_save.json' STOOL_QUERY='gold' \
STOOL_TUI_LOG='D:\STool\shots\tauri-run-save2.log' \
  timeout 30 D:/STool/stool-tauri/src-tauri/target/release/stool-tauri.exe
# 期望日志尾部：format=JSON 明文存档 writable=true top=11 / search "gold" -> 1 行

# CLI 不受影响
cd D:/STool/stool-rs && cargo build --release
cargo build --release --no-default-features --bin stool-cli
./target/release/stool-cli.exe detect   D:/STool/verify/tail_test
./target/release/stool-cli.exe doctor   D:/STool/verify/tail_test
./target/release/stool-cli.exe selfcheck D:/STool/verify/tail_test/unencrypted.xp3 -o <临时目录>

# 界面截图
msedge --headless=new --disable-gpu --hide-scrollbars --window-size=1180,900 \
  --virtual-time-budget=12000 --user-data-dir=D:/STool/shots/.ep_x \
  --screenshot=D:/STool/shots/tauri/NAME.png \
  "file:///D:/STool/stool-tauri/preview.html?nogame=1#tools"

# 一键撤销那几张（长页面要更高的窗口，否则还原条在折叠线以下、截不到）
msedge --headless=new --disable-gpu --hide-scrollbars --window-size=1180,1900 \
  --virtual-time-budget=15000 --user-data-dir=D:/STool/shots/.ep_x \
  --screenshot=D:/STool/shots/tauri/36-save-undo.png \
  "file:///D:/STool/stool-tauri/preview.html?save=undo#save"
```

> 预览参数全表：`?theme=dark`、`?nogame=1`、`?kind=`、`?pick=`、`?mtl=1`、`?fold=1`、
> `?autorun=1`、`?apply=1`、`?bak=1`、`?run=1`、`?scan=1`、`?diag=1`、`?mv=1`、`?procs=1`、
> `?mods=`、`?tools=`、`?check=`、**`?save=undo|bak|bakempty`**、**`?repack=done|bak`**、
> **`?engine=unknown`**。

## 11. 命令层测试清单（51 个）

`cargo test --tests` 全绿。其中与本次改动直接相关的 4 个：

| 测试 | 钉住的行为 |
|---|---|
| `restore_list_finds_backups_with_size_and_age` | 只认 `.stool.bak`，且回填了人类可读的大小/时间 |
| `restore_list_rejects_missing_dir_with_a_fix` | 目录不存在时报错**必须带「改法」** |
| `restore_one_rejects_paths_that_are_not_backups` | 非 `.stool.bak` 路径拒绝（含前后空格容错） |
| `restore_one_puts_the_old_content_back_and_keeps_the_backup` | 还原后**备份仍在**，可反复还原 |
