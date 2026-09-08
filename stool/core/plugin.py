"""插件架构：EnginePlugin 基类、检测结果、能力声明、注册表与外部插件加载。"""
from __future__ import annotations

import importlib
import importlib.util
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Dict, List, Optional, Type

from .util import ProgressFn


@dataclass
class Detection:
    """单个引擎插件对某游戏目录的判定结果。"""
    plugin_id: str = ""
    name: str = ""
    score: int = 0            # 0~100，>=60 视为命中
    evidence: List[str] = field(default_factory=list)
    notes: str = ""           # 版本/变体等补充说明

    def ok(self) -> bool:
        return self.score >= 60


@dataclass
class OpResult:
    """一次操作的执行结果。"""
    success: bool = True
    message: str = ""
    files_done: int = 0
    detail: List[str] = field(default_factory=list)


class Capability:
    """能力名常量。"""
    EXTRACT = "extract"          # 解包资源
    REPACK = "repack"            # 封包/回写
    DECOMPILE = "decompile"      # 脚本反编译
    TEXT_EXTRACT = "text_extract"  # 对话文本提取（本地化）
    TEXT_IMPORT = "text_import"    # 翻译文本回填
    SAVE = "save"                # 存档查看/编辑
    UNLOCK = "unlock"            # 全 CG/回想等解锁辅助


CAP_LABELS = {
    Capability.EXTRACT: "资源解包",
    Capability.REPACK: "封包/回写",
    Capability.DECOMPILE: "脚本反编译",
    Capability.TEXT_EXTRACT: "文本提取",
    Capability.TEXT_IMPORT: "翻译回填",
    Capability.SAVE: "存档编辑",
    Capability.UNLOCK: "解锁辅助",
}


class EnginePlugin:
    """引擎插件基类。子类放在 stool/engines/ 或 plugins/ 目录即可被自动加载。"""
    id: str = "base"
    name: str = "未命名引擎"
    priority: int = 0          # 同分时大者优先

    def __init__(self, host: "PluginRegistry"):
        self.host = host

    # ---- 必须实现 ----
    def detect(self, root: Path) -> Detection:
        raise NotImplementedError

    # ---- 按能力实现（未实现的能力会从可用列表中消失）----
    def capabilities(self) -> List[str]:
        caps = []
        for cap in ("do_" + c for c in CAP_LABELS):
            if callable(getattr(self, cap, None)):
                caps.append(cap[3:])
        return caps

    def describe(self, root: Path) -> str:
        return ""

    # ---- 能力方法默认实现（NotImplementedError 即视为未提供）----
    def do_extract(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        raise NotImplementedError

    def do_repack(self, root: Path, src_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        raise NotImplementedError

    def do_decompile(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        raise NotImplementedError

    def do_text_extract(self, root: Path, out_csv: Path, options: dict, progress: ProgressFn) -> OpResult:
        raise NotImplementedError

    def do_text_import(self, root: Path, csv_path: Path, options: dict, progress: ProgressFn) -> OpResult:
        raise NotImplementedError

    def do_save(self, root: Path, options: dict, progress: ProgressFn) -> OpResult:
        raise NotImplementedError

    def do_unlock(self, root: Path, options: dict, progress: ProgressFn) -> OpResult:
        raise NotImplementedError


class PluginRegistry:
    """插件注册表：加载内置引擎插件 + 用户插件目录的热插拔。"""

    def __init__(self, extra_dirs: Optional[List[Path]] = None):
        self.plugins: Dict[str, EnginePlugin] = {}
        self.extra_dirs = [Path(p) for p in (extra_dirs or [])]
        self.load_errors: List[str] = []

    def register(self, cls: Type[EnginePlugin]) -> None:
        plugin = cls(self)
        self.plugins[plugin.id] = plugin

    def discover(self) -> None:
        """扫描内置 engines 包与用户插件目录。"""
        self.plugins.clear()
        self.load_errors.clear()
        # 内置
        import stool.engines as pkg
        pkg_dir = Path(pkg.__file__).parent
        for f in sorted(pkg_dir.glob("*.py")):
            if f.stem.startswith("_"):
                continue
            mod_name = f"stool.engines.{f.stem}"
            try:
                mod = importlib.import_module(mod_name)
                importlib.reload(mod)
                self._collect_from(mod)
            except Exception as e:  # 单个插件坏了不影响其他
                self.load_errors.append(f"{f.name}: {e}")
        # 用户插件
        for d in self.extra_dirs:
            if not d.is_dir():
                continue
            for f in sorted(d.glob("*.py")):
                if f.stem.startswith("_"):
                    continue
                try:
                    spec = importlib.util.spec_from_file_location(f"stool_user_{f.stem}", f)
                    mod = importlib.util.module_from_spec(spec)
                    sys.modules[spec.name] = mod
                    spec.loader.exec_module(mod)
                    self._collect_from(mod)
                except Exception as e:
                    self.load_errors.append(f"{f}: {e}")
        # 注册基线兜底插件
        from stool.engines.generic import GenericPlugin
        self.register(GenericPlugin)

    def _collect_from(self, mod) -> None:
        for attr in dir(mod):
            obj = getattr(mod, attr)
            if (isinstance(obj, type) and issubclass(obj, EnginePlugin)
                    and obj is not EnginePlugin and obj.__module__ == mod.__name__):
                self.register(obj)

    def get(self, plugin_id: str) -> Optional[EnginePlugin]:
        return self.plugins.get(plugin_id)

    def detect_all(self, root: Path) -> List[Detection]:
        results = []
        for p in self.plugins.values():
            try:
                d = p.detect(root)
            except Exception as e:
                d = Detection(p.id, p.name, 0, [f"检测异常: {e}"])
            d.plugin_id, d.name = p.id, p.name
            results.append(d)
        results.sort(key=lambda d: (d.score, self.plugins[d.plugin_id].priority), reverse=True)
        return results

    def best(self, root: Path) -> Optional[Detection]:
        res = self.detect_all(root)
        return res[0] if res and res[0].ok() else None
