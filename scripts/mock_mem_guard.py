#!/usr/bin/env python3
"""反修改保护靶子：起一个进程，把一个已知数值放在**独占的内存页**上，并按指定模式保护它。

用途：**不依赖任何游戏**，直接在真机上验证 STool 的
`guard` / `guard-regions` 诊断与「强制写入 / 自适应锁值」是否判断正确。

用法::

    python scripts/mock_mem_guard.py <mode>

`<mode>`:

* `none`      —— 不保护。期望诊断结论「稳定（写入后被保持）」。
* `periodic`  —— 每 10ms 把数值写回 100。
                 期望判定「周期性回滚 ~10ms」+ 自适应锁值方案（周期 ≈3ms）。
* `fast`      —— 每 1ms 写回。期望判定周期 ≈1ms，或（若实测 <1ms）给出「锁值无效」。
* `constant`  —— 只读页 + 每 1ms 由自己临时放开、写回 100（只读页与周期回滚叠加）。
                 期望报出「页不可写 → 强制写入」**并且**「回滚间隔约 1ms → 锁值无效/勉强可试」。
* `readonly`  —— 只把页设为只读。
                 期望报出「页不可写」+「强制写入（解除页保护）」方案。

脚本会打印一行机器可读的目标信息::

    PID=1234 ADDR=0x1a2b3c4d VALUE=100 MODE=none

然后一直挂着（Ctrl-C 退出）。配合使用::

    stool-cli guard <PID> --opt:addr=<ADDR> --opt:type=i32

注意：数值放在 `VirtualAlloc` 出来的**独占页**里，这样把它设成只读不会波及
Python 自己的堆（否则解释器会立刻崩）。
"""

import ctypes
import os
import sys
import threading
import time

K32 = ctypes.WinDLL("kernel32", use_last_error=True)
K32.VirtualAlloc.restype = ctypes.c_void_p
K32.VirtualAlloc.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32, ctypes.c_uint32]
K32.VirtualProtect.restype = ctypes.c_int
K32.VirtualProtect.argtypes = [
    ctypes.c_void_p,
    ctypes.c_size_t,
    ctypes.c_uint32,
    ctypes.POINTER(ctypes.c_uint32),
]

MEM_COMMIT_RESERVE = 0x3000
PAGE_READWRITE = 0x04
PAGE_READONLY = 0x02
PAGE_SIZE = 4096

INITIAL = 100

# 独占一页，页内只有一个 int32
_BUF = K32.VirtualAlloc(None, PAGE_SIZE, MEM_COMMIT_RESERVE, PAGE_READWRITE)
if not _BUF:
    print(f"VirtualAlloc 失败: {ctypes.get_last_error()}", file=sys.stderr)
    sys.exit(1)
VALUE_ADDR = int(_BUF)
_CELL = ctypes.c_int32.from_address(VALUE_ADDR)
_CELL.value = INITIAL


def _set_page(protect: int) -> bool:
    old = ctypes.c_uint32(0)
    return bool(K32.VirtualProtect(ctypes.c_void_p(VALUE_ADDR), PAGE_SIZE, protect, ctypes.byref(old)))


def _reset_loop(target: int, interval_s: float) -> None:
    """周期性把数值写回 target。"""
    while True:
        if interval_s > 0:
            time.sleep(interval_s)
        _CELL.value = target


def _readonly_reset_loop(target: int, interval_s: float) -> None:
    """把页设为只读，但周期性临时放开、写回 target、再锁上。"""
    while True:
        time.sleep(interval_s)
        _set_page(PAGE_READWRITE)
        _CELL.value = target
        _set_page(PAGE_READONLY)


def main() -> int:
    mode = (sys.argv[1] if len(sys.argv) > 1 else "none").strip().lower()
    if mode not in {"none", "periodic", "fast", "constant", "readonly"}:
        print(f"未知模式: {mode}（可选 none/periodic/fast/constant/readonly）", file=sys.stderr)
        return 2

    if mode == "readonly":
        if not _set_page(PAGE_READONLY):
            print(f"VirtualProtect 失败: {ctypes.get_last_error()}", file=sys.stderr)
            return 1
    elif mode == "constant":
        if not _set_page(PAGE_READONLY):
            print(f"VirtualProtect 失败: {ctypes.get_last_error()}", file=sys.stderr)
            return 1
        threading.Thread(target=_readonly_reset_loop, args=(INITIAL, 0.001), daemon=True).start()
    elif mode == "periodic":
        threading.Thread(target=_reset_loop, args=(INITIAL, 0.01), daemon=True).start()
    elif mode == "fast":
        threading.Thread(target=_reset_loop, args=(INITIAL, 0.001), daemon=True).start()

    # 机器可读的一行，供 shell 解析
    print(f"PID={os.getpid()} ADDR={VALUE_ADDR:#x} VALUE={INITIAL} MODE={mode}", flush=True)

    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
