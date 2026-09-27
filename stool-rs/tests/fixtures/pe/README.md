# PE 版本资源固件（真实数据，别手改）

这两个 `.bin` 是从**真实游戏的 `nw.dll`** 里抠出来的 **RT_VERSION 资源**（VERSIONINFO），
用于给 `src/formats/pe.rs` 的解析器提供**真实输入** —— 而不是我自己造的 mock。

| 文件 | 来源 | RT_VERSION 字节数 | sha256 |
|---|---|---|---|
| `nwjs_0_76_1_version_resource.bin` | `D:\baidudownload\姫ハ運命ニ呻吟ヘリ\nw.dll`（NW.js **0.76.1**） | 1072 | `85c7ff40a07288e46d5f501be788ba87db2549145994e1698d6c445c39c351d3` |
| `nwjs_0_48_4_version_resource.bin` | `D:\baidudownload\天使の早漏治療クリニック\nw.dll`（NW.js **0.48.4**） | 1076 | `2685b8bb0203c9331205e0518dd6b6ee52f2dc7c45763c96365f8257688d3562` |

## 为什么是这两个版本

NW.js **≥ 0.70** 起上游坏了 `--remote-debugging-port`（[nwjs/nw.js#8191]），
「方式一·游戏里改数值」在这些游戏上**任何启动姿势都开不出端口**；0.48/0.49 则正常。
所以「能不能读出 0.76 与 0.48 并区分开」直接决定 STool 该说哪句话 —— 这两个固件就是那条判据的真实样本。

[nwjs/nw.js#8191]: https://github.com/nwjs/nw.js/issues/8191

## 怎么重新抠（若固件损坏或要换样本）

```bash
# 该脚本会走 PE 节表 + 资源目录三级树，把 RT_VERSION 抠出来写到本目录
python D:\tmp\extract_pe_version.py
```

脚本里写死了两款游戏的路径；换样本时改 `GAMES` 即可。

## 别做的事

- **别手改这两个 `.bin`** —— 它们必须是真实 `nw.dll` 的字节切片，改了就等于把固件变成自造数据。
- 别指望从仓库里直接看出它们属于哪个游戏：脚本会打印来源路径与 sha256，重跑即可对账。
