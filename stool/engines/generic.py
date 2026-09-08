"""通用兜底插件：未知引擎时给出排查建议与基础文件浏览。"""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

from ..core.plugin import Detection, OpResult
from ..core.util import ProgressFn
from .base import EngineBase

# 常见封包扩展名 → 提示
KNOWN_EXT = {
    ".xp3": "KiriKiri", ".rpa": "Ren'Py", ".rgssad": "RPG Maker XP",
    ".rgss2a": "RPG Maker VX", ".rgss3a": "RPG Maker VX Ace",
    ".wolf": "Wolf RPG", ".nsa": "NScripter", ".pck": "Godot",
    ".asar": "Electron", ".aqp": "AdvSys/ARCG", ".pac": "通用封包",
    ".arc": "通用封包", ".dat": "通用数据", ".pak": "通用封包",
    ".bin": "通用二进制", ".pfs": "Ethornell/BGI", ".scx": "ScenePlayer",
    ".mjo": "Mink", ".int": "Interlude", ".dpm": "Donut", ".noa": "Tactics",
}


class GenericPlugin(EngineBase):
    id = "generic"
    name = "未知引擎（兜底）"
    priority = -100

    def detect(self, root: Path) -> Detection:
        return Detection(score=10, evidence=["未匹配到已知引擎特征"])

    def describe(self, root: Path) -> str:
        hits = {}
        for p in root.rglob("*"):
            if p.is_file() and p.suffix.lower() in KNOWN_EXT:
                hits.setdefault(KNOWN_EXT[p.suffix.lower()], []).append(p.name)
        if hits:
            return "疑似: " + "; ".join(f"{k}({v[0]})" for k, v in list(hits.items())[:5])
        return "未发现特征封包，建议用 GARbro 打包器逐个尝试"

    def do_extract(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        cfg_root = root
        garbro = load_garbro()
        if garbro and Path(garbro).exists():
            r = subprocess.run([garbro, str(cfg_root), "-o", str(out_dir)],
                               capture_output=True, text=True)
            return OpResult(True, f"GARbro 批量提取完成 → {out_dir}" if r.returncode == 0
                            else f"GARbro 失败: {r.stderr[-300:]}")
        return OpResult(False, "未知格式：请在设置页配置 GARbro CLI 路径，"
                               "或参照报告中的“未知格式排查顺序”人工处理")


def load_garbro():
    from ..core.paths import load_config
    return load_config().get("garbro")
