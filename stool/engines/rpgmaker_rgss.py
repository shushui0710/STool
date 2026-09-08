"""RPG Maker XP / VX / VX Ace（RGSS）插件：rgssad/2a/3a 解包、Scripts 反编译、存档查看。"""
from __future__ import annotations

import csv
import io
import json
import re
from pathlib import Path

from ..core.formats import rgss1_decrypt, rgss1_extract_file, rgss3_parse, rgss3_extract_file, rgss_scripts_extract
from ..core.plugin import Detection, OpResult
from ..core.util import ProgressFn, ProgressReporter, safe_out_path
from .base import EngineBase

RGSS_KIND = {".rgssad": "RGSS1", ".rgss2a": "RGSS1", ".rgss3a": "RGSS3"}


class RpgMakerRgssPlugin(EngineBase):
    id = "rpgmaker_rgss"
    name = "RPG Maker XP / VX / VX Ace"
    priority = 80

    def detect(self, root: Path) -> Detection:
        ev, score, kind = [], 0, ""
        for ext, k in RGSS_KIND.items():
            found = list(root.glob(f"*{ext}"))
            if found:
                score += 70; kind = k
                ev.append(f"{found[0].name} 封包")
        for d in ("Data", "Graphics", "Audio"):
            if (root / d).is_dir():
                score += 10; ev.append(f"{d}/ 目录")
        if list(root.glob("Data/Scripts.r*data*")):
            score += 15; ev.append("Data/Scripts 数据脚本")
        return Detection(notes=kind or "", score=score, evidence=ev)

    def describe(self, root: Path) -> str:
        arcs = [p.name for p in root.glob("*.rgssa*") if p.is_file()]
        return f"封包: {arcs}"

    def _iter_archives(self, root: Path):
        for p in sorted(root.glob("*.rgssa*")):
            if p.suffix in RGSS_KIND:
                yield p, RGSS_KIND[p.suffix]

    def do_extract(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        archives = list(self._iter_archives(root))
        if not archives:
            return OpResult(False, "未找到 RGSS 封包")
        rep = ProgressReporter(progress)
        done = 0
        for i, (arc, kind) in enumerate(archives):
            data = arc.read_bytes()
            with rep.phase(i / len(archives), (i + 1) / len(archives), arc.name):
                if kind == "RGSS1":
                    entries = rgss1_decrypt(data)
                    names = list(entries.keys())
                    for j, name in enumerate(names):
                        off, size, _ = entries[name]
                        out = safe_out_path(out_dir, name)
                        out.write_bytes(rgss1_extract_file(data, off, size))
                        done += 1
                        rep(j / max(1, len(names)), name)
                else:
                    entries = rgss3_parse(data)
                    for j, (name, off, size, fkey) in enumerate(entries):
                        out = safe_out_path(out_dir, name)
                        out.write_bytes(rgss3_extract_file(data, off, size, fkey))
                        done += 1
                        rep(j / max(1, len(entries)), name)
        return OpResult(True, f"解包 {done} 个文件 → {out_dir}", files_done=done)

    def do_decompile(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        scripts = list(root.glob("Data/Scripts.r*data*"))
        if not scripts:
            return OpResult(False, "未找到 Data/Scripts.r*data*")
        data = scripts[0].read_bytes()
        try:
            codes = rgss_scripts_extract(data)
        except Exception as e:
            return OpResult(False, f"Ruby Marshal 解析失败: {e}")
        out = out_dir / "scripts"
        out.mkdir(parents=True, exist_ok=True)
        for i, raw in enumerate(codes):
            (out / f"{i:03d}.rb").write_bytes(raw)
        return OpResult(True, f"提取 {len(codes)} 个 Ruby 脚本 → {out}", files_done=len(codes))

    def do_text_extract(self, root: Path, out_csv: Path, options: dict, progress: ProgressFn) -> OpResult:
        """从提取出的 .rb 脚本与地图/事件 Marshal 中抽取 CJK 文本（尽力而为）。"""
        base = Path(options.get("rb_dir") or (root / "scripts"))
        if not base.is_dir():
            return OpResult(False, f"{base} 不存在，请先提取脚本")
        cjk = re.compile(r'[\u3400-\u9fff\uf900-\ufaff\u3040-\u30ff]{2,}')
        rows = []
        for i, p in enumerate(sorted(base.rglob("*.rb"))):
            try:
                text = p.read_text("utf-8", errors="replace")
            except Exception:
                continue
            for ln, line in enumerate(text.splitlines(), 1):
                for m in cjk.finditer(line):
                    rows.append({"id": f"{p.name}:{ln}:{m.start()}", "file": p.name,
                                 "context": "ruby", "source": m.group(0), "translation": ""})
            progress((i + 1) / max(1, len(list(base.rglob('*.rb')))), p.name)
        out_csv.parent.mkdir(parents=True, exist_ok=True)
        with open(out_csv, "w", newline="", encoding="utf-8-sig") as f:
            w = csv.DictWriter(f, fieldnames=["id", "file", "context", "source", "translation"])
            w.writeheader(); w.writerows(rows)
        return OpResult(True, f"提取 {len(rows)} 条含 CJK 文本 → {out_csv}", files_done=len(rows))

    def do_save(self, root: Path, options: dict, progress: ProgressFn) -> OpResult:
        saves = list(root.glob("Save*.rxdata")) + list(root.glob("Save*.rvdata*"))
        if not saves:
            return OpResult(False, "未找到 SaveNN.rxdata/rvdata 存档")
        return OpResult(True, f"发现 {len(saves)} 个 Marshal 存档（用 Ruby 或专用编辑器处理，工具给出位置）",
                        detail=[str(s) for s in saves])
