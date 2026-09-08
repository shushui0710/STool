"""Godot 引擎插件：PCK 解包。"""
from __future__ import annotations

from pathlib import Path

from ..core.formats import pck_parse
from ..core.plugin import Detection, OpResult
from ..core.util import ProgressFn, ProgressReporter, safe_out_path
from .base import EngineBase


class GodotPlugin(EngineBase):
    id = "godot"
    name = "Godot"
    priority = 70

    def detect(self, root: Path) -> Detection:
        ev, score = [], 0
        pcks = list(root.glob("*.pck"))
        if pcks:
            score += 70; ev.append(f"{len(pcks)} 个 .pck")
        if (root / ".godot").is_dir() or (root / "project.godot").exists():
            score += 30; ev.append("project.godot")
        return Detection(notes="", score=score, evidence=ev)

    def describe(self, root: Path) -> str:
        return f"封包: {[p.name for p in root.glob('*.pck')]}"

    def do_extract(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        pcks = list(root.glob("*.pck")) or list(root.glob("**/*.pck"))[:5]
        if not pcks:
            return OpResult(False, "未找到 .pck")
        rep = ProgressReporter(progress)
        done = 0
        for i, pck in enumerate(pcks[:10]):
            data = pck.read_bytes()
            try:
                entries = pck_parse(data)
            except Exception as e:
                return OpResult(False, f"{pck.name} 解析失败: {e}")
            with rep.phase(i / len(pcks), (i + 1) / len(pcks), pck.name):
                sub = out_dir / pck.stem
                for j, (path, off, size) in enumerate(entries):
                    safe_out_path(sub, path.lstrip("/")).write_bytes(data[off:off + size])
                    done += 1
                    rep(j / max(1, len(entries)), path)
        return OpResult(True, f"解包 {done} 个文件 → {out_dir}", files_done=done)
