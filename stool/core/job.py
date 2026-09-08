"""任务执行：把插件操作包成可取消、带进度与日志的任务，GUI/CLI 共用。"""
from __future__ import annotations

import traceback
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, List, Optional

from .plugin import EnginePlugin, PluginRegistry, OpResult
from .util import ProgressReporter


@dataclass
class Job:
    op: str                    # extract / repack / decompile / text_extract / text_import / save / unlock
    plugin: EnginePlugin
    game_root: Path
    out_dir: Path = field(default_factory=lambda: Path("."))
    options: dict = field(default_factory=dict)
    result: Optional[OpResult] = None
    logs: List[str] = field(default_factory=list)
    cancelled: bool = False

    def run(self) -> OpResult:
        rep = ProgressReporter(self._progress)
        fn = getattr(self.plugin, "do_" + self.op, None)
        if fn is None:
            self.result = OpResult(False, f"插件 {self.plugin.id} 不支持 {self.op}")
            return self.result
        try:
            if self.op in ("repack",):
                self.result = fn(self.game_root, Path(self.options["src_dir"]), self.options, rep)
            else:
                self.result = fn(self.game_root, self.out_dir, self.options, rep)
        except Exception as e:
            self.logs.append(traceback.format_exc())
            self.result = OpResult(False, f"执行异常: {e}")
        return self.result

    def _progress(self, fraction: float, message: str) -> None:
        if self.cancelled:
            raise KeyboardInterrupt("任务已取消")
        if message:
            self.logs.append(message)


def make_job(registry: PluginRegistry, plugin_id: str, op: str, game_root: Path,
             out_dir: Path, options: dict | None = None) -> Job:
    plugin = registry.get(plugin_id)
    if not plugin:
        raise ValueError(f"未知插件: {plugin_id}")
    return Job(op=op, plugin=plugin, game_root=Path(game_root), out_dir=Path(out_dir),
               options=options or {})
