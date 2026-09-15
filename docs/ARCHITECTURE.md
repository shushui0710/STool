# STool 架构说明

> 面向"新会话 / 新人快速上手"：模块职责、数据流、以及**改哪里**。
> 配套文档：`docs/引擎识别依据与解锁策略.md`（检测判据与解锁策略的单一事实来源）。

---

## 1. 形态

| 二进制 | 入口 | 说明 |
|---|---|---|
| `stool`（GUI） | `src/main.rs` → `src/gui/mod.rs` | eframe/egui 图形界面（release 不弹控制台） |
| `stool-cli`（CLI） | `src/cli_main.rs` → `src/cli.rs` | 命令行；无参数时 `cli_main` 也可拉起 GUI |

`src/lib.rs` 导出 `VERSION` 与全部模块，两个二进制共用。

### 1.1 第二个界面：`stool-tauri/`（Tauri v2 / WebView2）

内核（`features/` `formats/` `engines/` `memapi` `settings` `cli`）**对 UI 框架零依赖**，
所以界面可以整体替换。仓库根目录新增 `stool-tauri/`：

| 位置 | 说明 |
|---|---|
| `stool-tauri/src/` | 前端：静态 HTML/CSS/JS，**无打包器** |
| `stool-tauri/src-tauri/` | Rust 侧：命令层（`cmd.rs`）+ 全局状态（`main.rs`） |

- 依赖方式：`stool = { path = "../../stool-rs", default-features = false }`。
- `stool-rs` 的 `pub mod gui` 由 **`gui` feature**（默认开）门控；关掉后不再编译
  eframe/rfd，Tauri 壳因此不必重复编译整套 egui。两版**并存**：
  `stool.exe` 照旧可用，`stool-tauri.exe` 是新界面。
- 命令层**不重写业务逻辑**：只调 `engines::Registry` / `features::*`，且返回**算好的展示数据**
  （不把整棵 JSON 树丢过 IPC）。
- 方案与验收标准见 `docs/TAURI重构方案.md`。

新增 / 改页面时的落点：

| 想改什么 | 改哪 |
|---|---|
| 页面结构与交互 | `stool-tauri/src/js/pages/*.js` + `css/app.css` |
| 配色 / 间距 / 字号 | **只改** `stool-tauri/src/css/tokens.css`（UI 上不得出现表外颜色） |
| 导航里加一页 | `stool-tauri/src/js/app.js` 的 `PAGE_META` / `NAV_GROUPS` |
| 加一个后端能力 | `stool-tauri/src-tauri/src/cmd.rs` 加 `#[tauri::command]`，并在 `main.rs` 的 `generate_handler!` 里登记 || 值 ↔ 文本的口径 | `stool-rs/src/features/saves.rs` 的 `parse_edit_text` / `value_edit_text` / `type_hint` —— **两个界面共用，只有这一处实现** |

---

## 2. 目录结构（模块职责）

```
src/
├── engines/           引擎层：检测 + 各引擎的解包/回填/注入/存档/解锁
│   ├── mod.rs           Engine trait、Op 能力枚举、Registry（注册表 + 排序）、
│   │                    DETECT_LINE / Confidence / Detection、safe_out_path（带目录缓存）/ write_out、
│   │                    Resume（解包断点续传台账，P2-4）+ write_out_resume、
│   │                    Job<T> + parallel_extract + worker_count（解包并行化，P2-2）
│   ├── scan.rs          ScanCtx：**一次** read_dir + **一次** walkdir，收齐检测所需索引
│   ├── recognize.rs     盲区引擎的**表驱动**识别器（Sig 判据 + Rec 规则 + 处置建议）
│   ├── generic.rs       未知引擎兜底：GARbro 委派 + KNOWN_EXT 扩展名提示
│   ├── renpy.rs         Ren'Py（解包/回填/反编译/注入/persistent 解锁）
│   ├── others.rs        RPG Maker MV·MZ / RGSS / KiriKiri / Godot / NScripter / Tyrano / HTML
│   ├── artemis.rs       Artemis Engine 原生支持（.pfs 解包/回封，识别 70 分）
│   └── external.rs      Wolf（WolfDec 委派）、Unity（AssetRipper 委派 + 程序集精确键名 / 注册表解锁）
├── formats/           纯格式层（无 IO 之外的副作用），每个都有 roundtrip 测试
│   ├── source.rs       [新增] 流式数据源（P2-1）：Mem(≤128MB)/File 双模式，read_at 按需读，MAX_ARCHIVE=4GB
│   ├── safe.rs         边界安全读取原语（P0-2）：越界一律返回 None，解析器不再 panic
│   │                   （`Cursor` 提供 u8/u16/u32/i32/u64 的顺序读，读完自动前进）
│   ├── xp3.rs / pck.rs / asar.rs / rgss.rs / rpa.rs / pfs.rs …
│   │                   各格式均提供 `parse_index(&mut Source)` + `read_entry(&mut Source, …)`
│   │                   （按偏移按需读，替代整包 `read_to_end`；RGSS 为 parse_index_v1/v3 + read_entry_v1/v3）
│   │                   xp3.rs 另含写侧：`Xp3Version{V1,V2}` / `version_of` / `write_paths_v` /
│   │                   `encrypted_count`（见 §6 约定）
│   │                   pfs.rs Artemis 封包：pf6 明文 / pf8 用 **SHA-1(index) 前 20 字节做 XOR**，
│   │                   且**每条目从 key[0] 重新开始**（与 GARbro 的 `ByteStringEncryptedStream`
│   │                   按全局偏移递进不同）；写侧 `write_archive` / `write_paths` 沿用原代次
│   ├── dotnet.rs       [新增] 最小 .NET / PE 元数据读取（零依赖、只读、不执行）：
│   │                   DOS→PE→可选头→节表(RVA2off)→CLI 头→元数据根(BSJB)→流头，
│   │                   取 `#US`（UTF-16LE 用户字面量，长度前缀为 ECMA-335 压缩整数）
│   │                   与 `#Strings`（NUL 分隔名字堆）
│   └── il2cpp.rs       [新增] Unity IL2CPP 的 `global-metadata.dat` 字符串读取（零依赖、只读）：
│                       `stringLiteral`（`{i32 len; i32 dataIndex}`，len 为**精确字节长度**）+
│                       `stringLiteralData` 池 + `string` 名字堆；**只接受通过校验的输入**
│                       （magic / 版本 / 三区首尾相接 / 字面量 UTF-8 可解率 ≥90%），否则退回启发式
├── features/          横向功能
│   ├── unlock.rs        [新增] 统一解锁抽象：UnlockRoute / UnlockSpec / UNLOCK_CATALOG /
│   │                    find_save_dirs / find_bundled_saves / pick_route / run
│   ├── gallery.rs      Unity 全CG解锁（PlayerPrefs 注册表路线）：djb2 哈希、Win32 读写、
│   │                    快照/还原；**键名两级来源** —— `scan_precise`（Mono 程序集 `#US` /
│   │                    IL2CPP 元数据字面量，精确）+ `scan_scene_strings`（场景二进制 ASCII
│   │                    启发式兜底）；`looks_like_key` 白名单字符集过滤框架串、
│   │                    `sort_candidates` 按相关性分层后再截断
│   ├── inject.rs       运行时 JSON 注入汉化（SUPPORT_TABLE 声明范围与边界）
│   ├── translate.rs    机翻（OpenAI 兼容；术语表、断点续翻、`jobs` 批并发 + 429/5xx 指数退避）
│   ├── tpack.rs        翻译包导出/导入
│   ├── saves.rs        存档文档模型（多格式解析 + 指针寻址编辑）
│   ├── mods.rs         MOD 安装/启停/卸载
│   ├── runtime.rs      运行时修改（内存/脚本层）
│   ├── xp3patch.rs     [新增] KiriKiri 运行时补丁包（P2-12）：`list` / `next_name` /
│   │                   `build`（打 patchN.xp3，原封包不动）/ `remove`（只删自己建的）/
│   │                   `build_changed`（与现有封包逐字节比对，**只打包改动文件** →
│   │                   支撑「解包 → 汉化 → 打补丁」闭环）
│   ├── memscan.rs      进程内存扫描（RAII 句柄，MAX_REGION 限制）；
│   │                   **首次写入前必须二次确认**（GUI 弹窗 / `ms_ack` 勾选）；
│   │                   底层内存 API 已抽到 `src/memapi.rs`（与 guard 共用）
│   ├── guard.rs        [新增] 反修改保护识别与对抗（P3-1）：「CE 改完立刻被还原 / 改了
│   │                   没反应」的诊断引擎。写入-存活探测 → `Verdict`（稳定/立即/周期/延迟
│   │                   回滚）；两次写入差分 → `Rewrite`（XOR/偏移/常量/不透明，判定派生副本）；
│   │                   同值镜像 + 数据源反测；页保护汇总（`RegionStat`/`PageSummary`）；
│   │                   保护线索（已加载模块 `MODULE_TABLE` + PE 导入表反调试 API）；可执行区
│   │                   `mov [disp32], imm32` 立即数写入点扫描（还原指令落点）；最后按
│   │                   `Feasibility` 生成 `Plan`（强制写入 / 自适应锁值 / 改数据源 / 代码补丁 /
│   │                   替代路线 / 在线反作弊仅提示）。**在线反作弊只给风险提示，不提供绕过**
│   ├── preview.rs      资源预览：类型分类（`kind_of`/`mime_of`）、列表（`MAX_MEDIA` 上限防超大
│   │                   目录卡顿）、**文本编码判定**（`decode_text` → `DecodedText`，见 §6.2.1）
│   ├── tools_dl.rs     外部工具下载：**流式**写盘 + 对照 GitHub `digest` 做 SHA-256 校验，
│   │                   无摘要时把校验和记到 `<工具目录>/.sha256`
│   ├── restore.rs      `.stool.bak` 备份还原 + 双档案切换
│   ├── diagpack.rs     一键诊断包导出（P1-1）
│   ├── precheck.rs     环境预检（P1-2）：目录可写 / 文件占用 / 磁盘空间，
│   │                   失败给「原因 + 修法」；`scope_for_op` 按操作类型选范围
│   ├── batch.rs        [新增] 批量队列（P2-9）：`discover` 扫描候选游戏目录 +
│   │                   `detect_batch` / `run_batch` 逐个处理
│   ├── health.rs       [新增] D1 游戏体检（P2-8）：区域设置 / 日文字体 /
│   │                   运行库 DLL / 路径 / 写权限（复用 precheck 的 Item/Level/Report）
│   └── selfcheck.rs    [新增] 封包自检（P2-6）：解包 → 重打包 → 逐条目比对
│                       （XP3/PCK/asar/RPA/RGSSAD v1·v3；磁盘中转，只比对条目内容）
├── gui/               GUI（P2-11 按页拆分）
│   ├── mod.rs         核心：StoolApp 状态 + Default + `eframe::App::update` 派发 +
│   │                  `exec_op` + 后台任务（spawn/log/refresh_detections/poll_worker）+ `run()`
│   ├── util.rs         通用小工具（页头/路径转义/JSON 值显示与解析/类型提示）
│   ├── save_tree.rs    存档结构树只读渲染（渲染预算、分批展开，见文件内性能注释）
│   └── pages/*.rs      各页 `impl StoolApp`：home / extract / preview / text / save /
│                       runtime / mods / settings / help / log
│                       （子模块 `use crate::gui::*;`，可访问父模块私有字段）
├── cli.rs             命令行入口（子命令分发）
├── memapi.rs          [新增] 跨进程内存 API（`Proc` RAII 句柄 + `open/read/write/query/
│                      regions/modules/force_write`；裸句柄自由函数 `read_raw/write_raw/
│                      query_raw/protect_raw/force_write_raw`；页保护/类型名字映射
│                      `protect_name/kind_name/is_directly_writable/is_executable`；
│                      `list_processes/find_pid`）。由 memscan 与 guard 共用，避免两套
│                      重复的 Win32 调用；`force_write_raw` = 普通写失败 → `VirtualProtectEx`
│                      临时改可写 → 写 → 恢复原保护（返回 `PageFix`）
├── hash.rs            [新增] 无依赖 SHA-256（流式；校验外部工具下载内容）
├── settings.rs        配置（~/.stool/config.json，含版本化与损坏自愈）；
│                      机翻 API Key 用 **Windows DPAPI** 加密落盘（`enc:v1:<hex>`，
│                      旧明文值仍可读，下次保存自动升级）；外部工具/Python 路径
│                      **按跨机器成立的位置探测**（不写死本机绝对路径）
└── diag.rs            诊断：panic 钩子 / 日志落盘 / guard / 时间格式化
```

---

## 3. 数据流

### 3.1 检测 → 操作

```
用户选目录
   │
   ▼
ScanCtx::build(root)            ← 全树只扫一次（root 一级 read_dir + 递归 walkdir）
   │
   ▼
Registry::detect_all(scan)      ← 10 原生插件 + 15 表驱动识别器 + 兜底，各自打分
   │  按 (score, priority) 降序；score ≥ 60 才算"确认识别"
   ▼
Registry::pick_from(&results)   ← 选定引擎（不重新扫描）
   │
   ▼
UI / CLI 展示能力（effective_capabilities）→ 用户点某个 Op
   │
   ▼
precheck::run(root, out, scope_for_op(op))   ← 环境预检（P1-2）：Fail 则中止并给修法
   │
   ▼
engine.<op>(&Ctx)               ← Ctx{root, out_dir, options, progress, cancel}
```

**硬约定**：`Engine::detect_scan` 与各 `op` **不得**自行遍历整棵目录树；
需要目录信息用 `ScanCtx`。这是"批量检测不卡"的前提。

### 3.1.1 解包断点续传（`engines::Resume`，P2-4）

```
Resume::open(out_dir, "extract", root)        ← 读 <out>/.stool_resume_extract.json
   │                                             op/root 不匹配或解析失败 → 从零开始
   ▼
逐条目循环（RGSS / KiriKiri / Godot / asar / Ren'Py）
   ├─ resume.already_done(out, name)  → 已记录 & 产物仍在（大小一致）→ 跳过（连解压/读盘都省）
   └─ write_out_resume(...)           → 写成功即登记；每 64 条自动落盘
   │
   ▼
resume.finish(complete = 无写失败)   ← complete 删除台账；否则 flush 保留供续传
```

- 台账按「条目名 → 字节数」记录，**跳过前必须重新校验**文件存在且大小一致（防用户中途改动产物）。
- `root` 写进台账：换了游戏目录即视为全新任务，避免张冠李戴。
- `--opt:resume=0` 关闭续传（忽略旧台账、从零重写）。
- 取消 / 解析失败等提前返回路径都会先 `flush()`，保证断点不丢。

### 3.2 文本汉化（离线闭环）

```
extract（解包）→ text_extract（提取 CSV）→ text-mtl（机翻）→ text_import（回填）
      │                                                              │
      │                                                   repack（重封包，产出新封包）
      │                                                              │
      │                              pack-apply（一键替换原封包，backup_once 留底）
      │                                                              │
      │                                          archive-toggle（原版 ↔ 汉化 一键切换）
      └── 或走 text-inject（运行时 JSON 注入，不改游戏文件）
```

> MOD 安装同理走 `backup_once` 留底；多 MOD 争用同一文件时 `features::mods::conflicts()` 会告警（P2-10）。

### 3.3 解锁（`features/unlock.rs`）

```
Op::Unlock → Engine::unlock（trait 默认实现）
   │
   ▼
unlock::run(engine_id, ctx, |route| self.unlock_impl(route, ctx))
   │
   ├─ pick_route：显式 route > restore > 自带全CG存档(bundled) > 引擎首选
   │
   ├─ bundled  → 通用实现（复制自带存档进存档目录，覆盖前 backup_once）
   ├─ registry → Unity 私有实现（gallery.rs）
   ├─ savefile → Ren'Py 私有实现（persistent）
   └─ 其余     → 诚实告知"无内置自动实现 + 怎么做"
```

**三个界面共用同一份策略表**（CLI `stool unlock` / egui 解包页 / Tauri 解锁页），
所以「识别依据 / 可用手段 / 默认走法」三处**必须一致** —— 界面自己另判一遍，
就会出现「界面说会走 A、内核实际走 B」，用户按界面点了却报错。
Tauri 侧据此只做转述：

```
cmd::unlock_plan  → unlock::spec_or_generic + find_bundled_saves + find_save_dirs + pick_route
                    （只读，进页面即渲染；单测 unlock_plan_is_read_only 钉住"不写盘"）
cmd::unlock_run   → Ctx.options 填 apply=1 / route= / save_dir= / filter= → exec_op(Op::Unlock)
                    apply 缺省 = 只读报告；落地时覆盖前 backup_once 留 .stool.bak
cmd::unlock_backups / unlock_restore
                  → features::restore（与 CLI `stool restore` 同实现）；restore 只收 .stool.bak
```

`unlock_run_core` 在**本地**先校验 `route`（未知值直接给可选列表）与 `save_dir`（不存在就报错，
不静默退回自动 —— 那会把存档复制到别处），再交给内核，避免用户看到一句更绕的兜底报错。

### 3.4 环境预检（`features/precheck.rs`，P1-2）

```
scope_for_op(op, opts)          ← 导出类=out_only；回填/注入/封包=root_only；解锁未 apply=read_only
   │
   ▼
precheck::run(root, out, scope)
   ├─ 游戏目录存在且为目录        （否则 Fail 立即返回）
   ├─ 输出目录可创建 + 可写       （真实建删临时文件探测）
   ├─ 游戏目录可写               （仅 root 范围）
   ├─ 磁盘空间                   （GetDiskFreeSpaceExW；<100MB Fail / <1GB Warn）
   ├─ 文件占用                   （带写权限 open，只认共享冲突 32/33 → Warn）
   └─ 路径过长                   （>240 字符 → Warn，Windows MAX_PATH）
   │
   ▼
Report{ items } → ok()？继续 : 中止并把 fail_summary() 交给用户

接入点：GUI `spawn()`（按名字传入 scope）、CLI `run_op()`（自动推导）+ `stool precheck` 子命令。
```

缺省**只读预览**；`--opt:apply=1` 才落地。

### 3.5 游戏里改数值（Tauri 版转录链路）

egui 版 `gui/pages/runtime.rs` 是 1,229 行的大页；Tauri 版拆成「四张卡」，但**能力一项没少**，
且内核侧**一行未改** —— 全部是既有 `features::{runtime,memscan,guard,xp3patch}` 的转录：

```
① 按名字改（RPG Maker MV/MZ · 最省事）
   cmd::runtime_mvmz_status   → features::runtime::{find_game_exe, probe_port}
   cmd::runtime_mvmz_connect  → DebugGame::launch(7654) 或 DebugGame::connect(7654)，
                                随后 read_state() + read_names() 存进 MvmzSession
   cmd::runtime_mvmz_refresh  → read_state()，用缓存的 names 把 id 翻译成名字
   cmd::runtime_mvmz_set      → set_gold / set_variable / set_switch / set_item
   （值 ↔ 文本：edit_text_to_json 只在**整串**是数字且无前导零时才转数，"007" 保持字符串）

② 搜数值改（任何引擎 · Cheat Engine 式）
   cmd::runtime_options       → ScanType::ALL / Filter::ALL 转成 CE 口径的下拉项
   cmd::runtime_processes     → features::memscan::list_processes
   cmd::runtime_open          → Scanner::open(pid, ty)
   cmd::runtime_scan          → first_scan / next_scan（首次扫描 / 再次扫描）
   cmd::runtime_hits          → 分页取命中列表（前端另有 HIT_DISPLAY_CAP 截断提示）
   cmd::runtime_write         → write（普通）/ force_write（改页保护后再写）
   cmd::runtime_undo          → 撤销本页所有写入（无非本工具写入时给改法）

③ 进阶（平时用不到，出问题才来）
   cmd::runtime_diag          → guard::analyze(pid, addr, ty, ProbeOpts)
   cmd::runtime_regions       → guard::regions_of（页保护分布）
   cmd::runtime_patch_*       → xp3patch::{list, build, build_changed, remove}
```

**冻结（锁定数值）由前端驱动，内核不起线程**：

```
runtime_freeze        → write_raw 写一次 + 置 Session.frozen / period_ms（clamp 50..5000）
runtime_freeze_tick   → 前端 setInterval 周期性调用，重写一次（写失败静默）
RuntimePage.unmount   → clearInterval
```

这样页面一关就停止写内存，**不会留下改别人进程的孤儿线程** —— 这是改内存工具最不能有的东西。
时间间隔与 `period_ms` 一致，且失败不静默升级为报错（游戏可能已退出，报错也没意义）。

> ⑥ 的 `AppState` 字段（`scanner` / `mvmz`）是 `Arc<Mutex<Option<…>>>`，**不是**裸 `Option`：
> `tauri::State<'_, T>` 的生命周期活不过 `spawn_blocking` 闭包（E0597/E0521），
> 必须 `.clone()` 那个 `Arc` 进闭包。详见 `docs/TAURI重构方案.md` §9.5 坑 ⑨。

### 3.5.1 装MOD（Tauri 版转录链路）

内核 `features/mods.rs` 是**完整的**：安装（覆盖 + 留底 + 存副本）、卸载（还原 + 删记录）、
启停、冲突检测（`conflicts` 当前冲突 / `would_conflict` 安装前预演），另带 5 个单测。
Tauri 侧**一行未改内核**，只做转录：

```
cmd::mods_state    → mods::{list_mods, conflicts}       ← 进页面即调，**只读**
cmd::mods_preview  → mods::would_conflict(rels, name)   ← 安装前预演，只回答「会和谁抢文件」
cmd::mods_install  → mods::install_mod(..., force)      ← 冲突校验在**写盘之前**，拒绝时带原因+修法
cmd::mods_toggle   → mods::toggle_mod                   ← 停用=还原原文件但保留记录
cmd::mods_uninstall→ mods::uninstall_mod                ← 卸载=还原并删除记录
```

界面**不自己判冲突**：`conflict_count` 与 `conflicts[]` 都来自内核同一份策略，
否则会出现「界面说没冲突、内核装的时候却拒绝」。`mods_preview` 的意义是**把这一步提前** ——
内核本来就在写盘前拦，但用户只能等报错；预演让人先看到「抢哪几个文件」，再决定是停用别人还是强行覆盖。

「停用」与「卸载」是两件事，界面上是两个按钮，不能合成一个「删除」：
停用后 MOD 仍在列表里（`enabled=false`），随时能再开；卸载则彻底遗忘。

### 3.5.2 工具箱（Tauri 版转录链路）

「设置 / 日志 / 帮助 / 体检 / 自检」在 egui 版里是五个独立入口，Tauri 版按
重构方案 §5.1 第 1 条**合并成一页** —— 它们都是「出问题才用」，不该各占顶级入口。
页面内部用 `.tabs` 分三块（设置 / 诊断 / 帮助），但**命令层仍是六个单一职责命令**，
页面只是它们的组合视图：

```
cmd::tools_settings → settings::load + tools_dl::spec_by_key   ← 只回「密钥配没配」，不回密钥
cmd::tools_save     → settings::{load,save}（四个 Option，不传=不改）
cmd::tool_set       → settings::{load,save}（写单个外部工具路径）
cmd::tool_download  → tools_dl::download(cfg.proxy) → 回填路径 → settings::save
cmd::tools_health   → features::health::check(root, engine)     ← 区域/日文字体/运行库/路径/写权限
cmd::tools_selfcheck→ selfcheck::{find_archives,check_archive} / check_dir  ← 有封包逐条目比对
cmd::tools_log      → diag::log_path + 读尾部（tail 夹 [50,5000]）
cmd::tools_export_diag → diagpack::export(game_root, out.zip)   ← 日志+环境(脱敏)+检测+备份清单
```

四条设计约束：

1. **命令层不做业务判断**。体检/自检/诊断包全部复用内核既有实现，命令层只把
   `precheck::Report` / `selfcheck::Outcome` 摊成 `CheckRow`。本轮 `git diff` 覆盖到的
   六个内核文件（`health` / `selfcheck` / `diagpack` / `tools_dl` / `settings` / `diag`）全为空。
2. **密钥不过 IPC**。`SettingsOut` 只有 `mtl_key_set: bool`，没有密钥字段；要改密钥去
   「翻译文字」页（同一个 `mtl_save`）。
3. **失败必带修法**。内核 `precheck::Item.fix` 直接映射到 `CheckRow.fix`，界面在问题下缩进渲染。
4. **结构化与文本同源**。`CheckOut` 同时给 `items[]`（渲染）与 `text`（复制到剪贴板），
   两者由同一份报告生成，不会不一致。

### 3.6 封包自检（`features/selfcheck.rs`，P2-6）

```
sniff(archive)                     ← 魔数优先，回退扩展名 → Kind（XP3/PCK/asar/RPA/RGSSAD v1·v3）
   │
   ▼
extract_all(kind, 原封包, work/extract)   ← 逐条目落盘，记录 (名字, 大小, FNV-1a 64)
   │
   ▼
repack(kind, work/extract, work/repack.*) ← 复用各 formats 的 write_paths/pack/write_archive
   │
   ▼
extract_all(kind, 新封包, work/verify)    ← 回读
   │
   ▼
compare(原, 新)
   ├─ 名字按分隔符归一（`\` ≡ `/`，避免 RGSS 写出差异误报）
   ├─ 缺失 / 多出 / 大小变化 / 哈希不一致 → Mismatch
   └─ 一致 → 无损
   │
   ▼
Outcome → report()（复用 precheck Item/Level/Report）→ CLI `selfcheck` / GUI「🧩 封包自检」

比对的是**条目内容**而非封包字节：重压缩 / 条目顺序 / TOC 布局允许变化。
RPA 往返需整包驻留内存（`write_archive` 为内存式 API），总量 >512MB 时跳过往返比对（Warn）；
xp3/pck/asar/rgss 超 2GB 直接拒绝（避免无谓内存压力）。工作目录用完即删。
```

---

## 4. 关键约定（改动前务必遵守）

| 约定 | 位置 | 为什么 |
|---|---|---|
| **备份不可覆盖** | `settings::backup_once` | 反复执行写操作时，备份必须永远保留"首次的原始状态"，否则原始文件永久丢失 |
| **写盘失败要计数上报** | `engines::write_out` + `with_fail_note` | 不能静默吞掉写失败后还报"成功 N 个" |
| **解包可断点续传** | `engines::Resume` + `write_out_resume` | 大封包解包耗时长，中断后不能从零重来；台账按「条目名 → 大小」记录，**重跑前必须校验产物仍在且大小一致**，且 `root` 变了即作废 |
| **解包并行写盘** | `engines::parallel_extract`（+ `Job<T>` / `worker_count`） | 大封包几万条目、几 GB，串行读写太慢。worker 各自开一份 `Source`、原子游标抢条目、结果经 mpsc 回主线程（`Resume`/`Ctx` 非 `Sync`）；`--opt:jobs=N` 调并行度。**目录缓存**在 `safe_out_path`（线程本地），每次操作前 `clear_dir_cache()` |
| **机翻并发 + 重试退避** | `features::translate::{run_batches, retry_batch}` | 机翻是网络型任务，串行太慢且限流会直接失败。`jobs` 批并发（默认 4，`--jobs`/设置可调）；429/5xx/网络抖动按 1s/2s/4s… 指数退避（客户端错立即失败）。**已成功的批次照常回填**，不浪费已花掉的请求；与断点续翻协同 |
| **解析器不 panic** | `formats::safe` | 畸形/截断封包必须返回 `Err`，不能崩整个应用 |
| **解析走流式 Source** | `formats::source::Source` | 大封包不能整包进内存；各格式统一 `parse_index(&mut Source)` + `read_entry(&mut Source, …)`，超 `MEM_THRESHOLD` 自动切 `File::seek` 按需读，4GB 硬上限。等价性由 `tests/streaming.rs` 保证 |
| **XP3 写出必须合规范** | `formats::xp3::{Xp3Version, write_paths_v, version_of}` | 真实 KiriKiri 封包（实测 8 个样本/4 款游戏）都用 `0x17` 现代头；老写法（`u8 0x00` + `u32` 索引偏移 + `[u32 条目数]` 前缀的 TOC）**游戏加载不了**。写侧必须：头部代次可选（`V1`=`magic+u64`，`V2`=`magic+0x17 扩展头`，数据区起点 19/40）；TOC = `[u8 标记][u64 压缩长][u64 原始长][zlib]` + **无计数前缀**的 `File` 条目流（子块长度字段全是 **u64**）；`info.flags` 一律 **0**（bit31 = 引擎加密，乱置会让引擎按密文解出乱码）；`adlr` = 明文内容的 Adler-32。**回写原封包时沿用原封包的代次**（`version_of`） |
| **加密封包不许静默产出** | `xp3::encrypted_count` | KiriKiri 各家加密方案（Cx/Hx…）按游戏定制密钥，STool 不内置解密；读出的是随机字节。检测到加密必须**报错并给替代路线**，绝不能把乱码当解包结果写盘（`others.rs` 解包、`selfcheck.rs` 均遵守） |
| **运行时改动一律「文件级」** | `features::xp3patch` + `features::inject` | 项目边界是**不进目标进程**（见 §7）。KiriKiri 系靠 `patchN.xp3`（引擎搜索顺序 `Data 目录 → data.xp3 → patch.xp3 → patch2 → …`，后者覆盖前者）；JS/DOM 系靠新增脚本文件；**绝不用来覆盖游戏自带的补丁包**（`build` 拒绝非本工具创建的包，`remove` 靠 sidecar `<name>.stool.json` 认定归属） |
| **路径穿越防护** | `engines::safe_out_path` | 过滤 `..` / 绝对路径，防 zip-slip 式越界写 |
| **检测零额外 IO** | `ScanCtx` | 一次扫描供全部插件查表 |
| **panic 可诊断** | `diag::guard` + panic hook | GUI 无控制台，panic 必须落日志并能展示成错误 |
| **配置损坏自愈** | `settings::load` | 解析失败要备份 + 告警，不静默丢设置 |
| **写操作先预检** | `features::precheck` | 目录不可写 / 磁盘不足等**可预知**的失败要在开跑前拦下并给修法，别跑到一半才炸（几 GB 解包尤其致命） |
| **CI 门禁：clippy + 测试** | `.github/workflows/ci.yml` | `clippy -D warnings` 与 `cargo test --tests` 必须过 |
| **密钥不明文落盘** | `settings::{encrypt_key, decrypt_key}` | `mtl_key` 用 **Windows DPAPI** 加密后写入配置（`enc:v1:<hex>`）。旧明文值仍能读，下次保存自动升级；密文绑定当前 Windows 用户，跨机器解不开时告警 + 清空该字段（不能把解不开的垃圾当密钥用） |
| **外部工具下载必须校验** | `features::tools_dl` + `hash::Sha256` | 下载的是**随后会被执行**的可执行文件，只靠 HTTPS 不够。流式落盘 + 边算 SHA-256，与 GitHub Release `digest` 比对，不一致即中止并删除临时文件；上游无摘要时把校验和记到 `<工具目录>/.sha256` |
| **不写死本机绝对路径** | `settings::{unrpyc_path, python_path}` | 硬编码 `D:/STool/...` 换机器即失效，且会静默回退到系统 `python`。改为**按跨机器成立的位置顺序探测**（配置 → 环境变量 → `~/.stool/tools` → skill 副本；Python 扫描 `versions/*` 取最新） |
| **写内存要二次确认** | `features::memscan` + `gui/pages/runtime.rs` | 内存扫描会直接改写目标进程内存（修改器本质）。首次写入/锁定前必须确认（GUI 弹窗，`ms_ack` 勾选后不再问），失败/拒绝路径不得静默通过 |
| **反修改探测零副作用 + 不碰反作弊** | `features::guard` + `memapi::force_write_raw` | 诊断只是"写探测值 → 观察 → 恢复原值"，探测值不写用户的真实值，每轮结束恢复原页保护。阈值判定要实测校准（`classify_revert` 用「存活满窗口」区分周期回滚与延迟回滚，`recommended_period_ms` 间隔 <1ms 时不给锁值方案）。识别到**在线反作弊**只提示风险、**不提供绕过** |
| **重扫按需** | `gui::StoolApp::det_dirty` | 操作完成后是否重扫引擎由 `det_dirty` 决定：解包/反编译/文本提取只写输出目录 → 不重扫（大目录下这一步很贵）；`spawn` 默认置 `true`，只有明确不改游戏目录的操作才置 `false` |
| **列表要设上限** | `features::preview::MAX_MEDIA` | 素材目录可能有几万个文件，全量列表会拖垮 UI；达到上限提前停止并如实告知被截断 |
| **格式解读必须用真机样本定案** | `verify/`、`tools/peek_*.py` | 合成样本由自己写，**自己写错的假设会被自己的样本"证实"**。例如 IL2CPP 的 `len` 到底是"精确字节长度"还是"含结尾 NUL"，两种解读都能 97% 解出合法 UTF-8 —— 只有真机样本（6 款游戏 / 6 个 metadata）能判：切出来的是**完整串**还是**被截尾的碎片**。结论记进模块文档顶部，并留一条**自检**把错误解读挡在门外 |
| **解析器要"宁退回不硬读"** | `formats/il2cpp.rs`、`features::gallery::scan_precise` | 布局校验（magic / 版本 / 区首尾相接 / 字面量 UTF-8 可解率 ≥90%）任一不过就 `Err`，由调用方退回启发式扫描。**产出乱码候选比不产出更糟** —— 用户会把垃圾键写进注册表 |
| **候选列表要先筛后排再截断** | `features::gallery::{looks_like_key, sort_candidates}` | 上千条候选里真键只有几条：白名单字符集（`[A-Za-z0-9_ .-]` + 非 ASCII、首末字符必须字母数字）+ base64 常量块剔除 + 按"像不像画廊键"分层排序，**最后才截断**（否则字典序 + 截断会把真键挤出去） |
| **测试的临时目录每个用例必须唯一** | `src/formats/pfs.rs`（已踩坑） | `tmp()` 会先 `remove_dir_all`；两个用例共用同一目录时，cargo 并行跑测试会互相删掉对方的文件（表现为偶发 `fs::read` 失败）。标签必须逐调用点唯一 |
| **代码风格：宽行（120 列）** | `stool-rs/rustfmt.toml` | 历史代码为手工维护的宽行风格；**不设 fmt 门禁**（全量格式化会改动约 2000 行/37 文件），改代码时对齐相邻代码即可 |
| **GUI 语义色只从 `gui::util` 取** | `gui/util.rs`（`C_OK`/`C_WARN`/`C_DANGER`/`C_MUTED`） | 颜色散写在页面里 → 同一个含义在不同页面颜色不一样，换主题还要满仓找。新增状态色先进 `util.rs` |
| **GUI 分区统一用卡片** | `gui/util::{card, card_title}` | 各页面用同一种容器（浅色圆角 + 内边距 + 描边），分区一眼可辨；别在页面里各写 `Frame::group(...)` 变体 |
| **大列表必须虚拟化** | `egui::ScrollArea::show_rows` | egui 的 `ScrollArea::show` **不做视口裁剪**——它会把所有子控件布局一遍来量高度。上千行的列表/结构树必须走 `show_rows`（只布局可见行），否则滚动卡死 |
| **每帧别读盘、别做重分配** | 如 `save.rs::save_locs_sync`、`on_hover_ui` | 渲染循环里读盘/拼 `String` 会按行数×帧率放大。列表数据要缓存 + 指纹校验；悬停提示用惰性 `on_hover_ui`，别用每帧分配 `String` 的 `on_hover_text` |
| **不替换 GUI 框架（egui）** | `gui/` | 重写整层 UI（如换 Tauri）的收益只是"好看一点"，代价是全部页面 + 拖拽/文件对话框/硬件加速/无控制台诊断等基建重做，且与「好用优先、功能不回退」冲突。美观与易用性的整改**在 egui 内做**（卡片、语义色、排版、可操作空状态） |

---

## 5. 改哪里（速查）

| 需求 | 改哪几个文件 |
|---|---|
| **新增一个引擎** | `engines/scan.rs`（白名单）→ `engines/recognize.rs` 或新插件 → `features/unlock.rs` 加一行 `UnlockSpec` → `docs/引擎识别依据与解锁策略.md` → 测试 |
| **新增一种封包格式** | `formats/<fmt>.rs`（`parse_index(&mut Source)` + `read_entry`，别整包读）：+ `formats/mod.rs` + `tests/roundtrip.rs` + `tests/fuzz_parsers.rs` + `tests/streaming.rs`（内存/文件双模式等价） |
| **新增一个并行解包循环** | `engines::Resume::open(out, "extract", root).configured(ctx)` → 过滤 `already_done` 组装 `Vec<Job<T>>` → `parallel_extract(&jobs, out, worker_count(ctx), ctx, \|\| Source::open(arc, MAX_ARCHIVE), read_fn, &mut resume)` → 结尾 `resume.finish(failed == 0)` + `with_resume_note`；提前 return 前 `resume.flush()` |
| **新增一种汉化注入** | `features/inject.rs` 的 `SUPPORT_TABLE` + 对应引擎的 `text_inject` |
| **新增一个 CLI 子命令** | `cli.rs`：`missing_arg_usage`（若有必填参数）+ `main_args` 分支 + 未知命令提示 |
| **新增一个 GUI 页面** | `gui/mod.rs`：`Page` 枚举 + 左侧导航 + `update` 派发分支；新页 `impl StoolApp` 放 `gui/pages/<name>.rs`（`use crate::gui::*;`），并在 `gui/pages/mod.rs` 声明 `mod <name>;` |
| **改 GUI 风格 / 加一处分区** | `gui/util.rs`：`page_header` + `card`/`card_title` 做分区，语义色只用 `C_OK/C_WARN/C_DANGER/C_MUTED`；大列表走 `ScrollArea::show_rows` 虚拟化；页面级空状态用 `need_detect(ui, "页面名")`（给用户一条出路，别只丢"请先去首页"）。视觉验证见 §6.1 |
| **新增外部工具** | `settings.rs`（字段 + `external_tool` 分支）+ `features/tools_dl.rs` + 设置页 |
| **新增配置项** | `settings.rs`（加 `#[serde(default)]` 字段；纯追加不用升版本） |
| **新增一个机翻设置项** | `settings.rs`（`mtl_*` 字段 + `default_mtl_*()`）+ `gui/pages/text.rs`（`mtl_*: String` 字段在 `gui/mod.rs` 的 `StoolApp` 中 + 构造/`dirty` 判定/UI 输入）+ CLI `text-mtl --*` |
| **新增一项环境预检** | `features/precheck.rs`（在 `run()` 里 push 一个 `Item`）；需限定触发范围时改 `scope_for_op` |
| **新增一项游戏体检** | `features/health.rs`（在 `check()` 里 push 一个 `Item`） |
| **新增一种可自检封包** | `features/selfcheck.rs`：`Kind` 加变体 + `sniff` 魔数 + `extract_all`/`repack` 两个 match 分支（含 `label`/`ext`） |
| **新增一种 Unity 精确键名来源** | `features/gallery.rs`：`PreciseKind` 加变体 + `scan_precise` 里加一个分支（读出来 → `collect_literals` 进候选、`gallery_hits` 进报告）+ 报告里的 `precise_note`；若解析逻辑复杂，先落到 `formats/<fmt>.rs` 并配单测/模糊测试 |
| **新增一个"读元数据字符串"的格式** | `formats/<fmt>.rs`：`read_strings(path)` + `parse_metadata(&[u8])`，复用 `dotnet::{parse_strings_heap, gallery_hits_of, is_gallery_like}`（名字堆与画廊判据是共用口径） |
| **新增一个运行时补丁包引擎** | `features/xp3patch.rs`：`PATCH_TARGETS` 加一行 + `next_name` 的命名规则 +（若有新封包格式）`formats/<fmt>.rs` 的写侧；GUI/CLI 入口会自动读到新表 |
| **新增一条反修改保护线索** | `features/guard.rs`：模块名 → `MODULE_TABLE`；反调试 API → `ANTIDEBUG_IMPORTS`；新的指令形式 → `for_each_imm_write` 解码分支；新方案 → `build_plans` 里 push 一个 `Plan`（带 `Feasibility` / 可选 `AutoAction`）；**新判定/新方案都要补 `features/guard_tests.rs`**。真机回归用 `scripts/mock_mem_guard.py` + `scripts/_guard_smoke.sh`（靶子进程，别拿合成数据"自证"） |

---

## 6. 构建与验证

```bash
# 本机 Git Bash。注意：stool-rs/.cargo/config.toml 被 .gitignore 排除（写死了本机 MSVC 路径），
# 新克隆的仓库没有它 → 必须自己把 VC 运行库加进 PATH，否则链接阶段会找不到库。
export PATH="/usr/bin:$PATH"   # 本机 bash 的 PATH 可能不含 Git 自带 /usr/bin（否则 ls/tail/sed 都找不到）
export PATH="/c/Program Files/Microsoft Visual Studio/<版本>/VC/Redist/MSVC/<版本号>/x64/Microsoft.VC143.CRT:$PATH"
export PATH="/c/Users/<user>/.cargo/bin:$PATH"
# rustup 工具链的 bin 必须在 PATH 里 —— cargo-clippy.exe 是独立 exe，要就地找 std-*.dll。
# 缺了它 `cargo clippy` 会报 "error while loading shared libraries: std-<hash>.dll"（退出码 127），
# 而 `cargo build` / `cargo test` 照样能跑 —— 很容易误判成「clippy 坏了」。
export PATH="/c/Users/<user>/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin:$PATH"
export RUSTC="C:/Users/<user>/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin/rustc.exe"
export CARGO_INCREMENTAL=0   # 避免 target/ 增量目录偶发「拒绝访问(os error 5)」
cd stool-rs

cargo clippy --all-targets -- -D warnings   # lint 门禁（CI 强制）
cargo test --tests                  # 单元 + 集成（fuzz_parsers / roundtrip / streaming / parallel）
cargo build --release
# cargo fmt 不设门禁（宽行风格，见 rustfmt.toml）；如要局部对齐可手动跑
# cargo fmt --all -- --check
```

### 6.1 GUI 视觉验证（改 GUI 后必做）

> 本节针对 **egui 版**（`stool-rs/src/gui/`）。**Tauri 版**（`stool-tauri/`）的验证见 §6.2 ——
> 那边用 `preview.html` 离线渲染，不需要 `capture.ps1`。

窗口是硬件加速的，普通截屏抓不到内容，用 `shots/capture.ps1`（`PrintWindow` + `PW_RENDERFULLCONTENT`）：

```powershell
# 启动页由环境变量 STOOL_PAGE 指定；页面名见 gui::Page
powershell -ExecutionPolicy Bypass -File shots/capture.ps1 -Page save -OutPath D:\STool\shots\x.png
```

`gui/mod.rs` 里另有一组**调试钩子**（都是环境变量，走与手动操作完全相同的代码路径）：

| 变量 | 作用 |
|---|---|
| `STOOL_PAGE` | 启动即停在该页（`home` / `extract` / `preview` / `text` / `save` / `runtime` / `mods` / `settings` / `help` / `log`） |
| `STOOL_PID=<pid>` | 预选内存扫描的目标进程并**建立扫描会话**（等价于手动在「目标进程」里点一下） |
| `STOOL_SCAN=<数值>` | 配合 `STOOL_PID`，启动即跑一次「首次扫描」——用来无头验证扫描链路 |
| `STOOL_SAVE=<路径>` | 启动即加载该存档并切到存档页 |
| `STOOL_SAVE_SEL=<JSON Pointer>` | 载入存档后预选一个字段（验证编辑卡片） |
| `STOOL_SEARCH=<关键词>` | 载入存档后立刻搜一次（验证虚拟化结果列表） |

GUI 自身的 Windows 辅助文件在 `shots/`（截图）与 `scripts/`（`mock_mem_guard.py` 内存保护靶子、
`mock_mtl_server.py` 假机翻服务、`peek_xp3.py` 独立 xp3 解析器）。


> 风格约定：本项目使用 **120 列宽行风格**，非 rustfmt 默认 100 列；
> `rustfmt.toml` 已按此配置，但 CI **不**做 `fmt --check` 门禁，
> 以免一次性格式化冲掉 2000 余行历史代码。新增/修改代码请对齐相邻代码风格。

> `cargo test`（全量）会连带跑 doctest；本项目 doc 里的代码块都标注为 ` ```text `，
> 因此没有 doctest。若在特定环境遇到 doctest 执行器报 DLL 解析失败，用 `--tests` 即可。

---

### 6.2 Tauri 侧构建（`stool-tauri/`）

```bash
cd stool-tauri/src-tauri
cargo build --release     # 开发时 cargo run
```

**必须知道的一个坑**：`tauri/build.rs` 里是 `let dev = !custom_protocol;`。
不带 `custom-protocol` feature 时，**连 release 构建也会被判成 dev 模式**，
Tauri 会去找前端 dev 服务器 —— 结果是**一个静默的白窗口**（没有任何报错）。
`stool-tauri/src-tauri/Cargo.toml` 已写死 `features = ["custom-protocol"]`，
所以裸 `cargo build` 也能跑；走 Tauri CLI 时 CLI 会自动加这个 feature。
另外 `tauri-build` 在 Windows 上**强制要求** `src-tauri/icons/icon.ico`，缺了直接构建失败。

验证分三层，**都不靠抓像素**（WebView2 走 DirectComposition，`CopyFromScreen` 与 `PrintWindow` 都不可靠；
且反复 `taskkill` 会破坏它的窗口类状态 —— `Failed to unregister class Chrome_WidgetWin_0. Error = 1411`，
此后启动静默失败，所以「强杀后的 GUI 冒烟」不能当验收手段）：

1. **命令层单测**（`cd stool-tauri/src-tauri && cargo test`）：命令层里真正干活的
   `detect_impl` / `load_doc` / `run_op_core` 都是**不碰 Tauri 类型的纯函数**，可直接对
   `verify/` 下的真机样本测，不必拉起 WebView。样本不在就跳过 —— 不造假绿。
   > 解包的**正向**路径只能用未加密样本（`verify/tail_test`）；`verify/komoguri` 三个 xp3 全加密，
   > 拿它当成功用例只会得到「乱码 == 乱码」的假无损。
2. **离线看界面**：`preview.html` 用**同一套前端** + 内存假后端渲染，不需要 WebView2。
   支持 `#<页id>` 直达某页、`?theme=dark` 看深色、`?autorun=1` 自动跑一次看进度条。
   「看素材」另有 `?kind=image|audio|text` 预选类型、`?pick=N` 预选第几个文件；
   「翻译文字」另有 `?fold=1` 展开表格预览、`?mtl=1` 展开机翻设置。
   > 这个预置脚本**必须放在所有页面脚本之后** —— 它直接改 `PreviewPage` / `TextPage` 的字段，
   > 放前面会因页面对象还没定义而静默失效（踩过一次：`?kind` 不生效，两张截图 MD5 相同）。
   截图可在无头 Chromium/Edge 里做（产物 `shots/tauri/`），这条路完全绕开 WebView2。
   两个**静默失败**的坑：`--screenshot` 的目标必须**绝对路径**（相对路径报
   `拒绝访问 (0x5)` 但 Edge 退出码仍为 0）；`--user-data-dir` 要指向项目内固定目录
   （临时目录会让 headless 起不来且不写文件，同样没有非零退出码）。深浅两版各用一份干净
   profile —— 主题存 `localStorage`，共用 profile 会让深色版与亮色版像素级相同。
3. **日志通道**：前端把生命周期事件经 `log_line` 命令写到 `STOOL_TUI_LOG`。

#### 6.2.1 文本预览的编码判定（`features::preview::decode_text`）

**唯一实现在内核**，egui 版与 Tauri 版共用 —— 别在界面侧再写一份。
判定顺序与理由（每一条都是真机踩出来的）：

| 顺序 | 判据 | 为什么必须在此时判 |
|---|---|---|
| 1 | BOM（UTF-8 / UTF-16LE / UTF-16BE） | 有 BOM 就没有歧义 |
| 2 | **无 BOM UTF-16 嗅探**（NUL 规律落奇/偶位） | **必须早于严格 UTF-8**：纯 ASCII 的 UTF-16LE 是 `x, 0x00` 交替，而 NUL 是合法 UTF-8 字符，按 UTF-8 能「成功」解出 `/\0/\0 \0C\0…` —— 不报错但没法读 |
| 3 | 严格 UTF-8 | 通过即无歧义 |
| 4 | Shift-JIS + **双重质量闸** | 日文游戏默认编码 |
| 5 | 判定 `DecodedText::Binary` | 都解不动 → 老实说「不是文本」 |

**双重闸**（`acceptable()`）的必要性：Shift-JIS 有大量**单字节半角片假名**（0xA1–0xDF），
随机字节很容易被「成功」解码成一屏 `ﾏ0沢Sｴ` —— **零替换字符**，只靠替换字符闸拦不住。
加密封包解出的 `.ks`/`.tjs` 残留正是这种。真机实测：

| | 替换字符 | 半角片假名 |
|---|---|---|
| 正常日文脚本（`tail_test/out/unencrypted`） | 1~4% | 1~4% |
| 加密残留（`komoguri_out`） | 12~43% | 12~33% |

中间仍有**重叠带**（个别密文落在 8% 边缘），所以判为 Binary 时**不当作故障**：
界面给「用系统程序打开」这条出路，功能不会因此不可用。

启动钩子（都是环境变量，走的路径与手动点击**完全相同**，不是特制分支）：

| 变量 | 作用 |
|---|---|
| `STOOL_GAME=<目录>` | 启动即在「选游戏」页对指定目录跑一次检测 |
| `STOOL_SAVE=<存档文件>` | 启动即在「改存档」页打开该存档 |
| `STOOL_QUERY=<关键词>` | 打开存档后自动执行一次搜索 |
| `STOOL_OUT=<目录>` | 「取出素材」的默认输出目录 |
| `STOOL_PAGE=<页id>` | 启动即停在该页（与 egui 版同一口径） |
| `STOOL_TUI_LOG=<日志文件>` | `log_line` 的落点（默认在系统临时目录） |

---

## 7. 数据与安全边界

- **不入库**：`verify/`（真实游戏数据）、`tools/`（下载的外部工具）、
  `stool-rs/target/`、`.workbuddy/`、`.cargo/config.toml`（本机 MSVC 路径）。
  （`Cargo.lock` **入库**：本项目含 `[[bin]]`，属二进制应用，按官方建议锁定依赖版本。）
- **本机数据**：`~/.stool/config.json`（机翻 API Key 用 DPAPI 加密后落盘，见 §4）、
  `~/.stool/logs/stool-YYYY-MM-DD.log`（日志，导出诊断包时 Key 会脱敏）、
  `~/.stool/tools/<工具名>/`（下载的外部工具 + `.sha256` 校验记录）。
- **不注入、不提权**：所有外部工具都是独立进程 + 只写文件，不进目标进程。
  因此「运行时改动」一律做成**文件级**：KiriKiri 系 = `patchN.xp3`（`features/xp3patch.rs`），
  JS/DOM 系 = 新增脚本文件（`features/inject.rs`），MV/MZ 另有 CDP 远程调试改内存。
  **明确不做**：进程内 DLL/API 注入、Wolf RPG 注入（无验证样本，其数据本就在 `Data/` 目录里，
  直接改文件即可）、以及需要破解他人加密方案的解密。
- **写内存的例外要打招呼**：通用内存扫描（`features/memscan.rs`）会直接改写目标进程内存，
  是唯一"进内存"的能力，因此首次写入前强制二次确认（见 §4）。
- **反修改诊断只做临时写入并恢复原值**：`features/guard.rs` 的写入-存活探测会往目标地址写探测值
  （不写用户想要的值），每轮结束前**恢复原值**，不产生持久副作用；改用 `force_write` 解除页保护时
  也在写完后立刻恢复原页保护。**识别到在线反作弊（EAC/BattlEye/Vanguard/ACE…）只列风险提示，
  不提供任何绕过**——这条是硬边界，不许扩展成"对抗反作弊"。
- **不做非法解密**：保护类加密只给明确报错与替代路线（KiriKiri 加密封包会明确报错并指向 GARbro / KrkrExtract）。
