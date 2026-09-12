# STool — 多引擎游戏一站式综合工具

> 一个为 Ren'Py、RPG Maker、吉里吉里等主流视觉小说 / RPG 游戏引擎设计的"游戏工具箱"。
> 它能帮你完成这些事情：
>
> - **解包**：把游戏封包里的图片、音频、脚本等资源提取出来，变成普通文件
> - **封包**：把修改过的资源重新打回游戏封包
> - **提取文本**：把游戏里的台词、选项导出成表格（CSV 文件），方便翻译
> - **翻译回填**：把翻译好的表格内容重新写回游戏，实现汉化
> - **修改存档**：像 saveeditonline 一样打开存档、搜索数值、直接修改、保存回原文件
> - **运行时修改**：游戏开着就能改！RPG Maker MV/MZ 按名字精确改金币/变量/物品；
>   其他任何游戏用内置内存扫描修改器（和 Cheat Engine 用法一样），不用再装 CE
> - **管理补丁（MOD）**：一键安装、卸载、启用、停用玩家自制补丁，卸载时自动还原原文件；
>   多个补丁争用同一文件时会告警，不会悄悄覆盖
> - **封包自检**：一键完成"解包 → 重新封包 → 逐条目比对"，确认改动没有破坏原文件
> - **批量检测**：给一个根目录，自动扫描下面所有游戏、识别引擎并生成 CSV 报告
> - **游戏体检**：检查非 Unicode 区域设置、缺失的运行库 DLL、日文字体等常见"跑不起来"的原因
>
> 使用 Rust 语言编写：整个程序只有一个 exe 文件（约 6 MB），运行时内存占用很低。
> 程序内部按"引擎插件"的方式组织，以后想支持新引擎，加一个插件即可。
>
> 处理大封包时也做了优化：解包**多线程并行**、支持**断点续传**（中途取消/崩溃后重跑自动接着来）、
> 解析器**流式读取**（几 GB 的封包不再整包塞进内存）。

## 支持哪些引擎？能做什么？

下表中"解包"指提取资源文件，"封包"指把资源打回游戏，"反编译"指把加密的脚本还原成可读代码，
"文本"指提取台词翻译再回填，"存档"指查看或修改存档。

| 游戏引擎 | 这是哪些游戏用的 | 解包 | 封包 | 反编译 | 文本 | 存档 | 说明 |
|---|---|---|---|---|---|---|---|
| Ren'Py | 大量欧美及同人视觉小说 | ✔ | ✔ | ✔（借助 unrpyc 工具） | ✔ | ✔（可全解锁游戏进度记录） | 支持全部三种封包版本；老式加密方式也能兼容 |
| RPG Maker MV / MZ | 大量日系 RPG 游戏 | ✔（解密图片和音频） | ✔（重新加密） | — | ✔（台词、选项、词条） | ✔（导出成可读的 JSON 再改回） | 游戏数据本身是明文 JSON，可直接编辑 |
| RPG Maker XP / VX / Ace（老三代） | 经典日系 RPG | ✔ | ✔ | ✔（提取 Ruby 脚本） | ✔（含 Marshal 脚本指令深度提取） | 只读查看 | 支持两种加密封包格式（.rgssad 和 .rgss3a 后缀）；封包逐文件流式回写并自动备份 |
| 吉里吉里（KiriKiri） | 大量日系视觉小说 | ✔（标准头 + 网盘伪装变体：裸目录 / zlib 压缩目录、目录后置的翻译补丁变体） | ✔（按 GARbro / KiriKiri 规范回写，v1/v2 头均支持） | — | ✔（剧本对话） | — | 另有**运行时补丁包**：把改动文件打成 `patchN.xp3`（引擎按搜索顺序自动优先加载、删包即完全还原），几 GB 的 `data.xp3` 一个字节都不用动；Hxv4（合成文件名+数据加密）与目录加密的第三方汉化补丁属保护类，会给出明确提示而不是乱码 |
| Artemis Engine | 部分日系视觉小说（除 BGI 外另一个常见引擎） | ✔（`.pfs` 的 pf6 / pf8 两种代次，含 pf8 的 SHA-1 流混淆） | ✔（沿用原代次回写，原封包自动备份） | — | — | — | 与 BGI 同为 `.pfs` 后缀——按封包内容与伴随特征区分，不靠后缀猜；分卷封包（`x.pfs.000`）会明确提示不支持拼接并给出替代做法 |
| Godot | 部分独立游戏 | ✔ | ✔ | ✔（借助 GDRE Tools 一键下载） | — | — | 封包回写为未加密 v1，Godot 3.x/未加密 4.x 可用；遇到加密封包会明确提示 |
| NScripter / ONScripter | 老牌视觉小说引擎 | ✔（解密主脚本） | — | — | ✔（台词提取 + Shift-JIS 校验回填，自动备份） | — | 自动尝试多种解密方式，选效果最好的 |
| HTML / Electron 网页游戏 | 网页技术做的游戏 | ✔ | ✔ | — | — | — | 资源解出来就是普通网页文件，直接改 |
| Wolf RPG Editor（Wolf 引擎） | 日系 RPG | ✔（借助 WolfDec 工具） | — | — | ✔（从 Game.dat 等提取对话） | — | 需要在设置页填写 WolfDec 的路径 |
| Unity（Mono / IL2CPP 都支持） | 3D / 手游向游戏 | ✔（借助 AssetRipper 工具） | — | — | — | ✔（可一键全 CG 解锁） | 全 CG 解锁**不需要反编译器、不需要 .NET 运行环境**：直接读 `Managed/Assembly-CSharp*.dll`（Mono 版）或 `il2cpp_data/Metadata/global-metadata.dat`（IL2CPP 版）里的字符串，把画廊的 PlayerPrefs 键名写进注册表——**不动游戏文件**，缺省只读预览、写入前自动备份原值、可一键还原。解包仍需在设置页填写 AssetRipper 的路径 |
| 识别不出的游戏 | — | 借助 GARbro 工具兜底 | — | — | — | — | 同时给出文件后缀特征提示，方便人工判断 |

## MTool 式汉化（JSON 注入，不改游戏文件）

除了"提取 CSV → 翻译 → 回填"这种会改写游戏数据文件的方式，还支持 **MTool 同款的运行时注入**：

1. 准备一个翻译 JSON（兼容 MTool 的 `translation.json` 格式）：
   ```json
   { "原文": "中文译文", "Hello world": "你好，世界" }
   ```
   整句精确匹配；也允许分组嵌套（如 `{ "对话": { "原文": "译文" } }`，加载时自动拍平）。
2. 打开"文本汉化"页，点 **③ 注入翻译**（或命令行 `stool-cli text-inject <游戏目录> -o 翻译.json`）。
3. 启动游戏即生效：程序在内存里把命中的文本替换为中文，**不修改任何原始资源文件**；
   没匹配到的文本自动保留原文。

约定与还原：

- 翻译文件会被复制为游戏目录下的 `stool_translate.json`（游戏运行时读取；Ren'Py 例外：映射直接写进 .rpy）；
- 注入只新增/登记汉化文件（MV/MZ 登记 `js/plugins.js` 并自动备份原件；Ren'Py 新增独立 .rpy）；
- 点"移除注入"（或 `stool-cli text-uninject <游戏目录>`）即可还原（MV/MZ 字节级还原）；
- 支持的引擎：**RPG Maker MV / MZ**（NW.js 插件注入）、**Ren'Py**（`config.replace_text` 运行时替换）、
  **HTML / Electron**（DOM 文本节点 MutationObserver 替换，Canvas 画面内的文本除外）；
  其余引擎（RGSS/Godot/NScripter/Unity/Wolf）文本封在私有封包或编译脚本里、无运行时注入点，
  会明确提示改用"翻译回填"；吉里吉里虽无内存注入点，但可改用**运行时补丁包**（见"运行时修改"页的方式三）。

## 怎么修改存档？（存档编辑器）

打开图形界面的"💾 存档编辑"页：

1. **打开存档文件** —— 直接把存档文件**拖进窗口**最方便；也支持手动选择，程序会自动识别格式
2. **搜索定位** —— 输入键名、数字或文字（比如金币数 1000、角色名），列出所有匹配位置
3. **直接修改** —— 点搜索结果或在目录树里找到想改的值，改完点开输入框外任意地方即生效
4. **保存回写** —— 一键写回原文件，改之前自动备份为 `*.stool.bak`；也可以导出/导入 JSON 做备份或分享修改

支持的存档格式：

| 格式 | 常见于 | 能否改 |
|---|---|---|
| RPG Maker MV（.rpgsave） | 日系 RPG | ✔ 保存时自动压缩回去 |
| RPG Maker MZ（.rmzsave） | 新日系 RPG | ✔ |
| JSON 明文存档 | 网页/Electron 游戏 | ✔ |
| RPG Maker XP/VX/Ace（.rxdata 等） | 经典 RPG | 只能看（老格式很难安全回写） |
| Ren'Py 进度记录（persistent） | 视觉小说 | 只能看（可用"一键全解锁"） |
| 未知扩展名 | — | 自动嗅探内容，认得出来就处理 |

## 怎么直接看游戏里的资源？（资源预览）

打开"🖼 资源预览"页，把游戏目录**拖进窗口**（或手动选择）：

- 左侧列出目录里所有图片和音频文件，支持按名称关键字筛选
- 点图片文件**直接预览**（PNG / JPEG / BMP / WebP / GIF / TGA / TIFF）
- 点音频文件**直接试听**（WAV / OGG / MP3 / FLAC），可调音量、随时停止
- 这样可以在解包前先确认某个资源是不是你想要的，不用一个个文件导出来看

## 怎么在游戏运行时改数据？（运行时修改）

打开"🎯 运行时修改"页，两种方式任选：

**方式一：RPG Maker MV / MZ 游戏（推荐，改了立刻生效）**

全自动：在首页检测游戏目录后，切到本页就会**自动启动游戏并连上**（游戏窗口自动弹出属正常现象），
连上后界面会列出当前金币、所有游戏变量（带变量名，如"#12 学生的好感度"）、持有物品——点"选"或手动填编号和新值，点"✅ 写入游戏"。

游戏已经手动开着连不上时：先关掉游戏，回到本页点"🔌 连接游戏"重试（或用"▶ 以调试模式启动游戏"按钮手动启动）。

原理：让游戏自带的浏览器内核（NW.js）开放调试端口，程序直接在游戏页面里执行 JS 调用游戏自己的
存档接口（`$gameVariables`、`$gameParty` 等），所以改完立即生效、菜单和界面都会正常刷新，
不是简单改内存数字。

**方式二：通用内存扫描（其他任何引擎的游戏）**

用法和 Cheat Engine 完全一样：

1. "🔄 刷新进程列表"，选中正在运行的游戏进程
2. 记住游戏里的数值（比如金币 100），选类型（一般是"整数 32 位"），填 100，点"🔎 首次扫描"
3. 回游戏花点钱让数值变化，回来说明新值（比如 80），选"等于"，点"再次扫描"
4. 重复过滤到只剩几条，填入想要的值，点"✅ 写入所有命中"

支持整数 32/64 位、小数 32/64 位、UTF-8 / UTF-16 文本。
扫完后可以**锁定数值**（游戏里怎么花都花不掉）和**撤销写入**（一键恢复原值）。
如果扫不到，试试以管理员身份运行 STool。

**方式三：封包类引擎的运行时补丁包（吉里吉里 / KiriKiri）**

吉里吉里的资源封在 `data.xp3` 里，但引擎启动时按固定顺序加载：
`Data 目录 → data.xp3 → patch.xp3 → patch2.xp3 → …`，**后面的会覆盖前面的**。
利用这一点，把你改过的文件（保持封包内的原始相对路径）打进**下一个空号**的补丁包，
引擎就会自动优先加载你的版本——`data.xp3` 一个字节都不用动，删掉补丁包即完全还原。
这也是社区汉化/魔改补丁的通行做法。

在本页"方式三"区块选好改动目录、点生成即可；命令行等价写法：

```bash
stool-cli xp3-patch <游戏目录> --list                     # 列出游戏里已有的封包和编号
stool-cli xp3-patch <游戏目录> --src 改动目录             # 自动取下一个空号生成补丁包
stool-cli xp3-patch <游戏目录> --remove patch.xp3         # 移除（只删本工具生成的）
```

安全约束：**绝不覆盖游戏自带的补丁包**（检测到非本工具创建的包会拒绝并自动改取下一个空号）；
补丁包名强制为 `patch*.xp3`（挡掉把 `data.xp3` 误当补丁写的操作）；移除只认本工具生成的包。

**「解包 → 汉化 → 打补丁」闭环**：如果封包已经整个解开过，可以直接把**解包目录**交给它，
它会和游戏现有封包逐字节比对（按引擎搜索顺序，游戏自带补丁包也算在内），
**只把真正改过的文件**打进补丁包——未改动的文件不会白占体积：

```bash
stool-cli extract <游戏目录> -o <解包目录>                  # 1. 解包
stool-cli text-extract <解包目录> -o text.csv               # 2. 提台词
stool-cli text-mtl text.csv --preset deepseek --key sk-xxx  # 3. 机翻
stool-cli text-import <解包目录> text.csv                   # 4. 回写译文
stool-cli xp3-patch <游戏目录> --from-extract <解包目录>     # 5. 只把改动打成 patchN.xp3
```

第 5 步若一个文件都没改，会明确报「没有发现任何改动」而不是产出一个空补丁包。

## 怎么用？

直接双击 `stool.exe` 就会打开图形界面（支持中文显示，**不会弹出黑色控制台窗口**），按页面提示操作即可。

喜欢命令行的话，使用目录下的 `stool-cli.exe`（专门为终端准备的版本）：

```bash
# 判断这个游戏是用什么引擎做的（会给出判断依据和置信度）
stool-cli detect  <游戏目录>

# 把游戏资源解包出来（自动识别引擎；-p 可手动指定引擎）
stool-cli extract <游戏目录> -o 输出目录

# 把解包出来的资源重新封包回游戏（原封包自动备份为 *.stool.bak）
stool-cli repack <游戏目录> --opt:src_dir=解包输出目录

# 把加密脚本还原成可读代码（Ren'Py 游戏借助 unrpyc，RPG Maker 老三代提取 Ruby 脚本）
stool-cli decompile <游戏目录> -o 输出目录

# 把台词导出成 CSV 表格（列：编号 / 文件 / 位置 / 原文 / 译文）
stool-cli text-extract <游戏目录> -o 文本.csv

# 把翻译好的 CSV 表格写回游戏
stool-cli text-import <游戏目录> 文本.csv

# 把 RPG Maker MV / MZ 的存档转换成可读的 JSON，改完再转回去
stool-cli save <游戏目录> -o 输出目录

# 一键解锁 Ren'Py 游戏的完成度记录（如 CG 回廊、回想模式全开）
stool-cli unlock <游戏目录> --opt set_true_all=1

# Unity 全 CG 解锁：先只读看看会写哪些键（完全不碰注册表）
stool-cli cg-candidates <游戏目录> --opt:filter=cg
# 确认无误再真正写入（写前自动备份原值，可用 --opt:restore= 一键还原）
stool-cli unlock <游戏目录> --opt:apply=1 --opt:route=registry

# 命令行直接搜索/修改存档（适合批量脚本化处理）
stool-cli save-edit <存档文件> --search 关键词
stool-cli save-edit <存档文件> --set /system/gold=99999 --set /items/1/count=99

# 安装 / 管理 MOD（卸载时会自动还原被覆盖的原文件）
stool-cli mod-install <游戏目录> <补丁目录> -n 补丁名称
stool-cli mod-list / mod-enable / mod-disable / mod-uninstall <游戏目录> [补丁名称]

# MTool 式 JSON 注入汉化（MV/MZ，运行时替换，不改游戏文件）
stool-cli text-inject <游戏目录> -o 翻译.json
stool-cli text-uninject <游戏目录>   # 移除注入，字节级还原

# 把文本提取的 CSV 转成注入 JSON 骨架（原文为键；已有译文自动带入，其余留空待机翻）
# GUI：文本/汉化页 → JSON 通道 → ① 从 CSV 生成 JSON 骨架
stool-cli text-mtl text.csv --preset deepseek --key sk-xxx   # 先机翻 CSV
# （GUI 里同样支持"从 CSV 生成"，生成后直接机翻 JSON → 注入）

# 机器翻译（自动填充 CSV translation 列或注入 JSON 的空译文，断点续翻）
stool-cli text-mtl text.csv --preset deepseek --key sk-xxx
stool-cli text-mtl 翻译.json --base-url https://open.bigmodel.cn/api/paas/v4 --model glm-4-flash --key xxx
# 预设：deepseek / zhipu / ollama（本地模型免 Key）；中断重跑自动从 <文件>.mtl.json 断点继续

# 备份还原与 原版/汉化 封包切换
stool-cli restore <游戏目录>            # 列出目录里所有 .stool.bak 备份
stool-cli restore <备份文件.stool.bak>   # 用备份还原该文件（备份保留，可反复还原）
stool-cli archive-toggle <游戏目录>      # repack 过的封包一键在 原版/汉化 间切换

# 翻译包：CSV + 注入 JSON + 清单 打包成单个 zip（备份 / 换机 / 分享给同游戏玩家）
stool-cli pack-export -o 包.stoolpack.zip --csv text.csv --json 翻译.json --engine rpgmaker_mv --game 游戏名
stool-cli pack-import 包.stoolpack.zip <游戏目录>   # JSON 自动进游戏目录（可注入）、CSV 进输出目录

# 检查类：写操作前预检 / 封包完整性自检 / "游戏跑不起来"体检
stool-cli precheck <游戏目录> [-o 输出目录]        # 目录可写、文件占用、磁盘空间
stool-cli selfcheck <游戏目录|封包文件>            # 解包 → 重打包 → 逐条目比对是否无损
stool-cli doctor <游戏目录>                        # 区域设置 / 运行库 DLL / 日文字体 / 写权限

# 批量：给一个根目录，自动扫描下面的游戏并生成报告
stool-cli batch <根目录> [--depth N] [--op detect|extract|decompile|text-extract|save|unlock] [-o 输出] [--csv 报告.csv]

# 回填一键闭环：用重打包产物替换原封包（覆盖前自动留底），配合 archive-toggle 可来回切换
stool-cli pack-apply <原封包> <新封包>

# 诊断包：把环境、日志、检测结果打成 zip，方便排查问题
stool-cli diag-export <游戏目录> -o 诊断包.zip
```

**完整汉化工作流**：`text-extract` 提取 → `text-mtl` 机翻（或人工翻译 CSV）
→ `text-import` 回填；或写好 JSON 后 `text-mtl` 机翻 → `text-inject` 运行时注入。
吉里吉里这类封包引擎还可以走「解包 → 汉化 → 打补丁」闭环（见上文"运行时修改"页方式三）。

## 从源码构建

需要 **Rust 稳定版工具链** + **MSVC 生成工具**（Windows 目标 `x86_64-pc-windows-msvc`）。
开发环境是 Git Bash + MSVC，关键在于**让 `rustc` 与链接器找得到 VC 运行库**：

```bash
# 1) 安装工具链（已装可跳过）
rustup toolchain install stable-x86_64-pc-windows-msvc

# 2) 把 VC 运行库与本机工具链加进 PATH
#    版本号按本机 Visual Studio 的实际路径调整（Community/BuildTools、14.xx.xxxxx 都可能不同）
export PATH="/c/Program Files/Microsoft Visual Studio/2022/Community/VC/Redist/MSVC/14.36.32532/x64/Microsoft.VC143.CRT:$PATH"
export RUSTC="$HOME/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin/rustc.exe"
# 关闭增量编译，避免偶发的 target/ 目录占用报错（os error 5）
export CARGO_INCREMENTAL=0

# 3) 构建与验证
cd stool-rs
cargo build --release                       # 产物：target/release/stool.exe、stool-cli.exe
cargo test --tests                          # 跑全部测试
cargo clippy --all-targets -- -D warnings    # 质量门禁：必须零警告
```

两点说明：

- **测试请用 `cargo test --tests`**，不要用裸 `cargo test`：本项目没有 doctest
  （文档里的代码块都标了 ` ```text `），显式限定可避免 Windows 上 doctest 执行器的环境差异造成假失败。
- **本项目不设 `cargo fmt --check` 门禁**：仓库是手工维护的「宽行」风格
  （`stool-rs/rustfmt.toml` 仅作编辑器指引）。全量 `cargo fmt` 会产生约 2000 行、跨 37 个文件的改动，
  与「保持既有风格一致」冲突。

## 程序结构（给想参与开发的你）

```
src/
├── formats/   # 各种游戏文件格式的"读 / 写"实现，每个格式一个独立模块，可以单独拿去复用
│              #   safe.rs 越界安全读取（解析器永不 panic）/ source.rs 流式数据源（大封包不整包进内存）
│              #   pfs.rs Artemis 封包（pf6/pf8，含 SHA-1 流混淆与写侧）
│              #   dotnet.rs 最小 .NET/PE 元数据读取（#US / #Strings 堆，不反编译）
│              #   il2cpp.rs Unity IL2CPP 的 global-metadata.dat 字符串读取
├── engines/   # 引擎插件：每个引擎一个插件（如何识别它、它能做哪些事），统一注册进插件列表
│              #   scan.rs 单次遍历扫描 / recognize.rs 证据打分识别 / artemis.rs Artemis 原生支持
├── features/  # 通用功能：文本表格读写、存档编辑器、运行时修改（MV/MZ 调试协议 + 内存扫描）、
│              #   补丁管理、资源预览（图片/音频）、外部工具一键下载、MTool 式 JSON 注入汉化、
│              #   precheck 预检 / selfcheck 封包自检 / health 游戏体检 / batch 批量 /
│              #   unlock 解锁归纳 / gallery Unity 全CG解锁（程序集精确键名 + 注册表快照还原）/
│              #   xp3patch 吉里吉里运行时补丁包
├── cli.rs     # 命令行入口
├── gui/       # 图形界面（mod.rs + pages/ 各页 + util / save_tree；自动加载中文字体，界面不乱码）
├── hash.rs    # 无依赖 SHA-256 / SHA-1（校验一键下载的外部工具是否被篡改 + pf8 流混淆）
└── settings.rs# 配置（~/.stool/config.json；机翻 API Key 用 Windows DPAPI 加密后落盘）
```

**想加一个新引擎？** 只需实现一个 `Engine` 接口（其中"如何识别该引擎"必须实现，
其余功能按需实现），然后在插件列表里注册一行即可。
识别采用"证据打分"机制：游戏目录里命中的特征越多，分数越高，超过 60 分判定为该引擎；
任何一个插件出错都不影响其他插件工作。

## 可选的外部辅助工具（无需手动下载）

以下工具不是必需的，但可以扩展出更多引擎的支持能力。
**不需要自己去找、去下载**：打开"⚙ 设置"页，点每个工具旁边的"⬇ 下载"按钮，
程序会自动从官方 GitHub Release 下载、解压到 `~/.stool/tools/` 并填好路径
（下载不动时可在设置页配置代理）。手动安装的话，把可执行文件完整路径填进去保存即可。

下载是**流式**的（不会把上百 MB 的压缩包整个读进内存），并且**边下边算 SHA-256**，
与 GitHub 给出的官方摘要比对——不一致直接中止并删除临时文件（防代理劫持或上游换包）；
上游没提供摘要时，会把算出的校验和记到 `~/.stool/tools/<工具名>/.sha256` 供人工核对。

- **unrpyc** — 把 Ren'Py 的加密脚本还原成可读源码（需要电脑装有 Python；本程序会自动寻找捆绑的副本）
- **WolfDec** — 解包 Wolf 引擎游戏
- **AssetRipper** — 导出 Unity 游戏的资源（传统的 Mono 版 Unity 游戏也可以用 dnSpy 查看）
- **GARbro** — 通用游戏资源浏览器，作为识别不出格式时的兜底方案
- **GDRE Tools** — 反编译 Godot 游戏（提取资源和脚本）

## 测试情况

本项目的质量门禁：`cargo clippy --all-targets -- -D warnings`（零警告）+ `cargo test --tests`。

当前 `cargo test --tests` 共 **228 项测试全部通过**（另有 1 项需联网/写真实注册表的用例默认忽略）：

- **193 项单元测试**（内置在 `src/` 各模块）：各格式解析/回环、存档编辑、文本提取回填、
  MTool 式 JSON 注入、MOD 管理、解锁策略、预检/自检/体检/批量、吉里吉里运行时补丁包
  （含「只打包改动」比对）、断点续传、并行解包、机翻重试退避、SHA-256/SHA-1 官方向量、
  API Key 加解密、Unity 精确键名提取（.NET 元数据 / IL2CPP 元数据，含逐字节变异模糊）……
- **16 项回环测试**（`tests/roundtrip.rs`）：每种封包"生成样本 → 解析 → 写回 → 再解析"，
  与原文件逐字节比对
- **9 项模糊测试**（`tests/fuzz_parsers.rs`）：对解析器投喂畸形/截断/随机输入，
  验证**永不 panic**（越界一律安全返回）
- **6 项流式等价性测试**（`tests/streaming.rs`）：同一封包分别用内存模式与强制文件模式解析，
  要求逐字节一致
- **4 项并行一致性测试**（`tests/parallel.rs`）：真实跑 `jobs=1` 与 `jobs=8`，
  产物逐文件逐字节相同
- **交叉验证**：另有 Python 第三方解析器（各写一份、不共用代码，用来核对 Rust 实现的解读）
  - `scripts/peek_xp3.py` —— 按 GARbro/KiriKiri 规范独立核对 XP3 头部/条目/校验和
  - `scripts/peek_dotnet.py` —— 独立核对 .NET `#US` / `#Strings` 堆
  - `scripts/peek_il2cpp.py` —— 独立核对 `global-metadata.dat`（三区连续性、`dataIndex` 步进、UTF-8 可解率）

关键格式的解密算法都对照了成熟的开源实现（GARbro、RPGMakerDecrypter、
pieroxy/lz-string 官方源码）逐一核对过，确保对真实游戏文件有效。

## 真实游戏验证

不只跑合成样本——以下能力都用本机真实游戏完整验证过：

| 引擎 / 格式 | 验证游戏 | 结果 |
|---|---|---|
| RPG Maker Ace（.rgss3a） | BLACK SOULS II | 2591 文件封包→解包逐字节一致；`selfcheck` 重打包后 2591 条目全部无损；Marshal 脚本指令提取 18385 行台词，0 跳过 |
| Godot（.pck v2） | 囚われのリリ | 8166 文件解包，回封后再解包与原结果完全一致；`selfcheck` 8166 条目全部无损 |
| 吉里吉里 XP3（目录后置变体） | ライムライト unencrypted.xp3 | 189 文件全提取，TJS 脚本、TLG 图像均有效 |
| 吉里吉里 XP3（引擎加密） | 小粥姉妹 | 3 个封包的 `info.flags` 均置加密位：解包**明确报错并给出替代路线，零产物、不输出乱码**；补丁包功能正确识别已占号（data/patch/patch2），自动取号 `patch3.xp3` |
| 解包断点续传 | BLACK SOULS II（.rgss3a） | 解包中途强杀 → 重跑自动续传（跳过 2048 个已完成条目），最终 2591 文件完整、台账自动清理 |
| Artemis（.pfs / pf8） | 美少女万華鏡 呪われし伝説の少女、アマカノ3 | 检测判定 70 分确认；`root.pfs`（1.5 GB / 2703 条目）与 `Amakano3.pfs`（1.7 GB / 3361 条目）索引 100% 解析零越界，解包产物 PNG/OTF/OGV/`.ast` 全部有效；`selfcheck` 重打包逐条目无损 |
| Unity Mono 全CG解锁（.NET 元数据） | Inari | 精确定位 `Managed/Assembly-CSharp.dll`（+ `-firstpass`）；读出 10085 条 `#US` 字面量、26460 条 `#Strings` 名字，识别出 164 个画廊类型/字段（如 `<<OpenGallery>g__GoGalleryScene\|2>d`）；与独立 Python 解析器计数完全一致 |
| Unity IL2CPP 全CG解锁（global-metadata.dat） | Yakuzarogue、MiraisMidnightStream、sinSister 等 6 款 | 6/6 样本正确解析（v24/v29/v31），字面量 UTF-8 可解率 99.99%（16662/16663）；Yakuzarogue 直接扫出真实键 `cg_button_name1`…`cg_button_name7`，Mirais 扫出 `CG0`…`CG9` |

## 网盘发布游戏的"伪装"适配

网盘渠道发布的游戏，发布者常对封包做改动以躲避审查。本机实测到的 4 类形态及处理方式：

| 形态 | 典型特征 | 处理 |
|---|---|---|
| XP3 v2 扩展头（0x17）+ 裸目录 | 头部 40 字节，索引偏移藏在偏移 32 处 | ✔ 正常解包/封包 |
| XP3 目录 zlib 压缩 | 目录区以压缩标记开头 | ✔ 正常解包/封包 |
| 目录后置 + 免责声明头（翻译资源） | 头部是一段 zlib 压缩的说明文字，真目录在文件尾 | ✔ 正常解包 |
| Hxv4（合成文件名 + 数据段加密）/ 目录加密的汉化补丁 | 文件名是顺序合成字符，或全文找不到目录结构 | ⛔ 属刻意保护，明确报错并说明原因，不输出乱码 |

> 顺带说明：`0x17` 扩展头是 KiriKiri / 吉里吉里Z 的**标准头布局**，并非伪装；本工具写出的
> 封包也采用这一布局（同时兼容老版 v1 头），以确保真实引擎能正常加载。

注意：少数游戏的封包对**数据段**做了逐文件加密（以小粥姉妹为例，3 个封包均如此），
头部会标记加密位（`info.flags` bit31）。这类内容解出来是随机密文，本工具会**明确报错并给出
替代路线**（GARbro / KrkrExtract），而不是输出一堆乱码文件——解开这类加密属于破解范畴，本工具不做。

## 使用限制与注意事项

- 本工具仅用于**你已合法拥有副本**的游戏的个人研究、汉化和 Mod 制作，不得用于盗版分发
- 部分游戏使用了开发商自定义的加密方式（吉里吉里 Hxv4 保护、Wolf 高版本加密、
  Unity 的 IL2CPP 打包），这些需要借助对应的外部工具或暂不支持
- 所有会修改游戏文件的操作（封包、存档、进度解锁、MOD 安装）都会先把原文件
  自动备份为 `*.stool.bak`，改坏了随时可以还原
- **机翻 API Key 不会明文落盘**：保存设置时用 Windows DPAPI（按当前用户派生密钥）加密后
  写入 `~/.stool/config.json`。该密文**绑定当前 Windows 用户**，把配置拷到别的机器/账户会解不开——
  此时程序会告警并清空该字段，重新填一次即可，其它设置不受影响
- **内存扫描类操作（修改游戏进程内存）会在首次写入前弹确认**，说明可能的风险；
  确认过一次后不再重复询问（可随时在界面上取消勾选）
