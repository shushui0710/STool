"""Wolf RPG Editor / Unity 插件：检测 + 外部工具委派（WolfDec / AssetRipper）。"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

from ..core.plugin import Detection, OpResult
from ..core.util import ProgressFn
from ..core.paths import load_config
from .base import EngineBase


class WolfPlugin(EngineBase):
    id = "wolf"
    name = "Wolf RPG Editor (ウディタ)"
    priority = 72

    def detect(self, root: Path) -> Detection:
        ev, score = [], 0
        wolves = list(root.glob("Data/*.wolf"))
        if wolves:
            score += 75; ev.append(f"{len(wolves)} 个 .wolf 封包")
        if (root / "Game.exe").exists() and wolves:
            score += 10
        return Detection(notes="", score=score, evidence=ev)

    def describe(self, root: Path) -> str:
        return f"封包: {[p.name for p in root.glob('Data/*.wolf')][:3]}"

    def do_extract(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        cfg = load_config()
        wolfdec = cfg.get("wolfdec")
        if not wolfdec or not Path(wolfdec).exists():
            return OpResult(False, "未配置 WolfDec 路径（设置页填写 WolfDec.exe 后重试；"
                                   "下载: github.com/Cherrydatable/WolfDec 等发行版）")
        r = subprocess.run([wolfdec, "-o", str(out_dir), str(root)],
                           capture_output=True, text=True)
        if r.returncode == 0 or "complete" in r.stdout.lower():
            return OpResult(True, f"WolfDec 解包完成 → {out_dir}")
        return OpResult(False, f"WolfDec 失败: {(r.stderr or r.stdout)[-400:]}")


class UnityPlugin(EngineBase):
    id = "unity"
    name = "Unity"
    priority = 78

    def detect(self, root: Path) -> Detection:
        ev, score = [], 0
        data_dirs = list(root.glob("*_Data"))
        if data_dirs or (root / "UnityPlayer.dll").exists():
            score += 75; ev.append("UnityPlayer.dll / *_Data")
        if data_dirs and list(data_dirs[0].glob("Managed/Assembly-CSharp.dll")):
            ev.append("Mono 版（Assembly-CSharp.dll）")
        elif list(root.glob("*/GameAssembly.dll")) or list(root.glob("*_Data/GameAssembly.dll")):
            ev.append("IL2CPP 版（GameAssembly.dll）")
        return Detection(notes="", score=score, evidence=ev)

    def describe(self, root: Path) -> str:
        mono = list(root.glob("*_Data/Managed/Assembly-CSharp.dll"))
        return "Mono 版" if mono else "IL2CPP 版（或未知）"

    def do_extract(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        cfg = load_config()
        ripper = cfg.get("asset_ripper")
        if not ripper or not Path(ripper).exists():
            return OpResult(False, "未配置 AssetRipper 路径（设置页填写 AssetRipper.cli/win 后重试；"
                                   "Mono 版亦可用 dnSpy 直接改 Assembly-CSharp.dll）")
        r = subprocess.run([ripper, "-i", str(root), "-o", str(out_dir)],
                           capture_output=True, text=True)
        if r.returncode == 0:
            return OpResult(True, f"AssetRipper 导出完成 → {out_dir}")
        return OpResult(False, f"AssetRipper 失败: {(r.stderr or r.stdout)[-400:]}")
