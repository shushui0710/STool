"""HTML / JS / Electron 游戏插件：asar 解包封包、明文 JS 美化提示。"""
from __future__ import annotations

import json
import struct
from pathlib import Path

from ..core.formats import asar_parse, asar_read_file
from ..core.plugin import Detection, OpResult
from ..core.util import ProgressFn, ProgressReporter, safe_out_path, backup_file
from .base import EngineBase


def asar_pack(src_dir: Path, out: Path) -> None:
    """把目录打包为 asar。"""
    files = {}
    for p in sorted(src_dir.rglob("*")):
        if p.is_file():
            rel = str(p.relative_to(src_dir)).replace("\\", "/")
            files[rel] = p
    header = {"files": {}}
    for rel, p in files.items():
        node = header["files"]
        parts = rel.split("/")
        for part in parts[:-1]:
            node = node.setdefault(part, {}).setdefault("files", {})
        node[parts[-1]] = {"size": p.stat().st_size, "offset": "0"}
    # 先序列化拿总头长
    def fill_offsets(node: dict):
        off = 0
        for name, child in node.items():
            if "files" in child:
                off += fill_offsets(child["files"])
            else:
                child["offset"] = str(off)
                off += child["size"]
        return off
    fill_offsets(header["files"])
    header_str = json.dumps(header, ensure_ascii=False)
    # 头部按 4 字节对齐填充
    pad = (4 - len(header_str) % 4) % 4
    header_str += " " * pad
    json_size = len(header_str.encode("utf-8"))
    with open(out, "wb") as f:
        f.write(struct.pack("<I", 4))
        f.write(struct.pack("<I", json_size + 8))
        f.write(struct.pack("<I", json_size + 8))
        f.write(struct.pack("<I", json_size))
        f.write(header_str.encode("utf-8"))
        for rel, p in files.items():
            f.write(p.read_bytes())


class HtmlGamePlugin(EngineBase):
    id = "html_game"
    name = "HTML / JS / Electron"
    priority = 55

    def detect(self, root: Path) -> Detection:
        ev, score = [], 0
        asars = list(root.glob("**/resources/app.asar")) + list(root.glob("*.asar"))
        if asars:
            score += 70; ev.append(f"{len(asars)} 个 app.asar")
        if (root / "index.html").exists():
            score += 50; ev.append("index.html")
        if (root / "resources").is_dir() and list(root.glob("*.exe")):
            score += 30; ev.append("Electron 结构 (resources/ + exe)")
        return Detection(notes="asar" if asars else "明文", score=score, evidence=ev)

    def describe(self, root: Path) -> str:
        return "Electron asar" if list(root.glob("**/resources/app.asar")) else "明文前端代码"

    def do_extract(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        asars = list(root.glob("**/resources/app.asar")) + list(root.glob("*.asar"))
        if not asars:
            return OpResult(False, "未找到 asar（明文 HTML 游戏无需解包，直接编辑）")
        rep = ProgressReporter(progress)
        done = 0
        for i, arc in enumerate(asars[:5]):
            data = arc.read_bytes()
            try:
                files, _, data_start = asar_parse(data)
            except Exception as e:
                return OpResult(False, f"{arc.name} 解析失败: {e}")
            with rep.phase(i / len(asars), (i + 1) / len(asars), arc.name):
                sub = out_dir / arc.stem
                names = list(files.keys())
                for j, name in enumerate(names):
                    try:
                        safe_out_path(sub, name).write_bytes(asar_read_file(data, data_start, files[name]))
                        done += 1
                    except Exception:
                        pass
                    rep(j / max(1, len(names)), name)
        return OpResult(True, f"解包 {done} 个文件 → {out_dir}", files_done=done)

    def do_repack(self, root: Path, src_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        target = Path(options.get("out_asar") or (root / "resources" / "app.asar"))
        if target.exists() and target.parent in [root / "resources"]:
            backup_file(target)
        asar_pack(src_dir, target)
        return OpResult(True, f"已打包 → {target}")
