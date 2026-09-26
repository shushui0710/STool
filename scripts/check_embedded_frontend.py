#!/usr/bin/env python3
"""判定「Tauri 打包进 exe 的前端是不是当前源码」。

背景（为什么需要它）：
    `tauri-build` 把 `frontendDist` 的每个文件**用 brotli 压缩**后写成
    `target/<profile>/build/stool-tauri-<hash>/out/tauri-codegen-assets/<sha256>.json`
    （文件名是**压缩后**字节的 sha256，内容也是压缩的）。
    所以：
      - `grep -a <中文串> stool-tauri.exe` **永远 0 命中**（内容被压缩）；
      - `sha256sum <源文件>` 也**对不上**资产文件名（哈希算的是压缩字节）。
    只比 mtime 是旁证，不是证据。本脚本把资产**解压**后跟源文件逐字节比 sha256 —— 这才是硬证据。

依赖：
    brotli（装在项目隔离 venv 里：C:\\Users\\86136\\.workbuddy\\binaries\\python\\envs\\default）
        ...\\envs\\default\\Scripts\\python.exe -m pip install brotli

用法：
    python check_embedded_frontend.py                      # 自动找最新构建产物 + 比对 src/ 下全部前端文件
    python check_embedded_frontend.py --profile release
    python check_embedded_frontend.py --assets <目录> --src <目录>
退出码：0 = 内嵌前端与源码逐字节一致；1 = 有源文件没出现在资产里（前端没被重嵌）。
"""

from __future__ import annotations

import argparse
import hashlib
import sys
from pathlib import Path

try:
    import brotli
except ModuleNotFoundError:  # pragma: no cover
    sys.exit(
        "缺少 brotli 模块。用项目隔离 venv 装：\n"
        r"  C:\Users\86136\.workbuddy\binaries\python\envs\default\Scripts\python.exe -m pip install brotli"
    )

REPO = Path(__file__).resolve().parents[1]          # D:\STool
DEFAULT_SRC = REPO / "stool-tauri" / "src"
TAURI = REPO / "stool-tauri" / "src-tauri"

# 前端静态资产（相对 src/）。改页面时按需加。
FRONTEND_FILES = [
    "index.html",
    "css/tokens.css",
    "css/app.css",
    "js/app.js",
]


def frontend_files(src: Path) -> list[str]:
    want = [p for p in FRONTEND_FILES if (src / p).exists()]
    want += sorted(
        str(p.relative_to(src)).replace("\\", "/")
        for p in (src / "js").rglob("*.js")
        if "js/app.js" not in str(p).replace("\\", "/")
    )
    return want


def newest_assets(profile: str) -> Path:
    """找最新一次构建写出的资产目录（同一个 hash 目录会被复用，看 mtime）。"""
    root = TAURI / "target" / profile / "build"
    if not root.is_dir():
        sys.exit(f"找不到构建目录: {root}（先跑 cargo build --{profile}）")
    cands = [d for d in root.glob("stool-tauri-*/out/tauri-codegen-assets") if d.is_dir()]
    if not cands:
        sys.exit(f"{root} 下没有 tauri-codegen-assets（先跑 cargo build --{profile}）")
    return max(cands, key=lambda d: d.stat().st_mtime)


def decompress_all(assets: Path) -> dict[str, bytes]:
    """资产文件名 → 解压后的原始字节。解不开的（非 brotli）跳过。"""
    out: dict[str, bytes] = {}
    for f in sorted(assets.iterdir()):
        if f.is_file():
            try:
                out[f.name] = brotli.decompress(f.read_bytes())
            except Exception:
                continue
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--profile", default="release", choices=["release", "debug"])
    ap.add_argument("--assets", type=Path, default=None)
    ap.add_argument("--src", type=Path, default=DEFAULT_SRC)
    a = ap.parse_args()

    assets = a.assets or newest_assets(a.profile)
    src: Path = a.src
    exe = TAURI / "target" / a.profile / "stool-tauri.exe"

    print(f"资产目录 : {assets}")
    print(f"源目录   : {src}")
    if exe.exists():
        st = exe.stat()
        import datetime

        print(
            f"exe      : {exe}  {st.st_size:,} 字节  "
            f"{datetime.datetime.fromtimestamp(st.st_mtime):%Y-%m-%d %H:%M:%S}"
        )
    print()

    blobs = decompress_all(assets)
    plain = {k: v for k, v in blobs.items()}          # 解压成功的
    print(f"资产文件 {len(list(assets.iterdir()))} 个，其中可 brotli 解压 {len(plain)} 个\n")

    # 解压后的 sha256 → 资产文件名（可能有多个同名内容）
    by_hash: dict[str, str] = {}
    for name, raw in plain.items():
        by_hash.setdefault(hashlib.sha256(raw).hexdigest(), name)

    missing, matched = [], 0
    for rel in frontend_files(src):
        p = src / rel
        raw = p.read_bytes()
        h = hashlib.sha256(raw).hexdigest()
        name = by_hash.get(h)
        if name:
            matched += 1
            print(f"✔ {rel}")
            print(f"    资产 {name}  （压缩 { (assets / name).stat().st_size } / 原始 {len(raw)} 字节）")
        else:
            missing.append(rel)
            print(f"✘ {rel}  —— 源码 sha256 {h[:16]}… 在资产里找不到")

    print()
    if missing:
        print(f"结论：{len(missing)} 个源文件未被重嵌 → 前端是旧的，重跑 cargo build --{a.profile}")
        print("      （tauri-build 不 watch frontendDist；build.rs 已补 rerun-if-changed=../src）")
        return 1
    print(f"结论：{matched} 个前端文件与源码逐字节一致 → exe 内嵌的是当前前端。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
