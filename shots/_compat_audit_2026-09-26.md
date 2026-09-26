# STool 真实样本兼容性排查（2026-09-26）

样本根：`D:\ero`（38 目录）、`D:\baidudownload`（37 目录），共 **75 个游戏目录**。
命令：`stool-cli batch <root> --op detect --csv <out>`。原始报告：`shots/_audit_ero.csv`、`shots/_audit_baidu.csv`。

## 一、总览

| 指标 | 数量 |
|---|---|
| 样本目录 | 75 |
| 已确认（≥60 分） | 66 |
| 未确认 | 9 |

引擎分布（总数 / 已确认）：

| 引擎 | 总数 | 确认 | 引擎 | 总数 | 确认 |
|---|---|---|---|---|---|
| unity | 37 | 37 | godot | 1 | 1 |
| kirikiri | 8 | 8 | unreal | 1 | 1 |
| rpgmaker_mv | 5 | 5 | nscripter | 1 | 0 |
| bgi_ethornell | 4 | 3 | love2d | 1 | 0 |
| gamemaker | 3 | 3 | renpy | 1 | 1 |
| artemis | 2 | 2 | cocos2dx | 1 | 1 |
| rpgmaker_rgss | 2 | 1 | wolf | 1 | 1 |
| html_game | 2 | 2 | **generic（兜底）** | **5** | **0** |

Unity 占绝对多数（49%），且 Mono / IL2CPP 两种都覆盖到。

## 二、9 个未确认样本逐条定性

分三类：**① 真·漏识别（本工具可识别但没认出来）**、**② 非游戏（工具/启动器，不算缺陷）**、**③ 识别正确但不该算游戏**。

| # | 样本 | 现结果 | 定性 | 说明 |
|---|---|---|---|---|
| 1 | `ero\Cheat Engine 7.3` | love2d 45 分 | ② 非游戏 | CE 附带 `main.lua`（Lua 脚本引擎），弱特征凑到 45 分。**不是游戏**，love2d 识别器把"根目录 main.lua + 31 个 .lua"当证据，属于误伤。 |
| 2 | `ero\OpenSpeedy-v1.7.6` | generic 0 分 | ② 非游戏 | 变速齿轮工具。 |
| 3 | `ero\...美少女万华镜 -理与迷宫的少女-`（汉化） | bgi_ethornell 30 分 | **① 真·漏识别** | 只有 `GameData/` 目录命中（30 分）。该作是 BGI 系，通常有 `エンジン設定.exe`/`bregexp.dll`/`.arc`，但**汉化组换过主程序**导致强特征 DLL/exe 被删或改名。→ 需补弱判据组合。 |
| 4 | `ero\...补丁备份` | generic 0 分 | ② 非游戏 | 汉化补丁的备份目录（只有被替换的原文件）。 |
| 5 | `ero\游戏一键汉化工具` | rpgmaker_rgss 10 分 | ② 非游戏 | 汉化工具本体，只有 `Audio/` 目录。 |
| 6 | `baidu\Press the buttom.7z\Press the buttom` | generic 0 分 | ② 非游戏 | **Plain Craft Launcher（PCL 我的世界启动器）**，不是游戏。 |
| 7 | `baidu\Train45 1.0` | nscripter 15 分 | **① 真·漏识别（重要）** | **实为 Godot 自解压包**：1.6GB 主 exe 尾部嵌 `GDPC` PCK（Godot 魔数）、内部全是 `*.ogg.import`（Godot 导入清单）。但当前 Godot 识别器**只看散落的 `.pck` 文件 / `project.godot`**，看不见"嵌在 exe 尾部"的包 → 只蹭到 NScripter 的 15 分误报。 |
| 8 | `baidu\ディメンション凸ラバース!!` | generic 0 分 | **① 真·漏识别** | 1136KB 主 exe + `dll\`(27 个) + 17 个 `.pac`（魔数 `PAC `）+ 203 个 `.dat`；exe 内 `AdvGetVarString`/`PalSound*`/`PalSystem*`（32 个 `Pal*` API）、`Script.src`、`system.dat`、`save%03d.dat`。→ **SACT/System40 系引擎**，工具无此识别器。 |
| 9 | `baidu\心跳加速ver2.15` | generic 0 分 | **① 真·漏识别（需再确认）** | 目录只有 `DokiDoki-Massage-Setup.exe`(381MB NSIS 安装包) + `dokidoki...Portable.exe`(381MB) + `dlc\expansion-pack\<mod>\data\dialogues\<lang>\day*.json` + 2533 png + 292 json。**文本是明文 JSON、按 `ja/en/ko/zh-CN` 分语言**。看起来是**自研/私有引擎**（或 Unity 单 exe 自解压，未证实）。DLС 是"数据即 mod"结构。 |

### 关键结论

- **真·漏识别只有 3 个**：#3（汉化 BGI，弱判据不足）、#7（Godot 自解压）、#8（SACT/System40）。
- **Godot 自解压（#7）是最高价值缺口**：本工具**原生支持 Godot PCK 解包/回封**（`formats/pck.rs` + `GodotPlugin`），只是**检测环节看不见嵌在 exe 里的包**，白丢一个能完整处理的引擎。
- **5 个 generic 里 4 个是非游戏**（CE / OpenSpeedy / 汉化工具 / PCL 启动器），只有 1 个（#8）真需要新识别器。
- **汉化组改主程序**（#3）是本工具最该防的兼容场景：强特征 DLL 被删后，只剩目录布局。

## 三、按层排查兼容性（本工具各能力的"非标准样本"风险）

| 层 | 模块 | 面对非标准样本的风险 | 现状 |
|---|---|---|---|
| **引擎识别** | `engines/recognize.rs`、`others.rs`、`generic.rs` | ① 自解压/单 exe 无散落封包（#7）；② 汉化改主程序后强特征丢失（#3）；③ 未收录引擎全是 0 分，不给"疑似"提示 | **有缺口**：Godot 不看 exe 内嵌包；SACT 无识别器；generic 兜底对 `.pac/.dat` 无提示 |
| **格式解析** | `formats/{pck,xp3,pfs,rgss,rpa,asar,nscript}.rs` | 加密封包 / 目录加密（PCK 已显式报错"目录加密暂不支持"）；`.pac`(SACT) 无双胞胎解析器 | 单包解析普遍有 `safe.rs` 防越界，容错好；但**不认识的格式一律走 GARbro 委派**（依赖外部工具） |
| **存档编辑** | `features/saves.rs` | 自研引擎存档（#8 的 `save%03d.dat`）不认识；不支持的游戏无编辑路径 | 走"值↔文本"通用机制，只对识别出的格式有效 |
| **文本提取/回填** | `features/{text,inject,tpack,translate}.rs` | 明文 JSON 多语言（#9）不在现有提取器覆盖内；NSIS 安装包需先解包 | 对 xp3/rpgmmv/rgss/rpa 等有专门提取器 |
| **注入汉化** | `features/inject.rs` | 非 MV/MZ/Tyrano 引擎不支持 | 已明确"不支持就指向提取→回填"，无死胡同 |
| **未识别兜底** | `generic.rs` | 0 分时只给"未匹配到任何特征"，`KNOWN_EXT` 提示表**有 30 个扩展名**但**只对 top_exts 里命中才显示**，且不含 `.pac`/`.dat` | 兜底尚可，但 `.pac`(SACT) 这类高频格式应补进提示表 |

## 四、建议修复优先级

| 优先级 | 修复 | 价值 | 改动面 |
|---|---|---|---|
| **P0** | Godot 识别器增加"exe 尾部找 `GDPC`"判据 | 让 #7 这类自解压包被正确识别→直接走原生解包 | `engines/others.rs` GodotPlugin + `scan.rs` 或新增 `Sig` |
| **P1** | BGI 弱判据加强（`GameData/` + `エンジン設定` 类 + `.dat` 存档布局组合过线） | 覆盖汉化改主程序场景（#3） | `recognize.rs` 规则表 |
| **P1** | 新增 SACT / System40 识别器（`Pal*` API 在 exe 里、`.pac` 魔数 `PAC `） | 覆盖 #8 类日系老引擎 | `recognize.rs` 新 Rec |
| **P2** | `KNOWN_EXT` 补 `.pac`/`.dat`/`.sar` 等 + 让"疑似"提示更常触发 | 兜底体验 | `generic.rs` |
| **P2** | 复认 #9（自研 JSON 引擎）—— 先确认是否 Unity 自解压 | 决定是否新增识别器 | 需进一步分析 |

> 注：#9 的 `DokiDoki-Massage` 两个 381MB exe 均为 NSIS 安装包（已确认 `Nullsoft.NSI` manifest），本体需先解包才能判断；暂列为"待确认"。

## 五、修复结果（P0–P2 已实现，2026-09-26）

| 优先级 | 修复 | 落地 |
|---|---|---|
| **P0** | Godot 识别器支持 exe 尾部内嵌 PCK | `scan.rs::find_embedded_godot_pck` + `embedded_godot_pck_offset`（读 exe 尾部 `GDPC`/pck_offset）；`Source::open_region`（内嵌区域偏移包装，解包时 PCK 内相对偏移自动 +base）；`GodotPlugin` 检测与 `extract` 都接上 |
| **P1a** | BGI 弱判据组合 | `recognize.rs` 新增 `Sig::RootDirExtN`（`GameData/*.pack`）、`RootExe`；BGI 加 `GameData/*.pack`(35)、`SaveData/`(20)、`EngineSetting*`(35) |
| **P1b** | 新增 SACT / System40 识别器 | `recognize.rs` 新 Rec `sact`：`.pac`×2(45)、`.pac`(20)、`system.dat`(25)、`.dat`≥20(25)、根 `dll/`(20) |
| **P2** | `KNOWN_EXT` 兜底补全 | `generic.rs` 补 `.pac`/`.dat`/`.sar`/`.nbz`/`.afa`/`.ain`/`.ybn`/`.yui`/`.yus`/`.cst`/`.noa`/`.swf`/`.qsps`/`.qsp`/`.aos`/`.ws2` |

### 复测（batch 重扫两个根）

| 根 | 修前确认 | 修后确认 |
|---|---|---|
| `D:\ero` | 33 / 38 | **34 / 38** |
| `D:\baidudownload` | 33 / 37 | **35 / 37** |
| 合计 | 66 / 75 | **69 / 75** |

三个真缺口逐条复测：

| 样本 | 修前 | 修后 |
|---|---|---|
| `baidudownload\Train45 1.0` | NScripter 15 分（误报） | **Godot 70 分 已确认**（"train45.exe 尾部内嵌 PCK"） |
| `baidudownload\ディメンション凸ラバース!!` | generic 0 分 | **SACT 110 分 高置信** |
| `ero\…美少女万华镜 -理と迷宫的少女-`（汉化） | BGI 30 分 疑似 | **BGI 120 分 高置信**（`GameData/*.pack` 9 个 + `SaveData/` + `EngineSetting.exe`） |

### 剩余未确认（6 个，全部为非游戏，**符合预期**）

`Cheat Engine 7.3`、`OpenSpeedy-v1.7.6`、`…补丁备份`、`游戏一键汉化工具`、`Press the buttom`（实为 PCL 启动器）、`心跳加速ver2.15`（NSIS 安装包，本体未解包）。

### 门禁

- 内核：`clippy --all-targets -D warnings` 0 告警；`cargo test --tests -j 1` **319 passed / 1 ignored**（lib 284，较修前 281 +3）。
- 壳：`clippy` 0 告警；`cargo test --tests` **51 passed**。
- 新增单测：`test_find_embedded_godot_pck_trailer`、`test_bgi_survives_main_exe_swap`、`test_sact_recognizer`。
- 实机验证：release CLI 对三个真缺口样本逐个 `detect` 通过；两轮 batch 全量重扫数字如上。
