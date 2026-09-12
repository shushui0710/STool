# STool 工程评估与改进方向

> 评估时间：2026-09-10（代码规模 13,880 行 Rust）
> 评估方式：静态审查 + **实测复现**（不靠印象）。凡标 "已复现" 的结论都有可重跑的命令与输出。
> 与 `docs/STool改进方案.md` 的分工：那份偏**功能规划**（机翻管线 / 注入路线 / 生态），
> 本份偏**工程底子**（健壮性 / 代码质量 / 性能 / 可维护性 / 安全），并列出那份文档里没覆盖的缺口。

---

## 0. 结论摘要

整体评价：**功能面推进很快，工程底子明显滞后**。核心解析链路存在可复现的崩溃与数据丢失级缺陷，
而死代码与单体 UI 已经构成了新增功能的摩擦成本。按"自己用得好"的标准，最该先补的不是新功能，是**不崩、不丢数据、出错能查**。

| # | 问题 | 严重度 | 状态 |
|---|---|---|---|
| 1 | 备份被无条件覆盖 → 二次回填会把"原始文件"备份顶掉，**原始文件不可恢复** | 🔴 高（数据丢失） | 已复现（代码级） |
| 2 | 畸形/截断封包触发 slice 越界 **panic**，应用直接崩 | 🔴 高（崩溃） | **已复现** |
| 3 | GUI 无控制台，panic 信息被丢弃，界面只留"任务线程异常退出" | 🟠 中（不可诊断） | 已复现（代码级） |
| 4 | CLI 缺参数 **panic**，退出码 101 + 裸 panic 栈 | 🟠 中 | **已复现** |
| 5 | 日志只在内存（上限 500 条），退出即丢；无日志文件 | 🟠 中（不可追溯） | 代码级 |
| 6 | 关键写盘 `let _ = fs::write(...)` 静默失败，仍报"成功 N 个" | 🟠 中（结果不可信） | 代码级 |
| 7 | 13 处 `Engine::describe` 实现 + `capability_matrix` + `Registry::pick` **零调用** | 🟡 低（维护负担） | 已核 |
| 8 | `Cargo.lock` 被 `.gitignore` 排除（二进制项目应锁定依赖） | 🟡 低（供应链可复现性） | 已核 |
| 9 | 机翻 API Key 明文存 `~/.stool/config.json` | 🟡 低（本机自用可接受） | 代码级 |
| 10 | 外部工具下载无哈希校验 | 🟡 低 | 代码级 |
| 11 | `gui.rs` 2906 行 / 55 函数，最长函数 342 行 | 🟡 低（可维护性） | ✅ 已补（P2-11 拆为 `gui/mod.rs` 997 行 + 12 个子模块） |
| 12 | 解析器整包读入内存无上限（7 处 `read_to_end`），大封包 OOM 风险 | 🟡 低 | ✅ 已补（P2-1 `formats::source` 流式化 + 4GB 上限） |

---

## 1. 功能完整性

### 1.1 已经做得好的

- **解包侧覆盖广**：10 个原生插件（Ren'Py / MV·MZ / RGSS / KiriKiri / Godot / NScripter / TyranoBuilder / HTML·Electron / Wolf / Unity）
  + 15 个表驱动盲区识别器 + GARbro 兜底；识别、解包、文本提取、回填、重封包、备份/还原形成闭环。
- **注入侧范围明确**（4 引擎）：Ren'Py、MV/MZ、TyranoBuilder、HTML/Electron，且每种的适配方式与边界都写进了
  `features::inject::SUPPORT_TABLE`，GUI/CLI 都能查。
- **自用场景的克制**：不做云服务/账号、不做非法解密、尊重"人工修正 > 机翻"。

### 1.2 缺口（对照 `docs/STool改进方案.md`，标注实际完成情况）

| 方案项 | 实际状态 |
|---|---|
| A1 快速行动卡片 | ✅ 已做（首页"💡 推荐流程"卡） |
| A2 环境预检（写权限/文件占用/磁盘空间） | ✅ 已做（`features/precheck.rs`；CLI `precheck` + GUI 任务前置） |
| A3 帮助文案可搜索 | ✅ 已做（帮助页） |
| A4 版本更新提示 | ❌ 未做 |
| B1 全自动机翻管线 | ✅ 已做 |
| B2 翻译引擎插件化 + 自有 key/本地模型 | 🟡 自有 key 已做（OpenAI 兼容）；**纯本地兜底模型（Argos/NLLB）未做** |
| B3 术语表一致性 | ✅ 已做（`mtl_glossary`） |
| B4 断点续翻 | ✅ 已做（旁车 `.mtl.json`） |
| C1 双档案切换 | ✅ 已做（`archive-toggle` / `restore`） |
| C2 运行时注入（KiriKiri/RGSS/Wolf） | 🟡 **定案：一律走「文件级」，不做进程注入**。KiriKiri 系已做 `patchN.xp3` 运行时补丁包（原封包不动、删除即还原）；Ren'Py / MV·MZ / Tyrano / HTML 已由 JSON 注入覆盖；MV·MZ 另有 CDP 实时改；RGSS/Wolf 走「回写封包 / 存档改写 / 解锁目录 / 双档案切换」等离线替代（Wolf 无样本，明确不做，见下） |
| D1 "游戏跑不起来"体检 | ✅ 已做（`features/health.rs`，CLI `doctor` + GUI 体检按钮） |
| D2 还原入口常在手边 | ✅ 已做（还原按钮 + 操作完成提示） |
| E1 翻译包导出/导入 | ✅ 已做（`tpack`） |
| F1 CI | ✅ 已加门禁（`clippy -D warnings` + `cargo test --tests`；fmt 因宽行风格不设门禁） |
| F2 移除已知噪音 | ✅ 已做（零警告） |

### 1.3 本报告新发现的功能缺口（原方案未覆盖）

1. **无诊断包导出能力** → ✅ 已补（P1-1 `features/diagpack.rs` + CLI `diag-export`）。
2. **无解包断点续传**：几 GB 的封包解到一半取消/崩溃，只能从头再来（`Ctx::cancelled()` 只支持中止，不支持恢复）。→ ✅ 已补（P2-4 `engines::Resume` + `write_out_resume`，台账 `<out>/.stool_resume_extract.json`）
3. **无"回填一键闭环"** → ✅ 已补（P2-5 `restore::apply_repack` + CLI `pack-apply`）。
4. **无封包完整性校验** → ✅ 已补（P2-6 `features/selfcheck.rs` + CLI `selfcheck` + GUI「🧩 封包自检」）：解包 → 重打包 → 逐条目比对，把 `verify/*_diff.txt` 的人工流程产品化。
5. **无批量/队列** → ✅ 已补（P2-9 `features/batch.rs` + CLI `batch` + GUI 批量检测）。
6. **MOD 无依赖/冲突校验** → ✅ 已补（P2-10 `mods::conflicts` + `mod-conflicts`）。

---

## 2. 代码质量

### 2.1 优点

- 注释密度 6%，但**关键设计决策**（为什么这样选、踩过什么坑）写得很实，例如
  `recognize.rs` 的打分原则、`tools_dl.rs` 的 TLS 后端接线原因、`scan.rs` 的性能动机。这比"逐行注释"更有价值。
- 测试密度不错：42 单测 + 15 集成测试，roundtrip 覆盖所有自研格式；每个新引擎/注入分支都有针对性测试。
- 对外部工具一律**委派进程 + 只写文件**，不自造加解密轮子。

### 2.2 缺陷

| 类别 | 具体问题 |
|---|---|
| **崩溃风险** | 二进制解析大量 `data[a..b].try_into().unwrap()`（`xp3.rs` 18、`pck.rs` 8、`rgss.rs` 4、`asar.rs` 3、`marshal.rs`/`pickle.rs` 各 1）。长度校验不一致 → 截断/伪造封包直接 panic。**已复现**：24 字节的假 `.pck` 触发 `pck.rs:30 range end index 32 out of range for slice of length 24`。 |
| **数据丢失风险** | 6 处备份**无条件覆盖**：`others.rs:545(RGSS)、829(KiriKiri)、1049(Godot)、1241(NScripter)、1341(asar)` + `renpy.rs:421(backup_file)`。对比 `inject.rs:288/416` 是正确写法（`if !bak.exists()`）。重复操作两次 → 备份变成"改过的文件"，`restore` 也救不回来。 |
| **静默失败** | 关键路径大量 `let _ = fs::write(...)` / `fs::copy(...)`（至少 20 处），失败不计数。`others.rs:489` 解包时 `let _ = fs::write(safe_out_path(...), blob)` → 实际少写了文件，界面仍报"解包 N 个文件"。 |
| **不可达代码** | `saves.rs:239` 与 `pck.rs:142` 的 `unreachable!()`；后者写作 `if size > u64::MAX { unreachable!() }`，逻辑上永远为假 —— 无效防御。 |
| **死代码** | `Engine::describe` 有 13 个实现但**零调用**；`Registry::capability_matrix`、`Registry::pick` 零调用；`cli.rs:536` 用 `#[allow(dead_code)] fn _t(_: Arc<Mutex<()>>) {}` 糊未用导入；`translate.rs::_unused_read` 同理。 |
| **错误类型** | 全程 `Result<_, String>`，无 `thiserror`/自定义错误枚举 → 调用方无法按错误类型分支，只能做字符串匹配。 |
| **测试盲区** | 无畸形/模糊输入测试（正是第 1 条崩溃能存活至今的原因）；无 GUI 层测试；无并发/取消路径测试。 |

---

## 3. 性能

### 3.1 已改进（本次会话完成）

检测从「8 次全树 `walkdir` + 2 次子树遍历」改为「`ScanCtx` 一次扫描 + 内存查表」，并删掉了每插件对结果的无用 `sort`。
对 2–10 万文件的 Galgame 目录是数量级改善。

### 3.2 仍然存在的问题

> 下表是**施工前**的评估结论，保留原始判断以便对照；**实际处置见 §7.2**（P2-1 ~ P2-12）。
> 最新状态：🟢 已解决 / 🟡 部分解决 / 🔴 仍存在。

| 问题 | 影响 | 位置 | 现状 |
|---|---|---|---|
| 每次操作完成后都 `refresh_detections()`，整目录**重新扫一遍** | 操作 → 卡一下；小目录无感，大目录明显 | `gui/mod.rs` | 🟡 已加 `det_dirty`：解包/反编译/文本提取（只写输出目录）不再重扫；其余写操作仍需重扫（正确性优先） |
| 解析封包**整包读入内存**且无大小上限（7 处 `read_to_end`） | 4GB 级 `.xp3/.pck` → 内存峰值等同文件大小，可能 OOM | `formats/{pck,rpa,asar,xp3}.rs` | 🟢 P2-1：`formats::source::Source` 流式化 + 4GB 上限 |
| 解包**逐文件同步写盘**，且每个文件都调 `create_dir_all` | 万级文件解包慢；目录 syscall 重复 | `engines/others.rs` 各 `extract` | 🟢 P2-2：`parallel_extract` 多线程 + `safe_out_path` 线程本地目录缓存 |
| 文本提取/机翻**完全串行**，机翻无重试与退避 | 几万条文本耗时长；遇 429 直接失败（只能靠断点重跑） | `features/translate.rs`、各 `text_extract` | 🟢 P2-3：`jobs` 批并发 + 429/5xx 指数退避重试 |
| `tools_dl` 下载全量进内存 | AssetRipper 包 ~100MB，峰值内存翻倍 | `features/tools_dl.rs` | 🟢 改为流式写盘（边下边算 SHA-256） |
| 预览 `list_media` 递归遍历无上限 | 超大素材目录下列表构建卡顿 | `features/preview.rs` | 🟢 新增 `MAX_MEDIA` 上限，达到即停止并如实告知被截断 |

> 说明：本表早期版本为**代码路径分析**结论（当时未做基准测试）。标 🟢 的项在 §7.2 有具体验证记录。

---

## 4. 可维护性

> 同 §3.2：下表为**施工前**评估，保留原始判断；🟢 = 已在 §7 处置完毕。

| 问题 | 具体表现 | 现状 |
|---|---|---|
| **UI 单体** | `gui.rs` 2906 行 / 55 个函数；`page_text` 342 行、`page_runtime` 297 行、`update` 283 行、`page_save` 174 行。改一个按钮要在几百行闭包里找上下文。 | 🟢 P2-11：拆为 `gui/mod.rs` + `pages/*.rs`（纯机械搬移，测试数不变） |
| **引擎接口过重** | `Engine` trait 11 个方法，多数默认实现返回 `"未实现"`。新增一个只支持解包的引擎，仍要面对一整套方法。 | 🟡 未改（trait 形态保持不变；新增引擎仍有默认实现兜底，成本可接受） |
| **死接口占用实现** | `describe` 13 个实现全是白维护（见 2.2）。 | 🟢 P1-6 / P2-7：`describe` 接进 GUI「引擎详情」面板，不再是无用实现 |
| **本机路径硬编码** | `settings.rs` 写死 `D:/STool/tools/unrpyc/unrpyc.py` 与 python `3.13.12`；截图/验证脚本写死 `D:/STool/verify/...`。换机器即失效，且失败是"静默回退到 `python`"。 | 🟢 已改为跨机器探测（配置 → 环境变量 → `~/.stool/tools` → skill 副本；Python 扫 `versions/*` 取最新）。`verify/` 脚本属本机验证工具、不入库，可接受 |
| **构建不可移植** | `.cargo/config.toml` 写死 MSVC；已被 `.gitignore` 排除（正确），但 README 没有替代的"如何在新机器构建"说明。 | 🟢 README 新增「从源码构建」章节（含 VC 运行库 PATH / `RUSTC` / `CARGO_INCREMENTAL`），`ARCHITECTURE.md` §6 同步 |
| **无质量门禁** | CI 只 `cargo test --release`；无 `cargo clippy -D warnings`。 | 🟢 P1-3：CI 增加 `clippy -D warnings`；测试改 `cargo test --tests`。fmt 门禁**有意不设**（见 rustfmt.toml 说明） |
| **文档不均** | README 很完整；但缺 `ARCHITECTURE.md`（模块职责与数据流）。 | 🟢 P1-4：新增 `ARCHITECTURE.md` + `引擎识别依据与解锁策略.md` |
| **配置无版本** | `Config` 靠 `#[serde(default)]` 兼容新增字段；字段语义变化时无迁移路径，且 `load()` 出错时**静默回退默认值**。 | 🟢 P1-5：`CONFIG_VERSION` + `migrate()`；解析失败**备份损坏文件 + 明确报错**，不再静默丢设置 |

---

## 5. 安全性

### 5.1 已经做对的

- **备份体系**（`.stool.bak`）是全案的安全底线，方向正确（只是有 2.2 的覆盖 bug）。
- **路径穿越防护**：解包统一走 `engines::safe_out_path()`（过滤 `..`、`/`、绝对路径）；`tools_dl` 用 `zip::enclosed_name()` 防 zip-slip。两个入口都堵住了。
- **不注入、不提权**：所有外部工具（GARbro/AssetRipper/WolfDec/unrpyc）都是独立进程 + 只写文件，不进目标进程。
- **内存扫描的 `unsafe` 有 RAII**：`Scanner` 实现 `Drop` 关句柄，无句柄泄漏；`MAX_REGION=256MB` 限制单次读取。
- **无凭据入库**：`~/.stool/config.json` 在 `USERPROFILE` 下，仓库 `.gitignore` 已排除 `.workbuddy/`、`verify/`、`tools/`。

### 5.2 风险与建议

| 风险 | 说明 | 建议 | 现状 |
|---|---|---|---|
| API Key 明文落盘 | `Config.mtl_key` 明文写入 `~/.stool/config.json`。本机自用风险低，但会被任何同步盘/备份/误分享带走 | Windows 用 DPAPI（`CryptProtectData`）加密该字段 | 🟢 **已做**：DPAPI 加密，落盘形如 `enc:v1:<hex>`；旧明文自动兼容并在下次保存时升级；跨机器解不开时告警 + 清空该字段 |
| 下载无完整性校验 | `tools_dl` 只依赖 GitHub 的 HTTPS，未校验 SHA256，下载后可执行文件直接被写进 `~/.stool/tools` 并在后续被 `Command::new` 执行 | 内置已知版本的 SHA256；或在 UI 显示实际哈希供人工核对 | 🟢 **已做**：流式下载 + 自实现 SHA-256（`src/hash.rs`，零依赖），对照 GitHub Release 资产 `digest` 校验，不一致即中止并删临时文件；无摘要时把校验和记到 `<工具目录>/.sha256` |
| `Cargo.lock` 未入库 | 二进制项目的依赖版本应锁定，否则供应链不可复现 | 从 `.gitignore` 移除并提交 | 🟢 **已做**（P0-8） |
| panic 信息丢弃 | GUI 无控制台，`std::panic` 输出无处可见 | panic hook + 工作线程 `catch_unwind` | 🟢 **已做**（P0-4，`diag.rs`） |
| 内存写入无显式警告 | `memscan` 会**写**目标进程内存，文档/UI 未强调风险 | 首次写内存前弹二次确认 | 🟢 **已做**：首次写入/锁定前弹确认窗（`MsPending`），确认后可勾选免打扰 |
| 日志无脱敏 | 日志若落盘，可能含绝对路径与翻译内容 | 落盘时过滤 `mtl_key` | 🟢 **已做**（P1-1，诊断包导出时 `mask()` 脱敏） |

---

## 6. 尚未实现、但值得进一步开发的功能或改进方向

> 排序依据：**① 不崩不丢数据 → ② 出错能查 → ③ 降低新增功能的摩擦 → ④ 体验增量**。
> 工作量：S ≈ 半天，M ≈ 1–2 天，L ≈ 3 天以上（AI 施工口径）。

### P0｜健壮性与数据安全（本次立即施工）

| 项 | 内容 | 状态 |
|---|---|---|
| P0-1 | **备份不可覆盖**：统一 `backup_once()`，备份已存在绝不覆盖（修 6 处） | ✅ |
| P0-2 | **解析器不 panic**：新增 `formats::safe` 边界安全读取原语，逐步替换裸切片/`unwrap` | ✅ |
| P0-3 | **畸形输入回归测试**：对每个解析入口喂截断/随机字节，断言"返回 Err 且不 panic" | ✅ |
| P0-4 | **panic 可诊断**：panic hook + 工作线程 `catch_unwind`，界面显示"内部错误（已拦截）+ 位置" | ✅ |
| P0-5 | **日志落盘**：`~/.stool/logs/stool-YYYYMMDD.log` + 启动横幅（版本/路径/配置摘要） | ✅ |
| P0-6 | **CLI 参数校验**：缺参数输出用法 + 退出码 2，不再 panic | ✅ |
| P0-7 | **静默失败上报**：写盘失败计数并写入结果消息（"成功 N，失败 M"） | ✅ |
| P0-8 | **`Cargo.lock` 入库**（`.gitignore` 已移除排除项；本项目为二进制应用） | ✅ |

### P1｜可运维性与质量门禁

| 项 | 内容 | 状态 |
|---|---|---|
| P1-1 | **一键导出诊断包**：日志 + 引擎检测结果 + 环境（OS/架构/配置摘要，脱敏）+ 失败文件清单 → 单个 zip | ✅ |
| P1-2 | **环境预检前置**（对应原方案 A2）：写权限 / 文件占用 / 磁盘空间，失败给"原因 + 修法" | ✅ |
| P1-3 | **CI 加门禁**：`cargo clippy -D warnings` + `cargo test --tests`（fmt 门禁见下方说明） | ✅ |
| P1-4 | **`ARCHITECTURE.md`**：模块职责 + 数据流 + "新增一个引擎要改哪几个文件"清单 | ✅ |
| P1-5 | **配置版本化**：加 `config_version` 与迁移函数；`load()` 解析失败时**备份损坏文件 + 明确报错**，不再静默回退 | ✅ |
| P1-6 | **清理死代码**：删 `describe` 13 个实现 / `capability_matrix` / `pick` / `_t` / `_unused_read`（或把 `describe` 接进 UI 的引擎详情区） | ✅ |

### P2｜性能与能力扩展

| 项 | 内容 | 工作量 |
|---|---|---|
| P2-1 | **解析器流式化 + 大小上限**：超阈值走 `File::seek` 按需读，而不是整包进内存 | ✅ |
| P2-2 | **解包并行化**：多线程写盘 + 目录缓存（`safe_out_path` 只建一次父目录） | ✅ |
| P2-3 | **机翻并发 + 重试退避**：限流下也稳（429/5xx 指数退避），配合已有断点续翻 | ✅ |
| P2-4 | **解包断点续传**：记录已完成条目，中断后可继续 | ✅ |
| P2-5 | **回填一键闭环**：重封包后原子替换原档案（`backup_once` 保底）+ 双档案自动登记 | ✅ |
| P2-6 | **封包自检命令**：解包 → 重打包 → 逐条目校验，输出差异报告（把 `verify/*_diff.txt` 的人工流程产品化） | ✅ |
| P2-7 | **引擎详情区**：把已算出的 `Confidence` / 证据 / 建议渲染成"这个引擎能做什么 + 下一步"面板（顺便复活 `describe`） | ✅ |
| P2-8 | **D1 游戏体检卡片**：非 Unicode 区域设置 / RTP 缺失 / 日文字体 / 常见 DLL | ✅ |
| P2-9 | **批量队列**：多游戏目录排队处理 | ✅ |
| P2-10 | **MOD 冲突校验**：同一文件被多个 MOD 覆盖时告警 | ✅ |
| P2-11 | **UI 拆分**：`gui.rs` 按页拆成 `gui/pages/*.rs`，为后续功能腾出可读空间 | ✅ |
| P2-12 | **C2 运行时注入**：定案为**文件级**（不进目标进程），KiriKiri 系走「patchN.xp3 运行时补丁包」，其余引擎按替代路线收敛 | ✅ |

### 明确不做（沿用原方案边界）

- 不做云服务/账号体系；机翻只走自有 key 或本地模型。
- 不做非法解密（保护类加密维持明确报错）。
- 不引入 GUI 框架替换（egui 够用，重写 UI 成本远大于收益）。

---

## 7. 本次立即施工的内容（P0 全项，已完工）

> 施工原则：**先不崩不丢数据，再出错能查，最后才是新功能**。下表为实际改动与验证方式。

| 项 | 状态 | 改了什么 | 验证方式 |
|---|---|---|---|
| P0-1 备份不可覆盖 | ✅ | `settings.rs` 新增 `backup_path_for / backup_once / backup_or_abort`；`others.rs`（RGSS、KiriKiri、Godot、NScripter、asar 共 5 处）、`renpy.rs`(1 处)、`inject.rs`(4 处) 全部改用；`backup_once` 备份已存在即返回，**绝不覆盖** | 新增单测 `settings::tests::backup_once_never_overwrites`（二次备份后校验备份仍是原始内容）；真实目录 `bs2/lili` 的 `.stool.bak` 均保持首次内容 |
| P0-2 解析器不 panic | ✅ | 新增 `formats/safe.rs`（`u8_at/u16_le/u32_le/u64_le/arr/slice/Cursor`，越界一律 `None`）；修 `pck.rs` 头偏移越界、无守卫的 v2 flags 读取；修 `xp3.rs` **`data[11]`**（MAGIC 仅 11 字节时越界）；`pck/asar/xp3/rgss` 的 `read_file/extract_*` 用**饱和加法**消除整数溢出 | `tests/fuzz_parsers.rs` 用确定性 PRNG 构造截断/随机/逐字节翻转样本，全部断言"不 panic"；曾精确复现 `pck::read_file(offset=u64::MAX)` 溢出并被修复 |
| P0-3 畸形输入回归测试 | ✅ | 新增 `tests/fuzz_parsers.rs`（9 个测试：pck/xp3/asar/marshal/pickle/rpgmmv/nscript 切片入口 + 文件入口 + 合法样本变异） | `cargo test --tests` 全绿 |
| P0-4 panic 可诊断 | ✅ | 新增 `diag.rs`：全局 panic hook（记录时间/线程/`file:line:col`/原因）；`diag::guard()` 在后台任务线程用 `catch_unwind` 把 panic 折成**可展示错误**；`gui.rs` worker 与 `poll_worker` 接入 | 单测 `diag::tests::guard_converts_panic`；GUI 失败信息落盘 |
| P0-5 日志落盘 | ✅ | `~/.stool/logs/stool-YYYY-MM-DD.log`，`diag::init()` 在 `main`/`cli_main` 最早调用；无第三方依赖的本地时间格式化（Howard Hinnant 算法） | 单测 `log_writes_line`；实机确认日志文件已生成并含 panic/失败记录 |
| P0-6 CLI 参数校验 | ✅ | `cli.rs` 新增 `missing_arg_usage()`，对 detect/extract/repack/restore/mod-*/archive-toggle 等统一校验缺参，打印用法并返回退出码 2 | `stool-cli detect`、`mod-uninstall x`、`restore`、`pack-import x` 均优雅退出（原 `cli.rs:265` panic 已消除） |
| P0-7 静默失败上报 | ✅ | `engines::write_out()` + `with_fail_note()`：解包/解密/回填循环写盘失败**计数并写入结果消息**（"另有 N 个文件写入失败，详见日志"）；覆盖 RGSS/KiriKiri/Godot/asar/NScripter/Ren'Py/MV 回填 7 类循环 | 真实 14MB XP3 解包 189 文件、零失败提示；失败路径经日志验证 |
| P0-8 `Cargo.lock` 入库 | ✅ | `.gitignore` 移除 `Cargo.lock` 排除项（本项目含 `[[bin]]`，为二进制应用，按官方建议入库以固化依赖版本） | `git check-ignore` 已不再命中；下次提交即入库 |
| 附加：Clippy 清零 | ✅ | `cargo clippy --lib` 从 **31 → 0 警告**（自动修复 21 项 + 手改 10 项：类型别名 `DlResult/DetResult/PvListResult`、`find`、`clamp`、`enumerate`、文档缩进等） | `cargo clippy --lib` 无输出 |

**验证汇总**：`cargo test --tests` → **73 通过 / 0 失败**（49 单元 + 9 模糊 + 15 回环）；`cargo build --release` 零警告；真实目录回归 `bs2`=130（RGSS）、`lili`=95（Godot）、`komoguri_out`=70（KiriKiri）均与改造前一致，无功能回退。

**下一步（P1，待用户确认后施工）**：诊断包一键导出、CI 质量门禁（`fmt --check` + `clippy -D warnings`）、配置版本化与损坏自愈、死代码清理（`describe`/`capability_matrix`/`pick`）。

---

## 7.1 P1 施工结果（已完成）

| 项 | 状态 | 改了什么 | 验证方式 |
|---|---|---|---|
| P1-1 诊断包导出 | ✅ | 新增 `features/diagpack.rs`：单个 zip 打包 `env.txt`（api_key 脱敏）/ `logs/` / `warnings.txt`（WARN/ERROR/PANIC）/ `detect.txt` / `backups.txt`；CLI `diag-export` + GUI 日志页按钮 | 单测 `mask`/`env_report`/`export_writes_zip`/`backups_report`；真实游戏目录导出 zip 条目有效 |
| P1-2 环境预检前置 | ✅ | 新增 `features/precheck.rs`：`Level(Ok/Warn/Fail)` + `Item` + `Report`；探测式检查 **目录可写**（真实建删临时文件）/ **文件占用**（带写权限 open，只认共享冲突 32/33）/ **磁盘空间**（`GetDiskFreeSpaceExW`）；失败给「原因 + 修法」，`Warn` 不阻塞 | 10 个单测；CLI `precheck` 命令；实机验证：独占占用 `Game.exe` 时正确报「1 个文件疑似被占用」并给修法，释放后告警消失；GUI `spawn()` 按操作类型自动选范围（`scope_for_op`） |
| P1-3 CI 门禁 | ✅ | `.github/workflows/ci.yml` 门禁改为 `cargo clippy --all-targets -- -D warnings` + `cargo test --tests`；**不设 `cargo fmt --check`**（见下） | 门禁实际抓到并修掉 1 处真 lint（`unlock.rs` needless_borrow） |
| P1-4 ARCHITECTURE.md | ✅ | 新增 `docs/ARCHITECTURE.md`：模块职责、目录树、数据流、"改哪里"速查、构建命令、数据/安全边界 | 人工核对 |
| P1-5 配置版本化 | ✅ | `settings.rs` 增 `CONFIG_VERSION=1` + `migrate()` + `parse()`；`load()` 解析失败改为**备份损坏文件 + 告警**（不再静默回退默认值）；全字段补 `#[serde(default)]` | 单测 4 个（legacy 无版本迁移 / 空对象用默认 / roundtrip 保版本 / 解析错误上报） |
| P1-6 死代码复活 | ✅ | `describe`→GUI 「引擎详情」面板；`capability_matrix`→CLI `engines`；`pick`→`pick_from` | `engines` 命令输出 29 插件能力矩阵 |

**关于 P1-3 未采用 `fmt --check` 门禁的说明**：本仓历史代码为**手工维护的宽行风格**（非 rustfmt 默认 100 列），实测全量 `cargo fmt` 会改动 **约 2023 行 / 37 个文件**，与「保持既有代码风格一致」冲突。故新增 `stool-rs/rustfmt.toml`（`max_width=120` + `use_small_heuristics="Max"`）作为风格指引但**不作门禁**。若日后决定统一格式化一次，可再恢复该门禁。

**验证汇总（P1 完成后）**：`cargo test --tests` → **117 通过 / 0 失败**（93 单元 + 9 模糊 + 15 回环，其中 1 个联网用例 ignored）；`cargo clippy --all-targets -- -D warnings` 零警告；`cargo build --release` 成功。

---

## 7.2 P2 施工结果（进行中）

> 排序原则：先做「防丢数据 / 直接省事」的 S 项，再做性能与 UI 拆分。

| 项 | 状态 | 改了什么 | 验证方式 |
|---|---|---|---|
| P2-5 回填一键闭环 | ✅ | `features/restore.rs` 新增 `apply_repack(target, new_archive)`：用重打包产物替换原封包，覆盖前 `backup_or_abort` 留底（**已存在则不覆盖**）；该备份恰好是双档案切换所需的 `.stool.bak`，替换后即可 `archive-toggle`。CLI 新增 `pack-apply <原封包> <新封包>` | 单测 3 个（替换+留底+切换闭环 / 备份绝不被后续覆盖 / 同文件与不支持类型被拒）；实机验证 `pack-apply` → 内容替换、备份为原版、`archive-toggle` 双向翻转正常 |
| P2-10 MOD 冲突校验 | ✅ | `features/mods.rs` 新增 `Conflict{rel, mods}` + `conflicts()`（已启用 MOD 间）+ `would_conflict()`（预演）；`install_mod` 增 `allow_conflict` 参数，默认**拒绝**与已启用 MOD 争用同一文件并列出冲突。CLI：`mod-install --force`、新命令 `mod-conflicts`，`mod-list` 追加冲突段；GUI：MOD 页新增「允许冲突覆盖」勾选 + 冲突高亮列表 | 单测 4 个（无冲突安装 / 冲突被拒后强制安装 / 停用后冲突消失 / 不同文件不冲突）；实测 A、B 两 MOD 争用 `Data/common.txt` → 装 B 被拒（附冲突明细）→ `--force` 后 `mod-conflicts` 报 1 处冲突、退出码 1 |
| P2-7 引擎详情区 | ✅ | 复用 P1-6 复活的 `Engine::describe`：后台检测线程为选定引擎算好详情，GUI 资源页「引擎详情」面板展示"识别依据 / 能力 / 建议"；`Registry::effective_capabilities` 同步进能力矩阵 | `engines` 命令输出 29 插件能力矩阵；GUI `engine_detail` 渲染（`gui.rs:991`） |
| P2-9 批量队列 | ✅ | 新增 `features/batch.rs`：`discover()`（深度受限扫描候选游戏目录，命中即止、跳过工具/系统目录）+ `detect_batch()` / `run_batch()`（逐条记结果，单条失败不影响整批；写操作复用 P1-2 预检）。CLI `batch <根目录> [--depth N] [--op ...] [-o] [--csv]`；GUI 首页「📦 批量检测该目录下的多个游戏」按钮（后台线程 + 进度条） | 单测 6 个（发现/命中即止/工具目录跳过/CSV 转义/汇总/未确认识别）；实测 `verify/` 扫描 → 7 个游戏、引擎分布正确、CSV 写出；`--op unlock` 批量只读预览正常 |
| P2-8 D1 游戏体检 | ✅ | 新增 `features/health.rs`（复用 precheck 的 `Item/Level/Report`）：主程序 / 路径字符与长度 / 系统区域 ACP（注册表 `Nls\CodePage`）/ 日文字体（MS Gothic·Meiryo·Yu Gothic）/ 运行库 DLL（RGSS·Unity，按结构特征判断）/ 写权限。CLI `doctor <目录>`；GUI 首页「🩺 游戏体检」按钮 | 单测 6 个（缺目录 Fail / 无 exe 告警 / RGSS 缺 DLL 告警且补 DLL 后消失 / Unity 缺 UnityPlayer / ACP 判定）；实测 `bs2` 正确报「缺 RGSS*.dll」、`komoguri` 正确报「根目录无 .exe」 |
| P2-6 封包自检 | ✅ | 新增 `features/selfcheck.rs`：`sniff`（魔数优先、回退扩展名）分派 XP3/PCK/asar/RPA/RGSSAD v1·v3；**解包 → 重打包 → 回读 → 逐条目比对**（大小 + FNV-1a 64），比对的是**条目内容**而非封包字节；名字按分隔符归一（`\`≡`/`）避免 RGSS 写出差异误报；工作目录磁盘中转且**成功/失败都清理**；RPA 需整包在内存（写 API 为内存式）>512MB 跳过往返比对、xp3/pck/asar/rgss >2GB 直接拒绝。复用 precheck 的 `Item/Level/Report` 输出。CLI `selfcheck <目录\|封包>`；GUI 首页「🧩 封包自检」 | 单测 11 个（5 格式往返无损 + 篡改可检出 + 遍历/重名防护 + 目录汇总 + 无包告警 + 报错不留残留）；**真实封包实测**：`tail_test/unencrypted.xp3` 189 条目、`lili/Captive_Lili.pck` 8166 条目/374.5MB、`bs2/Game.rgss3a` 2591 条目/508.9MB 均逐一无损；OPPAI `Hxv4` 保护变体正确拒绝（与解包器一致）。**顺带抓到并修复 1 个真 bug**：`asar::pack` 头表偏移顺序（先叶子后目录）与数据写入顺序（全局字典序）不一致 → 含「顶层文件 + 子目录」的 asar 会内容错位 |

| P2-4 解包断点续传 | ✅ | `engines/mod.rs` 新增 `Resume`（台账 `<out>/.stool_resume_extract.json`，记「条目名 → 大小」）+ `write_out_resume` + `with_resume_note`：命中即跳过（连解压/读盘都省），每 64 条自动落盘，取消与解析失败等提前返回路径都会 `flush()`；`finish(complete)` 成功删台账、未完成保留。跳过前**必须**校验产物仍在且大小一致；`root` 写进台账，换目录即作废；`--opt:resume=0` 关闭。接入 RGSS / KiriKiri / Godot PCK / asar / Ren'Py 五处解包循环 | 单测 4 个（跳过+成功删台账 / 换 root 作废 / 大小不符必须重做 / 关闭开关不跳过）；**真机中断实测**：对 509MB `bs2/Game.rgss3a` 解包 7 秒后强杀 → 落盘 2067 文件、台账 2048 条；重跑输出「解包 2591 个文件（其中 2048 个沿用上次结果，断点续传）」、最终 2591 文件、台账自动清理 |

| P2-1 解析器流式化 | ✅ | 新增 `formats/source.rs`：`Source` 双模式（`Mem(Vec<u8>)` / `File{File,PathBuf,u64}`），`open()` 按 `MEM_THRESHOLD=128MB` 自动选路，`open_with()` 供测试强制模式；`read_at/read_exact_at/read_tail` 支持 seek 定向读，`MAX_ARCHIVE=4GB` 硬上限。XP3 / PCK / asar / RGSS v1·v3 各增 `parse_index(&mut Source)` + `read_entry(&mut Source, …)`（按偏移按需读，替代整包 `read_to_end`）；RGSS 用 `parse_index_v1/v3` + `read_entry_v1/v3`。五处解包循环（RGSS / KiriKiri / Godot PCK / asar / Ren'Py）与 `selfcheck::extract_all` 全部改走 `Source` | 新增 `tests/streaming.rs` 5 个等价性测试：每格式写合成封包，分别用**内存模式**与**强制文件模式**跑「索引解析 + 逐条目读取」，要求与原始内容**逐字节一致**；另含超大拒绝。`cargo test --tests` → **160 通过 / 0 失败**（131 单元 + 9 模糊 + 15 回环 + 5 流式，1 个联网用例 ignored）；`cargo clippy --all-targets -- -D warnings` 零警告 |
| P2-2 解包并行化 | ✅ | `engines/mod.rs` 新增 `Job<T>` + `parallel_extract`：`std::thread::scope` 开 N 个 worker（默认 `available_parallelism`，`--opt:jobs=N` 可调），各自 `mk_reader()` 建一份读取句柄，按**原子游标**抢条目（work-stealing、无锁）；结果经 mpsc 回**主线程**统一报进度、登记断点台账、计失败（`Resume`/`Ctx` 非 `Sync`，不跨线程）。取消：主线程见 `ctx.cancelled()` 即置 `abort`，worker 在条目边界退出。`safe_out_path` 增**线程本地目录缓存**（同一目录只 `create_dir_all` 一次）+ `clear_dir_cache()`（cli/gui 每次操作前清，避免删目录后缓存失真）。接入 RGSS v1·v3 / KiriKiri xp3 / Godot pck / asar / Ren'Py rpa 五处；`rpa::Chunk` 类型别名顺带消除复杂类型 | 单测 4 个（200 条全写出+台账可恢复 / 读失败计数 / 单线程与 8 线程结果一致 / 目录缓存只建一次且清缓存后恢复）；新增 `tests/parallel.rs` 4 个端到端测试（xp3 300 条、pck 250 条、rgss3a 400 条、rgssad v1 120 条：真实跑 `Engine::extract` 的 jobs=1 vs jobs=8，产物**逐文件逐字节一致**、成功后台账清空）；`cargo clippy --all-targets -- -D warnings` 零警告 |

| P2-3 机翻并发 + 重试退避 | ✅ | `features/translate.rs`：`OpenAiCompat` 错误串携带 HTTP 状态码（`HTTP 429：…` / `网络错误: …`）；新增 `looks_retryable`（408/425/429/500/502/503/504 + 网络关键词 → 可重试，400/401/404 立即失败）、`sleep_cancellable`（分片睡可取消）、`retry_batch`（指数退避 1s/2s/4s…上限 30s，`RETRY_ATTEMPTS=4`）、`run_batches`（`thread::scope` + 原子游标抢批次 + mpsc 回主线程回调；**已成功批次照常回填**，不浪费请求）。CSV/JSON 两条管线改为「分批并发 → 每批完成即写盘+存断点」。并发度：settings `mtl_jobs`(默认 4) / CLI `--jobs` / GUI「并发」输入框 | 单测 +6（重试分类 / 退避后成功 / 客户端错只发一次且不退避 / 取消在退避中立刻生效 / 多 worker 真并发不重不漏 / 有失败时仍回填成功批次）；**端到端真机验证**（本机假机翻服务 `scripts/mock_mtl_server.py`，前 3 个请求返回 429）：60 条 / batch=5 / jobs=4 → `✔ 已机翻 60/60 条`，1.89s；服务端计数 `seen=15`（=12 批 + 3 次重试）、`max_inflight=4`（确为 4 并发）；CSV 逐行译文正确、断点文件已清理 |

| P2-11 `gui.rs` 拆分 | ✅ | `src/gui.rs`(3250 行) 拆成 `src/gui/mod.rs`(997 行) + `util.rs`(通用小工具：页头/路径转义/JSON 值显示解析) + `save_tree.rs`(存档结构树只读渲染) + `pages/{home,extract,preview,text,save,runtime,mods,settings,help,log}.rs`(各页 `impl StoolApp`)。做法：按方法边界**精确搬移**（不改一行逻辑），子模块用 `use crate::gui::*;` 取父模块项；父模块把 `StoolApp/Page/Shared/MsStatus/AudioOut` 与 `egui/engines/features` 依赖提升为 `pub(crate)` 供子模块复用；`RenderBudget` 字段与 `exec_op` 提升可见性 | 纯机械搬移：`cargo clippy --all-targets -- -D warnings` 零警告、`cargo test --tests` **174 通过 / 0 失败**（与拆分前完全一致）；GUI 可正常启动、各页渲染路径未变 |

**验证汇总（P2 本轮完成后）**：`cargo test --tests` → **189 通过 / 0 失败**（156 单元 + 9 模糊 + 15 回环 + 5 流式 + 4 并行，1 个联网用例 ignored）；`cargo clippy --all-targets -- -D warnings` 零警告；GUI 启动无 panic。

### P2-12 专项说明（定案：文件级，不做进程注入）

| 项 | 状态 | 改了什么 | 验证方式 |
|---|---|---|---|
| ✖ 顺带修掉一个真 bug：**XP3 写出格式不合规范** | ✅ | `formats/xp3.rs` 老写法是「`magic + u8 0x00 + u32 索引偏移`，TOC 用 `[u32 条目数]` 前缀」——**与真实 KiriKiri 封包完全不同**，游戏很可能加载不了；`info.flags` 还错写成 `1`（bit31 = 内容经引擎加密）。改为 GARbro/KiriKiri 规范布局：①头部两代可选（`Xp3Version::V1` = `magic + u64 索引偏移`；`V2` = `magic + u64 0x17 + u32 1 + u8 0x80 + u64 保留 + u64 索引偏移`，数据区起点 40）；②TOC = `[u8 标记][u64 压缩长][u64 原始长][zlib]` + 无计数前缀的 `File` 条目流（子块长度字段全为 **u64**）；③`info.flags` 一律 0（明文）；④`segm` 字段序 `[u32 是否压缩][u64 偏移][u64 原始][u64 归档]`；⑤`adlr` = 明文内容的 Adler-32（实测真实封包一致）。新增 `version_of / Xp3Version / encrypted_count / write_paths_v`。**回写 data.xp3 时沿用原封包的头部代次**（V1 老游戏套 V2 头可能加载不了） | 新增 9 个单测（V2/V1 头部布局 + 数据区起点 + 两代往返无损且条目为明文 + 流式/内存读一致 + Adler-32 已知值 + **逐字段核对 TOC 原始字节**）；**第三方规范交叉验证**：另写独立 Python 解析器（严格按 GARbro 规范，与 Rust 代码无共享）解析 STool 写出的封包 → 版本识别 v2、5/5 条目 `flags=0`、`cid`/大小/`adlr` 全对 |
| ✖ 顺带修掉一个真 bug：**加密封包静默产出乱码** | ✅ | `engines/others.rs` 的 KiriKiri 解包原先无条件把条目内容写盘；但真机 `komoguri` 的 3 个封包是**引擎加密**（Cx 类方案，按游戏定制密钥），读出来是随机字节 → 用户会拿到 1600+ 个乱码文件还以为是成功。改为：`xp3::encrypted_count` 检测到加密封包即跳过，全部加密时**直接失败**并给「原因 + 替代路线」（GARbro / KrrkExtract 导出明文后再用本工具）；`selfcheck` 对加密封包也明确跳过（避免"乱码==乱码"得出假的「无损」结论） | **真机验证**：`stool-cli extract verify/komoguri` → 0.15s 明确报错「data.xp3（1243/1244 加密）、patch.xp3（325/326）、patch2.xp3（28/29）」且**不产生任何产物**（原先会写 ~1600 个乱码文件）；`xp3-patch --list` 同样正确标出三个封包的条目数与「内容加密」 |
| **P2-12 KiriKiri 运行时补丁包** | ✅ | 新增 `features/xp3patch.rs`：把改动文件（保持封包内原始相对路径）打进**下一个空号**的 `patchN.xp3`。依据是 KiriKiri 官方搜索顺序 `Data 目录 → data.xp3 → patch.xp3 → patch2.xp3 → …`（**后者覆盖前者**），所以引擎会自动优先加载补丁包，几 GB 的 `data.xp3` 一个字节都不用动、删掉补丁包即完全还原——社区汉化补丁的通行做法。安全设计：**绝不用来覆盖游戏自带的 patch**（`build` 检测到非本工具创建的包直接拒绝；`remove` 靠 sidecar `<name>.stool.json` 认定归属，非本工具创建的一律拒删）；包名强校验 `patch*.xp3`（挡掉把 `data.xp3` 当补丁写的误操作）；重建自己的包会先 `backup_once`。CLI `xp3-patch <目录> [--list / --src <目录> [--name] / --remove <名>]`；GUI「运行时修改」页新增「方式三：封包类引擎的补丁包」区块（改动目录选择、自动取号、生成/列举/移除）；`inject-support` 追加补丁包能力表 | 单测 8 个（取号跳过已占号 / 生成可加载补丁且标记归属 / 拒绝覆盖他人补丁+自动改取号 / 拒绝非 patch 名与空目录 / 只删自己的包 / 重建留备份且内容更新 / 列表顺序与归属 / 能力表）；**CLI 端到端实测**：造 `patch_src/` → 生成 `patch.xp3`（263 B / 2 条目）→ 第三方 Python 解析器确认 v2 头 + `flags=0` + 路径与内容正确 → `--remove` 删除成功、重复移除报「找不到」、把 `patch.xp3` 换成「他人的」后移除被拒；**真机**：`komoguri`（已有 patch.xp3 + patch2.xp3）自动取号 `patch3.xp3` ✓ |

**RGSS / Wolf 的替代路线（定案）**

- **RGSS（XP/VX/ACE）**：不做运行时注入。已有能力已覆盖需求：封包回写（解包 → 改 `Data/*.rvdata` → `Op::Repack`）、存档改写（`features/saves.rs`）、全 CG/回想解锁（`features/unlock.rs`）、原版/汉化双档案切换（`archive-toggle`）。注：`Scripts.rvdata` 只有反序列化没有序列化，脚本级改写请走存档/数据文件路线。
- **Wolf RPG**：**明确不做**——手上没有可验证样本，且其数据文件就在 `Data/` 目录里（未打包），直接用文本/数据改写即可，不需要额外注入层。
- **其他封包型引擎**（Siglus / BGI / Majesty 等）：沿用 `SUPPORT_TABLE` 里已写明的替代路线（解包 → 文本 → 回填 / 封包回写）。

---

**P2 待办：无（P2 全部完成）。**

---

## 8. 功能增量：Unity「全 CG 解锁」（本次实施）

定位：把《game-unpacker / unity-godot.md》里沉淀的 **PlayerPrefs 注册表路线** 产品化。
只解锁 Unity 作品的画廊/CG 回廊，**不改动任何游戏文件**，因此 Mono / IL2CPP 通用、无完整性校验风险。

| 模块 | 内容 |
|---|---|
| `src/features/gallery.rs`（新） | djb2 变体哈希（`<键名>_h<哈希>`）、`app.info` → company/product、Win32 注册表读写（`windows-sys`，无第三方 crate）、场景文件字符串候选提取、哈希自检、备份/还原 |
| `src/engines/external.rs` | `UnityPlugin` 的 `capabilities` 增加 `Op::Unlock` 并实现 `unlock()` |
| `src/gui.rs` | 资源页新增「全 CG 解锁」面板：**默认只读扫描**，勾选后才写注册表；可填键名过滤 |
| `Cargo.toml` | `windows-sys` 增加 `Win32_System_Registry` / `Win32_Security` feature |

**安全设计（这是本功能能用的前提）**
1. **默认只读扫描**：先报告候选键名 + 哈希自检结果，`--opt:apply=1` 才写入。
2. **哈希自检**：用注册表里现存键反推哈希，不一致就**拒绝写入**（防哈希实现漂移）。
3. **类型守卫**：已存在但非 `REG_DWORD` 的键跳过（那是游戏的字符串/字节设置，改成 DWORD 会破坏它）。
4. **裸值守卫**：注册表里存在同名**裸值**（无 `_h` 后缀）说明它不是 pref，不为它新建。
5. **写前备份 / 写后校验**：整个键快照为 JSON（含类型），写后逐项读回；`--opt:restore=` 可回滚，并如实列出备份后新增、不会自动删除的键。

**验证**：`"Orc Kabe"` 哈希与 skill 实测样本一致（`3173159926`）；合成 Unity 目录 + 隔离注册表键（`HKCU\Software\StoolTest\...`，验证后已删除）跑通「扫描 → 写入 3 项 / 校验 3 项 → 回滚」全链路；新增 8 个单元测试 + 1 个 `#[ignore]` 的真实注册表回归测试（自建自删）。测试总数 **81 通过 / 0 失败**，`clippy --all-targets` 零警告。

**已知局限**：候选键名曾靠「场景字符串 + 启发式」提取，可能混入无关整数设置项。缓解手段是「用现存键族前缀自动收敛候选」+ `--opt:filter=` + 默认只读扫描；**最稳的仍是优先找游戏自带的「全开开关」**（已在报告里提示用户）。

**已解决（后续落地）**：原计划的「接 ilspycmd 反编译 `*GalleryManager*` 取 `_wholeNameList`」经实测**不需要**——
`_wholeNameList` 这类字面量本身就存在元数据字符串堆里，直接读即可，还省掉了外部工具与 .NET 运行时依赖：

- Mono 版：`*_Data/Managed/Assembly-CSharp*.dll` 的 `#US` 堆（`formats/dotnet.rs`，PE → CLI 头 → `BSJB` 元数据根 → 流头）；
- IL2CPP 版（**无 C# 程序集**）：`il2cpp_data/Metadata/global-metadata.dat` 的字面量池（`formats/il2cpp.rs`，v24..31，带布局自检）。

真机效果：Inari（Mono）读出 10085 条字面量 / 164 个画廊类型；Yakuzarogue（IL2CPP v31）直接列出真实键
`cg_button_name1`…`cg_button_name7`。噪声用字符集白名单 + base64 常量块剔除 + 相关性排序（先排序再截断）压制，
新增只读入口 `stool cg-candidates`（完全不碰注册表）。**不做 ilspycmd**：它拿到的信息不超出元数据字符串，
却引入一个需要 .NET 运行时的可选依赖（详见 `docs/引擎识别依据与解锁策略.md` §2.4）。



---

## 9. 本轮增量：Artemis 原生支持 + Unity 精确键名 + CI 升级

| 项 | 内容 | 验证 |
|---|---|---|
| **CI** | `actions/checkout@v4` → `@v5`（v4 指向 Node 20，GitHub 已弃用告警） | CI run 全绿 |
| **新引擎：Artemis Engine** | `formats/pfs.rs`（pf6/pf8 索引 + 写侧）+ `engines/artemis.rs`（识别 70 分、解包并行 + 断点续传、按原索引顺序回封、分卷明确报错）+ `selfcheck` 纳入 PFS | 真机：`root.pfs` 1.5GB/2703 条目、`Amakano3.pfs` 1.7GB/3361 条目，索引 100% 解析零越界，产物 PNG/OTF/OGV/`.ast` 全部有效；`selfcheck` 重打包逐条目无损 |
| **Unity 全 CG：键名精确化** | `formats/dotnet.rs`（.NET/PE 元数据 `#US`/`#Strings`）+ `formats/il2cpp.rs`（`global-metadata.dat`）+ `gallery::scan_precise` 两级来源 | Mono 版 Inari：10085 条字面量 / 164 个画廊类型，与**独立 Python 解析器**计数完全一致；IL2CPP 版 6 款游戏 6/6 正确解析，字面量 UTF-8 可解率 99.99% |
| **只读入口 `cg-candidates`** | 完全不碰注册表的候选查看命令（写入前确认 / 排查"一个键都没扫到"） | 真机 Yakuzarogue 扫出真实键 `cg_button_name1`…`cg_button_name7` |
| **封包自检扩容** | `selfcheck::Kind` 增加 `Pfs8` / `Pfs6` | 12 项 selfcheck 单测全过 |
| **对抗性测试** | dotnet / il2cpp 加入逐字节翻转 + 随机改写（各 500 轮）+ 逐长度截断；`tests/fuzz_parsers.rs` 加头级样本 | 无 panic |

**过程中发现并修掉的 3 个真 bug**（真机样本 + 交叉验证才暴露的）：

1. `.NET` 元数据根里 `Flags(u16)` 在 `Streams(u16)` **之前**，解析器把 `Flags`（恒 0）当成了流数量 → 误报"流数量异常（0）"。
2. 流头里的 `Offset` 是**相对元数据根**的，解析器当成了绝对文件偏移 → 所有堆读取越界。
3. IL2CPP 的 `len` 是**精确字节长度**（池内无 NUL 终止符），按"含结尾 NUL"切 `len-1` 会让**每条字面量都被截掉末字符**（`" - Linux"` → `" - Linu"`）。两种解读都能 97% 解出合法 UTF-8，只有真机样本能判定；现已加**布局自检**（字面量 UTF-8 可解率 ≥90%）把错误解读挡在门外。
   另修一个**偶发测试竞态**：`pfs.rs` 的 `sample()` 被两个用例共用同一临时目录，`tmp()` 的先删后建在并行测试下互相踩踏。

**门禁**：`cargo clippy --all-targets -- -D warnings` 零警告；`cargo test --tests` **228 项全过**
（193 单元 + 9 模糊 + 4 并行 + 16 回环 + 6 流式，1 项需真实注册表的用例默认 `#[ignore]`）。
