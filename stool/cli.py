"""STool 命令行入口。

用法示例：
  python -m stool.cli detect <游戏目录>
  python -m stool.cli extract <游戏目录> -o 输出目录 [-p 插件id]
  python -m stool.cli decompile <游戏目录> -o 输出目录
  python -m stool.cli text-extract <游戏目录> -o 文本.csv
  python -m stool.cli text-import <游戏目录> 文本.csv
  python -m stool.cli save-decode <游戏目录> -o 输出目录
  python -m stool.cli mod-install <游戏目录> <补丁目录>
  python -m stool.cli gui
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from . import __version__
from .core.plugin import PluginRegistry
from .core.job import make_job
from .core.util import null_progress


def _reg() -> PluginRegistry:
    reg = PluginRegistry(extra_dirs=[Path(__file__).parent / "plugins"])
    reg.discover()
    return reg


def _pick(reg: PluginRegistry, root: Path, plugin_id: str | None):
    det = reg.detect_all(root)
    if not det:
        print("没有可用插件"); sys.exit(1)
    if plugin_id:
        d = next((x for x in det if x.plugin_id == plugin_id), None)
        if not d:
            print(f"未知插件 {plugin_id}"); sys.exit(1)
    else:
        d = det[0]
    print(f"判定引擎: {d.name} (置信度 {d.score})")
    for e in d.evidence:
        print(f"  · {e}")
    return d


def main(argv=None):
    ap = argparse.ArgumentParser(prog="stool", description=f"STool v{__version__} 多引擎游戏综合工具")
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("detect", help="引擎检测")
    p.add_argument("root")
    p.add_argument("--json", action="store_true", dest="as_json")

    def _std(name, help_):
        s = sub.add_parser(name, help=help_)
        s.add_argument("root")
        s.add_argument("-o", "--out", default="")
        s.add_argument("-p", "--plugin", default="")
        s.add_argument("--opt", action="append", default=[], help="key=value 选项")
        return s

    _std("extract", "解包资源")
    _std("repack", "封包/回写（--opt src_dir=目录）")
    _std("decompile", "脚本反编译")
    _std("text-extract", "对话文本提取 → CSV")
    _std("text-import", "CSV 翻译回填")
    _std("save", "存档解码/处理")
    _std("unlock", "解锁辅助（如 Ren'Py persistent）")

    p = sub.add_parser("mod-install", help="安装补丁/MOD")
    p.add_argument("root"); p.add_argument("patch_dir"); p.add_argument("-n", "--name", default="")
    p = sub.add_parser("mod-list", help="列出 MOD")
    p.add_argument("root")
    p = sub.add_parser("mod-uninstall", help="卸载 MOD")
    p.add_argument("root"); p.add_argument("name")
    p = sub.add_parser("gui", help="启动图形界面")

    args = ap.parse_args(argv)
    root = Path(args.root).resolve() if hasattr(args, "root") else None

    def _opts(extra):
        d = {}
        for kv in extra or []:
            k, _, v = kv.partition("=")
            d[k] = v
        return d

    if args.cmd == "gui":
        from .gui.main_window import run_gui
        return run_gui()

    if args.cmd == "detect":
        reg = _reg()
        results = reg.detect_all(root)
        if args.as_json:
            print(json.dumps([{k: getattr(d, k) for k in ("plugin_id", "name", "score", "evidence")}
                              for d in results], ensure_ascii=False, indent=2))
        else:
            for d in results:
                mark = "★" if d.ok() else " "
                print(f"{mark} {d.name:<28} 置信度 {d.score:<4} {'; '.join(d.evidence)}")
        return

    if args.cmd and args.cmd.startswith("mod"):
        from .features import mods
        if args.cmd == "mod-install":
            entry = mods.install_mod(root, Path(args.patch_dir), args.name)
            print(f"已安装 MOD '{entry['name']}'：{len(entry['files'])} 个文件，覆盖 {len(entry['overwritten'])} 个原文件")
        elif args.cmd == "mod-list":
            for m in mods.list_mods(root):
                print(f"{'[启用]' if m['enabled'] else '[停用]'} {m['name']} — {len(m['files'])} 文件")
        elif args.cmd == "mod-uninstall":
            mods.uninstall_mod(root, args.name)
            print(f"已卸载并还原: {args.name}")
        return

    # 标准操作
    reg = _reg()
    d = _pick(reg, root, args.plugin or None)
    if not d.ok() and not args.plugin:
        print("⚠ 置信度不足，请用 -p 指定引擎或人工确认")
        if args.cmd != "detect":
            return 1
    out = Path(args.out) if args.out else Path(f"{root}_stool_out")
    opmap = {"extract": "extract", "repack": "repack", "decompile": "decompile",
             "text-extract": "text_extract", "text-import": "text_import",
             "save": "save", "unlock": "unlock"}
    options = _opts(args.opt)
    if args.cmd == "text-import":
        options["csv"] = str(args.out or "text.csv")
    job = make_job(reg, d.plugin_id, opmap[args.cmd], root,
                   out if args.cmd != "text-extract" else Path(args.out or "text.csv"),
                   options)
    result = job.run(null_progress)
    print(("✔ " if result.success else "✘ ") + result.message)
    for line in job.logs[-10:]:
        print("  ·", line)
    return 0 if result.success else 1


if __name__ == "__main__":
    sys.exit(main())
