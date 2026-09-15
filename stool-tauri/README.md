# stool-tauri —— STool 的图形界面（唯一界面）

Rust 内核（`../stool-rs`）+ Tauri v2 / WebView2 界面。重点：**好看**与**好用**。

方案与验收标准见 `../docs/TAURI重构方案.md`，验收结论见 `../docs/TAURI验收报告.md`。

## 为什么要单独一个 crate

内核（`features/` `formats/` `engines/` …）对 UI 框架**零依赖**，所以界面可以整体替换。

**旧的 eframe/egui 版（`stool-rs/src/gui/`、`stool.exe`）已删除** —— 本 crate 就是本仓库
唯一的图形界面，`stool-rs` 只留内核 + `stool-cli.exe`。因此这里直接依赖
`stool = { path = "../../stool-rs" }`，不需要任何 feature 门控。

## 目录

```
src/                 前端（静态 HTML/CSS/JS，无打包器）
  css/tokens.css       设计令牌：全站唯一取色 / 取尺寸来源
  css/app.css          布局与组件（三段式：说明条 / 主行动区 / 结果区）
  js/bridge.js         与 Rust 的唯一通道（call 调命令 / on 订阅事件）
  js/ui.js             通用小工具（转义 / 空状态 / 折叠 / toast / 路径缩写）
  js/pages/*.js        页面
  js/app.js            外壳：任务式导航 + 路由 + 主题
src-tauri/           Rust 侧
  src/main.rs          入口 + 全局状态
  src/cmd.rs           命令层（每页 1~2 个命令，只返回算好的展示数据）
preview.html         离线界面预览（内存假后端，不需要 WebView2）
```

## 构建

```bash
cd src-tauri
cargo run              # 开发（首次构建较慢：要编 tauri 全家桶）
cargo build --release
```

> **坑（必须知道）**：`tauri/build.rs` 里是 `let dev = !custom_protocol;`。
> 不带 `custom-protocol` feature 时，**连 release 构建也会被判成 dev 模式**，
> Tauri 会去找前端 dev 服务器，结果是一个**静默的白窗口**（没有任何报错）。
> 本 crate 的 `Cargo.toml` 已把 `features = ["custom-protocol"]` 写死，所以裸 `cargo build` 也能跑。

> **坑（杀软误报，一定会遇到）**：卡巴斯基等启发式引擎会把这个 exe 报成
> `VHO:Trojan-PSW.Win32.Greedy.gen`（`VHO:` = 启发式判定，不是特征码命中；`.gen` = 泛型）。
> 这不是感染，是本工具**自己功能**造成的：
>
> | 内核模块 | 用到的 API | 在启发式引擎眼里像什么 |
> |---|---|---|
> | `features/gallery.rs`（Unity 全CG解锁） | `RegOpenKeyExW` / `RegQueryValueExW` / **`RegSetValueExW`** | 翻注册表找凭据 |
> | `settings.rs`（机翻 Key 加密） | **`CryptProtectData` / `CryptUnprotectData`** | DPAPI 解密密钥 |
> | `memapi.rs` + `features/memscan.rs` / `guard.rs`（内存扫描 / 反修改诊断） | **`CreateToolhelp32Snapshot` / `Process32NextW` / `OpenProcess` / `ReadProcessMemory` / `VirtualProtectEx`** | 跨进程读内存 |
>
> 「读注册表 + **写**注册表 + DPAPI 加解密 + 枚举进程 + 跨进程读内存」正好是窃密器的典型调用面，
> 再加上**本地编译、未签名、零信誉**，被 `Trojan-PSW`（密码窃取类）泛型规则命中是必然的。
> 实测：`grep -ao` 在 `target/release/stool-tauri.exe` 里可逐个查到上面这些 API 名；
> debug 测试二进制（`target/debug/deps/stool_tauri-*.exe`）不含内存那几个，但含注册表与 DPAPI 那几个。
>
> **副作用（比报毒更烦）**：卡巴斯基会**阻止执行**刚编出来的二进制，表现为
> `cargo test` 报 `拒绝访问 (os error 5)` / `LNK1104: 无法打开文件 …`，
> 而直接跑那个 exe 有时又能过 —— 别去改代码，是杀软在挡。
>
> **处置**（免费版也支持，见 KFA 21.3 官方文档「威胁和排除项 → 管理排除项」）：
> 设置 → 安全 → 排除项和检测到对象时的操作 → **管理排除项** → 添加
> ① 文件或文件夹填 `D:\STool\**`（或至少 `stool-rs\target` 与 `stool-tauri\src-tauri\target`）；
> ② 对象类型填 `Trojan-PSW.Win32.Greedy.gen`（其余条件留 `*`）。
> 加之前先**暂停保护**，否则还原后会被立刻再杀一次。
> 想彻底不再报，可把该样本提交卡巴斯基病毒实验室（Virus Lab）申请撤销误报 —— 这是唯一能帮到别人的做法。

## 长任务与进度

解包 / 封包不是「一进一出」，所以走 Tauri **事件流**：内核的进度回调
（`engines::Ctx::progress`）在 Rust 侧被转成 `app.emit("op:progress", {frac, msg})`，
前端 `Tauri.on("op:progress", …)` 收到后**只更新几个节点**（不整页重绘，否则会闪、会丢焦点）。

取消沿用内核口径：`cancel_task` 置 `Ctx::cancel`，内核在**条目边界**退出，
已写出的文件保留（配合内核的断点续传，下次接着跑）。

## 三层验证（都不依赖截图）

本机抓 WebView2 的像素不可靠（DirectComposition，`PrintWindow`/`CopyFromScreen` 都可能抓到白/黑），
而且反复强杀会让 WebView2 的窗口类状态坏掉，所以验证分三层：

### 1. 内核门禁

```bash
cd ../stool-rs
cargo clippy --all-targets -- -D warnings
cargo test --tests
```

### 2. 命令层单测（对真机样本）

```bash
cd src-tauri
cargo test
```

命令层里真正干活的是 `detect_impl` / `load_doc` / `run_op_core` 这类**纯函数**
（不碰任何 Tauri 类型），所以可以直接拿 `../verify/` 下的真机样本测，不必拉起 WebView。
样本不在就跳过 —— **不造假绿**。

> 解包的**正向**路径只能用 `verify/tail_test`（未加密 xp3）。
> `verify/komoguri` 三个 xp3 全加密，把它当成功用例会得到「乱码 == 乱码」的假无损。

### 3. 界面：离线看 + 环境变量钩子

```bash
# 直接在浏览器里看界面（同一套前端 + 内存假后端）
#   可选：#extract 直接停在某页；?theme=dark 看深色；?autorun=1 自动跑一次看进度条
#   看素材专有：?kind=image|audio|text 预选类型；?pick=N 预选第几个文件
#   翻译文字专有：?fold=1 展开表格预览；?mtl=1 展开机翻设置
#   解锁全CG专有：?apply=1 勾上「确认写入」；?bak=1 展开备份列表；?run=1 直接跑一次
#   游戏里改数值专有：?mv=1 方式一显示「已连接」编辑器；?scan=1 演一遍搜数值（命中列表）；
#                     ?diag=1 演一遍回滚诊断（含方案清单）；?procs=1 展开进程列表
#   装MOD专有：?mods=1 演「选好补丁目录 + 预演无冲突」；?mods=hits 演「会和已装 MOD 抢文件」；
#              ?mods=clean 先清掉预置冲突再演无冲突预演；?mods=empty 演「一个 MOD 都没装」的空状态；
#              ?fold=1 展开一个 MOD 的文件清单
#   工具箱专有：?tools=settings|help 切到设置/帮助分区（默认诊断）；
#              ?check=bad 演一遍「体检有问题」；?check=selfcheck 演封包自检；?diag=1 演「已导出诊断包」
#   改存档专有：?save=undo 演「刚写回一次」（就地还原条）；?save=bak 展开备份列表；
#              ?save=bakempty 同上但目录里没有备份（验空状态给出路）
#   取出素材专有：?repack=done 跑一遍「重新打包」（看结果卡里的还原入口）；
#                ?repack=bak 只展开进阶里的备份列表
#   选游戏专有：?engine=unknown 演「认不出引擎」——验这一页给出的出路是否真能点
#   全站通用：?nogame=1 演「还没选游戏」——用来验各页空状态是否给得出路（§2.2「没有死胡同」）
start preview.html

# 真机跑：这几个环境变量只在启动时读一次，走的路径与手动点击**完全相同**
STOOL_TUI_LOG=D:/tmp/stool-tauri.log \
STOOL_PAGE=extract \
STOOL_GAME="D:/games/某个游戏" \
STOOL_OUT="D:/tmp/_extracted" \
STOOL_SAVE="D:/games/某个游戏/save/file1.rpgsave" \
STOOL_QUERY=gold \
./target/debug/stool-tauri.exe
```

截图可以离线做（Chromium 渲染 `preview.html`），产物在 `../shots/tauri/`。
**下面五个坑都踩过（全表见方案 §9.5），最容易中的是「给相对路径会静默不写文件」**：

```bash
msedge.exe --headless=new --disable-gpu --hide-scrollbars --window-size=1180,900 \
  --virtual-time-budget=12000 \
  --user-data-dir=D:/STool/shots/.edgeprof_dark \
  --screenshot=D:/STool/shots/tauri/02-extract.png \
  "file:///D:/STool/stool-tauri/preview.html#extract"
```

> ① `--screenshot` 必须**绝对路径** —— 给相对路径会报「拒绝访问 (0x5)」，而 Edge 退出码仍是 0，不看文件根本发现不了；
> ② `--user-data-dir` 指向**项目内固定目录**，别用 `mktemp -d` 造的临时目录（headless 会起不来、静默不写文件）；
> ③ 深浅两版**各用一份干净 profile** —— 主题存在 `localStorage` 里，共用 profile 会让「深色版」截得和亮色**一模一样（MD5 都相同）**；
> ④ URL **必须带 `#page`**，否则截出来是默认页、和上一张完全相同；
> ⑤ `--virtual-time-budget` 给到 **12000** —— 太小的话异步 hook 还没跑完就截，只会截到空态。

两个必看的坑：`--screenshot` 的目标**必须绝对路径**（相对路径会 `拒绝访问 (0x5)`，
但 Edge 退出码仍为 0）；`--user-data-dir` 用**项目内固定目录**而不是临时目录
（否则 headless 静默不写文件）。深浅两版各用一份干净 profile ——
主题存在 `localStorage`，共用会让「深色版」截出来和亮色一模一样。

## 进度

| 页面 | 状态 |
|---|---|
| ① 选游戏 | ✅ 真实引擎检测（走内核 `Registry::detect_all`） |
| ② 取出素材 | ✅ 进度事件流 + 取消；「重新打包」收进进阶并要求勾选确认 |
| ③ 看素材 | ✅ 虚拟化列表 + 图片/音频/文本预览；文本按**原编码**解（含无 BOM UTF-16、Shift-JIS） |
| ④ 翻译文字 | ✅ 双通道（CSV 回填 / 不改游戏文件）+ 机翻（DeepSeek / 智谱 / 本地 Ollama）+ 术语表；引擎不支持时如实说明并指向通用通道 |
| ⑤ 改存档 | ✅ 搜索虚拟化 + 常用字段速改 + 写回前自动备份 + **写回后一键「还原到改之前」** |
| ⑥ 游戏里改数值 | ✅ 四张卡：按名字改（RPG Maker MV/MZ）+ 搜数值改（Cheat Engine 式）+ 进阶（回滚诊断/页保护）+ 补丁包（KiriKiri）；术语对齐 CE（4 字节 / 8 字节）；冻结由前端驱动，不留孤儿线程 |
| ⑦ 解锁全CG | ✅ 只读计划（途径/自带存档/目标目录）+ 显式确认后才写入 + 一键还原备份 |
| ⑧ 装MOD | ✅ 安装（覆盖前自动留底）+ 冲突**装前预演** + 停用（留记录）/卸载（删记录）分开 + 折叠看改了哪些文件 |
| ⑨ 工具箱 | ✅ 设置 / 诊断 / 帮助 三块合一页；体检 + 封包自检 + 日志尾部 + 一键导出诊断包；外部工具一键下载；密钥永不回传界面 |
| 设计令牌 / 导航外壳 / 三段式骨架 | ✅ |

九个任务页全部接好，**收尾三条也已完成**（端到端真机跑 / 打成单 exe / 按 §2 三张表逐条核）。
逐条结论、实测数字与偏离项见 **[`docs/TAURI验收报告.md`](../docs/TAURI验收报告.md)**。

要点：单个 `stool-tauri.exe` = **13.4 MB**，启动到可操作 **822 ms**（达标线 1500 ms）；
`clippy` 0 警告 / **51 测试全过**；真机 `detect` 在样本上认出 KiriKiri 140 分，与 CLI **完全一致**。
§2 三张表 **17 条全达标**（含后补的两条：一级界面技术术语、两处写入的一键撤销）。

**尚未做**：`500 条搜索 <100ms` 与「诊断包 zip 逐字节比对」需真机交互才量得出（不编数字）。

## 翻译文字为什么有两张卡

汉化有两条路，**能不能走后一条取决于引擎**，所以不能只给一行按钮：

| 通道 | 覆盖面 | 代价 |
|---|---|---|
| ① 提取 → 翻 CSV → 回填 | 任何引擎 | 会写游戏目录里的文件（写前自动备份 `.stool.bak`） |
| ② 生成 JSON → 不改游戏文件 | 仅 Ren'Py / RPG Maker MV·MZ / TyranoBuilder / HTML | **不动游戏任何文件**，启动时在内存里替换 |

几个刻意的决定：

- **不支持注入不是故障**。`inject::support_of` 对表外引擎返回 `None`，如果就这样把空串交给界面，
  用户看到「通道 ② · 不改游戏文件（这个引擎不行）」后面一片空白，会以为是工具坏了。命令层补一句人话 + 指向通道 ①。
- **内核文案里的「注入」原样转述**，界面只在**按钮与标题**上换成人话（「让译文生效 / 撤回」）。
  同一个操作，CLI 与 GUI 必须说同一句话，所以不在 UI 层重写内核返回值。
- **机翻是「预填」不是「一键汉化」**。它只往 CSV 的译文列里先写一版，用户仍要过目 ——
  界面上直说，不给「点一下就是中文版了」的错觉。
- **批量/并发不另定阈值**，直接读 `settings::Config`（`mtl_batch` / `mtl_jobs`）。
  命令层再抄一份数字会出现「设置改了没生效」。
- **密钥存设置文件**（DPAPI 加密），不写游戏目录 —— 否则汉化包一起发出去就泄露了。

## 解锁全CG为什么先给「只读计划」

解锁是**唯一一类默认就会改存档**的操作，所以页面顺序刻意反过来：先看清楚，再动手。

- **进页面只出计划，不写盘**。`unlock_plan` 是只读的（单测 `unlock_plan_is_read_only` 盯着它：
  跑完原存档必须一字未变、且不产生 `.stool.bak`）。用户「只是看了看」就被改了存档 ——
  这类 bug 不报错，只造成损失。
- **界面不自己判路线**，一律转述内核 `features::unlock` 的同一份策略表
  （`spec_or_generic` / `find_bundled_saves` / `find_save_dirs` / `pick_route`）。
  界面自己判会出现「界面说会走 A、内核实际走 B」，用户按界面点了却报错。
- **手段是数据不是分支**：`UnlockRoute` 直接序列化成 `routes[]`，页面上就是一张表
  （`automated` 决定「可以 / 只能给指引」）。新增引擎只改内核策略表，这一页不用动。
- **「只能给指引」不是失败**。游戏内开关、人工分析这两条本来就无法自动化，
  照常列出并说清怎么做，比藏起来更诚实。
- **写入前明说备份位置、写入后能撤销**：覆盖走内核 `settings::backup_once`（留 `.stool.bak`），
  页面下方 `unlock_backups` 列出备份、`unlock_restore` 一键还原 ——
  `unlock_restore` 只接受 `.stool.bak` 结尾，否则这个命令就是任意文件覆盖入口。


## 文本预览为什么要有「双闸门」

`.ks` / `.tjs` / `.csv` 这些扩展名**不等于**内容是文本。真机踩到两次：

1. **无 BOM 的 UTF-16**：KiriKiri 的 `.tjs` 大量是这种（`verify/tail_test/out/unencrypted`）。
   纯 ASCII 的 UTF-16LE 是 `x, 0x00` 交替，而 NUL 是**合法 UTF-8 字符** ——
   按 UTF-8 能"成功"解出 `/\0/\0 \0C\0…`，不报错但没法读。
   → 所以嗅探必须排在严格 UTF-8 **之前**。
2. **Shift-JIS 的假阳性**：随机字节有相当比例会落进 Shift-JIS 的单字节半角片假名区，
   于是**密文**（加密封包解出来的残留）能被"成功"解码成 `ﾏ0沢Sｴ`，零替换字符。
   → 所以除了替换字符闸，还要加一道半角片假名闸。

真机实测的分界：正常日文文本半角片假名约 1~4%，密文 12~33%。
中间仍有重叠带（个别密文落在 8% 边缘），所以判为「不是文本」时**不当作故障** ——
界面给的是「用系统程序打开」这条出路，而不是一个报错框。
