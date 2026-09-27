# RPG Maker MZ 存档固件（真实文件，别手改）

## 为什么需要它

MZ 的**磁盘存档比裸 zlib 多一层 UTF-8 包装**，这是 MZ 的原生行为：

```text
pako.deflate(json, { to: "string", level: 1 })   // 二进制字符串，1 字符 = 1 字节
  ↓ StorageManager.saveToLocalFile → fs.writeFile(path, zip)   // 字符串按 UTF-8 写盘
```

每个 `>= 0x80` 的字节因此被展开成 **2 个**字节（`0xAD` → `C2 AD`）。

2026-09-27 之前 `saves.rs::decode_mz` 是拿磁盘字节**直接** `ZlibDecoder::new(raw)`，
于是所有真实 MZ 存档一律报 `corrupt deflate stream`；
而当时的单测用的是**自己造的裸 zlib 流**，所以测试全绿、真机全挂 ——
典型的「拿自己构造的 mock 当证据」。

本目录这两个文件是**真机存档**，用来钉死这条：

- 「直接 inflate 必须失败」（证明包装这层真实存在，别再把它当可选）；
- 「先 `mz_unwrap` 再 inflate 必须解开」（证明还原方式对）；
- 「改写后回写也必须再包一次」（证明写回方向没搞反，否则游戏读不了）。

## 文件来源

| 文件 | 大小 | 来自 |
|---|---|---|
| `shared.rmmzsave` | 147 B | `D:\baidudownload\姫ハ運命ニ呻吟ヘリ\save\shared.rmmzsave` |
| `global.rmmzsave` | 284 B | 同目录 |

该游戏 = RPG Maker MZ + NW.js 0.76.1，发行版重打包（"命运束缚的公主 Steam Patched v1.0.5"）。
`shared.rmmzsave` 由插件 `DarkPlasma_SharedSwitchVariable` 写入，内容只有
`{"switches":[{"id":65,"value":true},…],"variables":[…]}`（跨存档共享开关），**不含个人信息**。

sha256（2026-09-27 提取时）：

```text
219a0b4c34c57e8434a1931c7c5bbd2c5c74b1818d1a2035e3947c6544d3c9c9  shared.rmmzsave
f21b06b1841bbbe57f850261de51cb2d20c608657b242b021614627cfcf4a339  global.rmmzsave
```

## 怎么重新生成（换游戏时）

```bash
# 任意一个真实 MZ 游戏（NW.js 本地模式）的存档目录：
cp "<游戏>/save/shared.rmmzsave"  stool-rs/tests/fixtures/mz_save/
cp "<游戏>/save/global.rmmzsave"  stool-rs/tests/fixtures/mz_save/
sha256sum stool-rs/tests/fixtures/mz_save/*.rmmzsave   # 更新上面的哈希
```

## 别做的事

- **别把这两个文件「修好」成裸 zlib** —— 那正是本固件要防的东西。
- **别用工具重写它们**（编辑器保存会改行尾/字节），它们必须是原样字节。
- 想加更大的样本（如 `file1.rmmzsave`，13 KB）可以，但要先确认里面没有个人信息
  （进度存档含 `player` / `party` / `map` 等游戏状态，一般可接受）。
