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
│   └── external.rs      Wolf（WolfDec 委派）、Unity（AssetRipper 委派 + 注册表解锁）
├── formats/           纯格式层（无 IO 之外的副作用），每个都有 roundtrip 测试
│   ├── source.rs       [新增] 流式数据源（P2-1）：Mem(≤128MB)/File 双模式，read_at 按需读，MAX_ARCHIVE=4GB
│   ├── safe.rs         边界安全读取原语（P0-2）：越界一律返回 None，解析器不再 panic
│   ├── xp3.rs / pck.rs / asar.rs / rgss.rs / rpa.rs …
│   │                   各格式均提供 `parse_index(&mut Source)` + `read_entry(&mut Source, …)`
│   │                   （按偏移按需读，替代整包 `read_to_end`；RGSS 为 parse_index_v1/v3 + read_entry_v1/v3）
│   │                   xp3.rs 另含写侧：`Xp3Version{V1,V2}` / `version_of` / `write_paths_v` /
│   │                   `encrypted_count`（见 §6 约定）
├── features/          横向功能
│   ├── unlock.rs        [新增] 统一解锁抽象：UnlockRoute / UnlockSpec / UNLOCK_CATALOG /
│   │                    find_save_dirs / find_bundled_saves / pick_route / run
│   ├── gallery.rs      Unity PlayerPrefs 注册表（djb2 哈希、Win32 读写、快照/还原）
│   ├── inject.rs       运行时 JSON 注入汉化（SUPPORT_TABLE 声明范围与边界）
│   ├── translate.rs    机翻（OpenAI 兼容；术语表、断点续翻、`jobs` 批并发 + 429/5xx 指数退避）
│   ├── tpack.rs        翻译包导出/导入
│   ├── saves.rs        存档文档模型（多格式解析 + 指针寻址编辑）
│   ├── mods.rs         MOD 安装/启停/卸载
│   ├── runtime.rs      运行时修改（内存/脚本层）
│   ├── xp3patch.rs     [新增] KiriKiri 运行时补丁包（P2-12）：`list` / `next_name` /
│   │                   `build`（打 patchN.xp3，原封包不动）/ `remove`（只删自己建的）
│   ├── memscan.rs      进程内存扫描（RAII 句柄，MAX_REGION 限制）
│   ├── preview.rs      资源预览（图片/音频列表）
│   ├── tools_dl.rs     外部工具下载
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
├── settings.rs        配置（~/.stool/config.json，含版本化与损坏自愈）
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

### 3.5 封包自检（`features/selfcheck.rs`，P2-6）

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
| **代码风格：宽行（120 列）** | `stool-rs/rustfmt.toml` | 历史代码为手工维护的宽行风格；**不设 fmt 门禁**（全量格式化会改动约 2000 行/37 文件），改代码时对齐相邻代码即可 |

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
| **新增外部工具** | `settings.rs`（字段 + `external_tool` 分支）+ `features/tools_dl.rs` + 设置页 |
| **新增配置项** | `settings.rs`（加 `#[serde(default)]` 字段；纯追加不用升版本） |
| **新增一个机翻设置项** | `settings.rs`（`mtl_*` 字段 + `default_mtl_*()`）+ `gui/pages/text.rs`（`mtl_*: String` 字段在 `gui/mod.rs` 的 `StoolApp` 中 + 构造/`dirty` 判定/UI 输入）+ CLI `text-mtl --*` |
| **新增一项环境预检** | `features/precheck.rs`（在 `run()` 里 push 一个 `Item`）；需限定触发范围时改 `scope_for_op` |
| **新增一项游戏体检** | `features/health.rs`（在 `check()` 里 push 一个 `Item`） |
| **新增一种可自检封包** | `features/selfcheck.rs`：`Kind` 加变体 + `sniff` 魔数 + `extract_all`/`repack` 两个 match 分支（含 `label`/`ext`） |
| **新增一个运行时补丁包引擎** | `features/xp3patch.rs`：`PATCH_TARGETS` 加一行 + `next_name` 的命名规则 +（若有新封包格式）`formats/<fmt>.rs` 的写侧；GUI/CLI 入口会自动读到新表 |

---

## 6. 构建与验证

```bash
# 本机 Git Bash（MSVC 工具链由 stool-rs/.cargo/config.toml 固定）
export PATH="/c/Users/<user>/.cargo/bin:$PATH"
export RUSTC="C:/Users/<user>/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin/rustc.exe"
cd stool-rs

cargo clippy --all-targets -- -D warnings   # lint 门禁（CI 强制）
cargo test --tests                  # 单元 + 集成（fuzz_parsers / roundtrip）
cargo build --release
# cargo fmt 不设门禁（宽行风格，见 rustfmt.toml）；如要局部对齐可手动跑
# cargo fmt --all -- --check
```

> 风格约定：本项目使用 **120 列宽行风格**，非 rustfmt 默认 100 列；
> `rustfmt.toml` 已按此配置，但 CI **不**做 `fmt --check` 门禁，
> 以免一次性格式化冲掉 2000 余行历史代码。新增/修改代码请对齐相邻代码风格。

> `cargo test`（全量）会连带跑 doctest；本项目 doc 里的代码块都标注为 ` ```text `，
> 因此没有 doctest。若在特定环境遇到 doctest 执行器报 DLL 解析失败，用 `--tests` 即可。

---

## 7. 数据与安全边界

- **不入库**：`verify/`（真实游戏数据）、`tools/`（下载的外部工具）、
  `stool-rs/target/`、`.workbuddy/`、`.cargo/config.toml`（本机 MSVC 路径）。
  （`Cargo.lock` **入库**：本项目含 `[[bin]]`，属二进制应用，按官方建议锁定依赖版本。）
- **本机数据**：`~/.stool/config.json`（含机翻 API Key，明文，仅本机自用）、
  `~/.stool/logs/stool-YYYY-MM-DD.log`（日志，导出诊断包时 Key 会脱敏）。
- **不注入、不提权**：所有外部工具都是独立进程 + 只写文件，不进目标进程。
  因此「运行时改动」一律做成**文件级**：KiriKiri 系 = `patchN.xp3`（`features/xp3patch.rs`），
  JS/DOM 系 = 新增脚本文件（`features/inject.rs`），MV/MZ 另有 CDP 远程调试改内存。
  **明确不做**：进程内 DLL/API 注入、Wolf RPG 注入（无验证样本，其数据本就在 `Data/` 目录里，
  直接改文件即可）、以及需要破解他人加密方案的解密。
- **不做非法解密**：保护类加密只给明确报错与替代路线（KiriKiri 加密封包会明确报错并指向 GARbro / KrkrExtract）。
