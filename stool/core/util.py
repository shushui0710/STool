"""通用工具函数与进度回调约定。"""
from __future__ import annotations

import shutil
import time
from pathlib import Path
from typing import Callable, Optional

# 进度回调：progress(fraction: float 0~1, message: str)
ProgressFn = Callable[[float, str], None]


def null_progress(fraction: float, message: str = "") -> None:
    pass


class ProgressReporter:
    """带节流与阶段记忆的进度上报器。"""

    def __init__(self, cb: Optional[ProgressFn] = None, min_interval: float = 0.05):
        self.cb = cb or null_progress
        self.min_interval = min_interval
        self._last = 0.0

    def __call__(self, fraction: float, message: str = "") -> None:
        now = time.monotonic()
        if fraction >= 1.0 or now - self._last >= self.min_interval:
            self._last = now
            try:
                self.cb(max(0.0, min(1.0, fraction)), message)
            except Exception:
                pass

    def phase(self, start: float, end: float, message: str = "") -> "PhaseSpan":
        return PhaseSpan(self, start, end, message)


class PhaseSpan:
    """把一段子进度映射到总进度区间，支持 with 语法。"""

    def __init__(self, reporter: ProgressReporter, start: float, end: float, message: str):
        self.r = reporter
        self.start, self.end, self.message = start, end, message

    def __enter__(self) -> "PhaseSpan":
        return self

    def __exit__(self, *exc) -> None:
        self.r(self.end, self.message)

    def __call__(self, sub: float, message: str = "") -> None:
        sub = max(0.0, min(1.0, sub))
        frac = self.start + (self.end - self.start) * sub
        self.r(frac, message or self.message)


def iter_files(root: Path, pattern: str = "**/*"):
    for p in sorted(root.glob(pattern)):
        if p.is_file():
            yield p


def safe_out_path(out_dir: Path, rel) -> Path:
    """把封包内相对路径安全映射到输出目录（防路径穿越/非法字符），并确保父目录存在。"""
    rel = str(rel).replace("\\", "/")
    parts = [p for p in rel.split("/") if p not in ("", ".", "..")]
    if not parts:
        parts = ["unnamed"]
    path = out_dir.joinpath(*parts)
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def write_bytes_checked(path: Path, data: bytes) -> int:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return len(data)


def copy_tree(src: Path, dst: Path, progress: ProgressFn = null_progress):
    files = list(src.rglob("*"))
    total = len(files) or 1
    for i, p in enumerate(files):
        if p.is_file():
            rel = p.relative_to(src)
            target = dst / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(p, target)
        progress((i + 1) / total, f"复制 {p.name}")


def format_size(n: int) -> str:
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if n < 1024 or unit == "TB":
            return f"{n:.1f} {unit}" if unit != "B" else f"{n} B"
        n /= 1024.0
    return f"{n} B"


def backup_file(path: Path, suffix: str = ".stool.bak") -> Path:
    bak = path.with_suffix(path.suffix + suffix)
    if not bak.exists():
        shutil.copy2(path, bak)
    return bak
