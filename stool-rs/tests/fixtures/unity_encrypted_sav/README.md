# 固件：Unity + 第三方 SDK 加密的存档（只用于「格式识别」测试）

## 来源（真实游戏文件，不是合成数据）

游戏：`SexCP-069`（Unity **Mono** 版，Steam 发行）
原文件：`<游戏根>/SaveData/gamesave_01.sav`

| | 长度 | sha256 |
|---|---|---|
| 原文件（全量） | 117200 B | `52b4e204d2f412a6e372aaa260d312d9ff31bdcdbea0fd4b6634e7e977600742` |
| **本固件（前 4096 B）** | 4096 B | `a1e3d60bd3a3a9a050e9bac6253c02c4db58475c3cfbd074c1dfb5edb6979d87` |

固件 = 原文件的**前 4096 字节**，仅此而已（截断即可，因为被测的只是
「格式识别」这一步 —— 它只看长度、熵、前几个魔数字节）。

## 这个字节串为什么长这样（别当它坏掉了）

| 观察 | 事实 |
|---|---|
| 长度是 16 的倍数 | 有 PKCS7 填充 |
| 熵 7.955 bits/byte（前 4096） | 已加密 / 已压缩，不是明文 |
| 明文是 .NET `BinaryFormatter` | 用 `00 01 00 00 00 FF FF FF FF 01 00 00 00 00 00 00` 当已知明文反推，解出来**正好等于**该固定头 |
| 多个 `.sav` 前 16 / 32 字节相同 | 固定密钥分组加密（ECB 或 CBC 固定 IV）：明文头部相同 → 密文头部相同 |
| 加密方是谁 | `SexCP_069_Data/Managed/MarsSDK.Runtime.dll` 里的 `AESCryptography`（`CreateEncryptor` / `CreateDecryptor` / `_aes`），源码路径 `Packages/com.mars-sdk-1.0.0/Runtime/Files/AESCryptography.cs` |

## 本工具在此**不做**什么（重要）

`docs/ARCHITECTURE.md` §7：明确不做「需要**破解他人加密方案**的解密」。

所以这个固件**只用来钉死一件事**：这类文件必须被识别成
「疑似加密」并给出**有出路的提示**，而不是一句干巴巴的「无法识别」。
**不要**在这里加任何解密代码，也不要为了让它能跑而把阈值调松。

## 别手改

- 改了字节，上面的 sha256 就对不上了，测试还会照过（它只断言「被判为疑似加密」）——
  等于固件悄悄失效。要换样本就整份替换并更新本文件的长度 / sha256。
- `.gitattributes` 里有 `*.sav binary`：这串密文里**必然**含 `0x0A`，不显式声明为
  binary，`core.autocrlf=true` 的机器一 checkout 就会把 `0A` 变成 `0D 0A`，
  长度不再是 16 的倍数，测试直接失败（`mz_save` 的 `shared.rmmzsave` 就是这么踩过一次）。
