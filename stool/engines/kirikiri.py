"""KiriKiri (吉里吉里) 插件：XP3 解析提取、KAG 文本提取。"""
from __future__ import annotations

import csv
import re
from pathlib import Path

from ..core.formats import xp3_parse, xp3_read_file
from ..core.plugin import Detection, OpResult
from ..core.util import ProgressFn, ProgressReporter, safe_out_path
from .base import EngineBase


class KirikiriPlugin(EngineBase):
    id = "kirikiri"
    name = "KiriKiri / KAG"
    priority = 75

    def detect(self, root: Path) -> Detection:
        ev, score = [], 0
        xp3s = list(root.glob("*.xp3")) + list(root.glob("**/*.xp3"))
        if xp3s:
            score += 70; ev.append(f"{len(xp3s)} 个 .xp3 封包")
        if any(p.name.lower().startswith("krkr") for p in root.glob("*.exe")):
            score += 20; ev.append("krkr*.exe")
        if list(root.glob("*.tjs")) or list(root.glob("**/*.ks")):
            score += 15; ev.append("明文 .tjs/.ks 脚本")
        return Detection(notes="XP3" if xp3s else "", score=score, evidence=ev)

    def describe(self, root: Path) -> str:
        xp3s = [p.name for p in root.glob("*.xp3")]
        return f"封包: {xp3s[:5]}"

    def do_extract(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        xp3s = [p for p in root.glob("*.xp3")] or [p for p in root.glob("**/*.xp3")][:5]
        if not xp3s:
            return OpResult(False, "未找到 .xp3")
        rep = ProgressReporter(progress)
        done, failed = 0, []
        for i, arc in enumerate(xp3s[:20]):
            data = arc.read_bytes()
            try:
                files = xp3_parse(data)
            except Exception as e:
                failed.append(f"{arc.name}: {e}")
                continue
            with rep.phase(i / len(xp3s), (i + 1) / len(xp3s), arc.name):
                sub = out_dir / arc.stem
                names = list(files.keys())
                for j, name in enumerate(names):
                    try:
                        blob = xp3_read_file(data, files[name])
                        safe_out_path(sub, name).write_bytes(blob)
                        done += 1
                    except Exception:
                        pass
                    rep(j / max(1, len(names)), name)
        msg = f"解包 {done} 个文件 → {out_dir}"
        if failed:
            msg += f"；失败封包（可能自定义加密）: {'; '.join(failed[:3])}"
        return OpResult(done > 0, msg, files_done=done)

    def do_text_extract(self, root: Path, out_csv: Path, options: dict, progress: ProgressFn) -> OpResult:
        """从解包出的 .ks（KAG 剧本）提取对话文本。"""
        base = Path(options.get("ks_dir") or out_csv.parent / "ks")
        ks_files = [p for p in root.rglob("*.ks")] or [p for p in base.rglob("*.ks")]
        if not ks_files:
            return OpResult(False, "未找到 .ks 脚本（请先解包 .xp3）")
        rep = ProgressReporter(progress)
        rows = []
        for i, p in enumerate(ks_files):
            try:
                text = p.read_text("shift-jis", errors="replace")
            except Exception:
                text = p.read_text("utf-8", errors="replace")
            for ln, line in enumerate(text.splitlines(), 1):
                s = line.strip()
                if not s or s.startswith((";", "@", "*")):
                    continue
                s = re.sub(r"\[[^\]]*\]", "", s).strip()
                if s:
                    rows.append({"id": f"{p.name}:{ln}", "file": p.name, "context": "kag",
                                 "source": s, "translation": ""})
            rep((i + 1) / len(ks_files), p.name)
        out_csv.parent.mkdir(parents=True, exist_ok=True)
        with open(out_csv, "w", newline="", encoding="utf-8-sig") as f:
            w = csv.DictWriter(f, fieldnames=["id", "file", "context", "source", "translation"])
            w.writeheader(); w.writerows(rows)
        return OpResult(True, f"提取 {len(rows)} 行对话 → {out_csv}", files_done=len(rows))
