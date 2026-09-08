"""外部工具与用户配置路径管理。"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict

APP_DIR = Path(__file__).resolve().parent.parent.parent  # D:\STool
CONFIG_PATH = Path.home() / ".stool" / "config.json"
DEFAULTS: Dict[str, Any] = {
    "output_dir": str(APP_DIR / "output"),
    "garbro": "",            # GARbro CLI (ArcConv/GARbro.exe)
    "asset_ripper": "",      # AssetRipper CLI
    "il2cpp_dumper": "",
    "wolfdec": "",           # WolfDec.exe
    "gdre_tools": "",        # GDRE Tools (gdsdecomp)
    "unrpyc": "",            # unrpyc.py 路径，默认用捆绑副本
    "proxy": "http://127.0.0.1:7892",
}


def load_config() -> Dict[str, Any]:
    cfg = dict(DEFAULTS)
    try:
        if CONFIG_PATH.exists():
            cfg.update(json.loads(CONFIG_PATH.read_text("utf-8")))
    except Exception:
        pass
    return cfg


def save_config(cfg: Dict[str, Any]) -> None:
    CONFIG_PATH.parent.mkdir(parents=True, exist_ok=True)
    CONFIG_PATH.write_text(json.dumps(cfg, ensure_ascii=False, indent=2), "utf-8")


def bundled_unrpyc() -> Path:
    """skill 捆绑的 unrpyc 仓库中的主脚本。"""
    candidates = [
        APP_DIR / "tools" / "unrpyc" / "unrpyc.py",
        Path.home() / ".workbuddy" / "skills" / "game-unpacker" / "scripts" / "unrpyc" / "unrpyc.py",
    ]
    for c in candidates:
        if c.exists():
            return c
    return candidates[0]
