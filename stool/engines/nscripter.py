"""NScripter / ONScripter 插件：nscript.dat 解密。"""
from __future__ import annotations

from pathlib import Path

from ..core.formats import nscript_decode
from ..core.plugin import Detection, OpResult
from ..core.util import ProgressFn
from .base import EngineBase


class NscripterPlugin(EngineBase):
    id = "nscripter"
    name = "NScripter / ONScripter"
    priority = 65

    def detect(self, root: Path) -> Detection:
        ev, score = [], 0
        if (root / "nscript.dat").exists():
            score += 70; ev.append("nscript.dat 加密脚本")
        elif list(root.glob("nscript?.dat")) or list(root.glob("Script.dat")):
            score += 60; ev.append("nscript 变体脚本")
        nsas = list(root.glob("*.nsa"))
        if nsas:
            score += 20; ev.append(f"{len(nsas)} 个 .nsa 资源包")
        if (root / "ONScripter.exe").exists() or any("ons" in p.name.lower() for p in root.glob("*.exe")):
            score += 15; ev.append("ONScripter 运行时")
        return Detection(notes="", score=score, evidence=ev)

    def describe(self, root: Path) -> str:
        return "nscript.dat" if (root / "nscript.dat").exists() else ""

    def do_extract(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        src = root / "nscript.dat"
        if not src.exists():
            return OpResult(False, "未找到 nscript.dat")
        out = out_dir / "nscript.txt"
        out.write_bytes(nscript_decode(src.read_bytes()))
        return OpResult(True, f"脚本已解密 → {out}（若内容乱码说明是特殊加密变体）")
