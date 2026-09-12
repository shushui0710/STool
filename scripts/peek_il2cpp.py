#!/usr/bin/env python3
"""独立实现的 Unity IL2CPP `global-metadata.dat` 字符串读取器。

用途：**交叉验证** `stool-rs/src/formats/il2cpp.rs` 的解读是否正确 ——
第三份实现（不共用任何代码），用来核对"字面量池切出来的到底是不是完整串"。

用法：
    python scripts/peek_il2cpp.py <global-metadata.dat> [--dump N]

判据（正确解读时应全部成立）：
  * 三区首尾严格相接：lit_off + lit_sz == dat_off、dat_off + dat_sz == str_off
  * `stringLiteral` 的 dataIndex 恰好按 len 步进（单调递增且无空洞/重叠）
  * 切出的字面量可解 UTF-8 比例 ≈ 100%
  * `string` 区首个非空串通常是 `mscorlib`

文件头（实测 v24 / v29 / v31 一致）：
    @0  u32 magic = 0xFAB11BAF
    @4  i32 version
    @8  i32 stringLiteralOffset      / @12 i32 stringLiteralSize
    @16 i32 stringLiteralDataOffset  / @20 i32 stringLiteralDataSize
    @24 i32 stringOffset             / @28 i32 stringSize

`stringLiteral` 区 = size/8 条 {i32 len; i32 dataIndex}；
`stringLiteralData` 区 = 字符串池，第 i 条字面量 = pool[dataIndex : dataIndex+len]
（**len 是精确字节长度，池内无 NUL 终止符** —— 这点错了会得到"每条都被截掉末字符"的碎片）。
"""

import struct
import sys

MAGIC = 0xFAB11BAF


def peek(path: str, dump: int = 0) -> int:
    data = open(path, "rb").read()
    if len(data) < 64:
        print(f"✘ 文件过小（{len(data)} 字节）")
        return 1
    magic, ver = struct.unpack_from("<Ii", data, 0)
    print(f"文件: {path}")
    print(f"大小: {len(data)}   magic: 0x{magic:08X}   version: {ver}")
    if magic != MAGIC:
        print("✘ magic 不符，不是 IL2CPP 元数据")
        return 1
    if not (24 <= ver <= 31):
        print(f"⚠ 版本 {ver} 不在 24..31（本脚本未验证区间外的布局）")

    lit_off, lit_sz, dat_off, dat_sz, str_off, str_sz = struct.unpack_from("<6i", data, 8)
    print(
        f"stringLiteral     off={lit_off} size={lit_sz}\n"
        f"stringLiteralData off={dat_off} size={dat_sz}\n"
        f"string             off={str_off} size={str_sz}"
    )

    ok = True
    if lit_sz <= 0 or lit_sz % 8:
        print(f"✘ stringLiteral 区大小 {lit_sz} 不是 8 的正数倍")
        ok = False
    if lit_off + lit_sz != dat_off:
        print("✘ 区不连续：lit_off + lit_sz != dat_off")
        ok = False
    if dat_off + dat_sz != str_off:
        print("✘ 区不连续：dat_off + dat_sz != str_off")
        ok = False
    if str_off + str_sz > len(data):
        print("✘ string 区越界")
        ok = False

    n = lit_sz // 8
    pool = data[dat_off : dat_off + dat_sz]
    literals = []
    bad_utf8 = 0
    in_range = 0
    step_exact = 0
    prev_end = 0
    for i in range(n):
        ln, di = struct.unpack_from("<2i", data, lit_off + i * 8)
        if ln <= 0 or di < 0:
            continue
        in_range += 1
        if di == prev_end:
            step_exact += 1
        prev_end = di + ln
        raw = pool[di : di + ln]
        try:
            literals.append(raw.decode("utf-8"))
        except UnicodeDecodeError:
            bad_utf8 += 1

    total = in_range
    print(f"\n字面量条数: {n}（有效 {total}）")
    print(f"  可解 UTF-8: {total - bad_utf8}/{total}"
          f"  ({100.0 * (total - bad_utf8) / max(total, 1):.2f}%)")
    print(f"  dataIndex 恰好按 len 步进: {step_exact}/{total}"
          "  （非 100% 说明长度/起始解读有问题）")

    names = [x.decode("utf-8", "replace") for x in data[str_off : str_off + str_sz].split(b"\0") if x]
    print(f"  名字堆条数: {len(names)}   首串: {names[0]!r}" if names else "  名字堆为空")

    hits = sorted({s for s in names if "gallery" in s.lower()})
    print(f"  名字里含 gallery 的: {len(hits)} 个，例: {hits[:6]}")

    if dump:
        print(f"\n前 {dump} 条字面量:")
        for s in literals[:dump]:
            print(f"  {s!r}")

    if not ok:
        return 2
    # 正确解读的关键判据：UTF-8 可解率接近 100%
    ratio = (total - bad_utf8) / max(total, 1)
    if ratio < 0.9:
        print("\n✘ 可解率 <90%，布局解读与预期不符")
        return 2
    print("\n✔ 布局与解读一致")
    return 0


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)
    n_dump = 0
    if "--dump" in sys.argv:
        i = sys.argv.index("--dump")
        n_dump = int(sys.argv[i + 1]) if i + 1 < len(sys.argv) else 20
    sys.exit(peek(sys.argv[1], n_dump))
