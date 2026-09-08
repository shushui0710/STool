"""Ren'Py 引擎插件：RPA 解包/封包、rpyc 反编译、文本提取回填、persistent 存档。"""
from __future__ import annotations

import csv
import io
import os
import pickle
import re
import shutil
import struct
import subprocess
import sys
import zlib
from pathlib import Path

from ..core.plugin import Detection, OpResult
from ..core.util import ProgressFn, ProgressReporter, safe_out_path, backup_file
from .base import EngineBase


# ---------------- RPA 格式 ----------------

def rpa_read_index(archive: Path) -> tuple:
    """解析 RPA-3.0/3.2/4.0。返回 (version_str, {name: [(prefix_bytes, offset, dlen)]})。"""
    with open(archive, "rb") as f:
        header = f.readline()
        parts = header.split()
        if len(parts) < 2:
            raise ValueError("不是 RPA 封包")
        version = parts[0].decode("ascii", "replace")
        offset = int(parts[1], 16)
        key = int(parts[2], 16) if len(parts) > 2 else 0
        f.seek(offset)
        index = pickle.loads(zlib.decompress(f.read()))
    entries: dict[str, list] = {}
    for name, chunks in index.items():
        if isinstance(name, bytes):
            name = name.decode("utf-8", "replace")
        out = []
        for chunk in chunks:
            if len(chunk) == 2:
                off, dlen = chunk
                out.append((b"", off ^ key, dlen ^ key))
            else:
                off, dlen, start = chunk
                if isinstance(start, int):      # 旧式：前缀字节数，需 XOR
                    with open(archive, "rb") as f2:
                        f2.seek(off ^ key)
                        raw = f2.read(dlen ^ key)
                    prefix = bytes(b ^ (key & 0xFF) for b in raw[:start])
                    out.append((prefix, (off ^ key) + start, (dlen ^ key) - start))
                else:                            # 现代式：前缀字节明文
                    if isinstance(start, str):
                        start = start.encode("latin-1")
                    out.append((start, off ^ key, dlen ^ key))
        entries[name] = out
    return version, entries


def rpa_read_file(archive: Path, chunks: list) -> bytes:
    data = b""
    for prefix, off, dlen in chunks:
        data += prefix
        with open(archive, "rb") as f:
            f.seek(off)
            data += f.read(dlen)
    return data


def rpa_write_index(archive: Path, files: dict[str, bytes], key: int = 0xDEADBEEF) -> None:
    """从内存文件字典写 RPA-3.0 封包。"""
    index = {}
    body = io.BytesIO()
    for name, data in sorted(files.items()):
        off = body.tell()
        body.write(data)
        index[name.encode("utf-8")] = [(off ^ key, len(data) ^ key)]
    with open(archive, "wb") as f:
        offset = body.tell() + 34
        f.write(f"RPA-3.0 {offset:016x} {key:08x}\n".encode("ascii"))
        f.write(body.getvalue())
        f.write(zlib.compress(pickle.dumps(index, protocol=2)))


# ---------------- persistent 存档（尽力解析） ----------------

class _StubUnpickler(pickle.Unpickler):
    """为 Ren'Py 存档中未知类生成哑对象，避免反序列化失败。"""

    def find_class(self, module: str, name: str):
        try:
            return super().find_class(module, name)
        except Exception:
            class Stub:  # noqa
                def __init__(self, *a, **k):
                    pass
                def __setstate__(self, state):
                    if isinstance(state, dict):
                        self.__dict__.update(state)
            Stub.__module__, Stub.__qualname__ = module, name
            return Stub


def persistent_load(path: Path):
    with open(path, "rb") as f:
        head = f.read(8)
        f.seek(0)
    if head[:4] == b"\x80\x02" or head[:2] == b"\x80\x02":
        return _StubUnpickler(io.BytesIO(path.read_bytes())).load()
    # 带换行头 / 其他变体：找 pickle 起点
    raw = path.read_bytes()
    pos = raw.find(b"\x80\x02")
    if pos < 0:
        pos = raw.find(b"\x80\x03") + 1 if raw.find(b"\x80\x03") >= 0 else 0
    return _StubUnpickler(io.BytesIO(raw[pos:])).load()


def persistent_save(path: Path, obj, backup: bool = True) -> None:
    if backup:
        backup_file(path)
    with open(path, "wb") as f:
        pickle.dump(obj, f, protocol=2)


# ---------------- 文本提取/回填 ----------------

STR_RE = re.compile(r'"((?:[^"\\]|\\.)*)"')


class RenpyPlugin(EngineBase):
    id = "renpy"
    name = "Ren'Py"
    priority = 90

    def detect(self, root: Path) -> Detection:
        ev, score = [], 0
        if (root / "renpy").is_dir():
            score += 40; ev.append("renpy/ 引擎目录")
        rpas = list(root.glob("game/*.rpa"))
        if rpas:
            score += 30; ev.append(f"{len(rpas)} 个 .rpa 封包")
        if list(root.glob("game/*.rpyc")):
            score += 25; ev.append("game/*.rpyc 脚本")
        if list(root.glob("lib/py*-windows-*")):
            score += 15; ev.append("lib/py*-windows-* 运行时")
        return Detection(notes=", ".join(e for e in ev if "封包" in e) or "明文资源", score=score, evidence=ev)

    def describe(self, root: Path) -> str:
        rpas = [p for p in root.glob("game/*.rpa")]
        saves = self._save_locations(root)
        return (f"封包: {len(rpas)} 个 .rpa；存档目录: " +
                " ; ".join(str(s) for s in saves[:2]))

    # ---- 能力：解包 ----
    def do_extract(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        rpas = sorted(root.glob("game/*.rpa"))
        if not rpas:
            return OpResult(False, "未找到 .rpa 封包")
        rep = ProgressReporter(progress)
        total = len(rpas)
        done = 0
        for i, rpa in enumerate(rpas):
            try:
                _, entries = rpa_read_index(rpa)
            except Exception as e:
                # 回退到 unrpa
                return self._extract_via_unrpa(rpa, out_dir, progress)
            with rep.phase(i / total, (i + 1) / total, f"{rpa.name}"):
                for j, (name, chunks) in enumerate(sorted(entries.items())):
                    data = rpa_read_file(rpa, chunks)
                    safe_out_path(out_dir, name).write_bytes(data)
                    done += 1
                    rep(j / max(1, len(entries)), name)
        return OpResult(True, f"解包 {done} 个文件 → {out_dir}", files_done=done)

    def _extract_via_unrpa(self, rpa: Path, out_dir: Path, progress: ProgressFn) -> OpResult:
        cmd = [sys.executable, "-m", "unrpa", "-m", "-p", str(out_dir), str(rpa)]
        r = subprocess.run(cmd, capture_output=True, text=True)
        if r.returncode == 0:
            return OpResult(True, f"unrpa 解包完成 → {out_dir}")
        return OpResult(False, f"解包失败: {r.stderr[-400:]}")

    # ---- 能力：封包 ----
    def do_repack(self, root: Path, src_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        out = Path(options.get("out_archive") or (root / "game" / "stool_repack.rpa"))
        files = {str(p.relative_to(src_dir)).replace("\\", "/"): p.read_bytes()
                 for p in src_dir.rglob("*") if p.is_file()}
        progress(0.2, "写入封包")
        rpa_write_index(out, files)
        return OpResult(True, f"已生成 {out}（{len(files)} 个文件）", files_done=len(files))

    # ---- 能力：反编译 ----
    def do_decompile(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        from ..core.paths import bundled_unrpyc
        unrpyc = Path(options.get("unrpyc") or bundled_unrpyc())
        if not unrpyc.exists():
            return OpResult(False, f"未找到 unrpyc: {unrpyc}")
        rpyc_dir = root / "game"
        targets = [p for p in rpyc_dir.rglob("*.rpyc")] + [p for p in rpyc_dir.rglob("*.rpym")]
        if not targets:
            return OpResult(False, "未找到 .rpyc/.rpym")
        tmp = out_dir / "_rpyc_copy"
        tmp.mkdir(parents=True, exist_ok=True)
        progress(0.1, "复制 rpyc 到输出目录（只读模式）")
        for p in targets:
            rel = p.relative_to(rpyc_dir)
            dst = tmp / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(p, dst)
        in_place = options.get("in_place", False)
        args = [sys.executable, str(unrpyc), "-c"]
        if not in_place:
            args.append(str(tmp))
        else:
            args.append(str(rpyc_dir))
        progress(0.3, "运行 unrpyc 反编译")
        r = subprocess.run(args, capture_output=True, text=True)
        if r.returncode != 0:
            return OpResult(False, f"unrpyc 失败: {r.stderr[-500:]}")
        # 汇总 .rpy
        rpy_dir = rpyc_dir if in_place else tmp
        rpies = list(rpy_dir.rglob("*.rpy"))
        return OpResult(True, f"反编译 {len(rpies)} 个脚本（{rpy_dir}）", files_done=len(rpies))

    # ---- 能力：文本提取 ----
    def do_text_extract(self, root: Path, out_csv: Path, options: dict, progress: ProgressFn) -> OpResult:
        base = Path(options.get("rpy_dir") or (root / "game"))
        files = [p for p in base.rglob("*.rpy")]
        if not files:
            return OpResult(False, f"{base} 下无 .rpy，请先反编译")
        rep = ProgressReporter(progress)
        rows, rid = [], 0
        say_re = re.compile(r'^(\s*)(?:[\w\.\[\]"\' ]+\s+)?"(.*)"\s*(?:$|#)')
        for i, p in enumerate(files):
            for ln, line in enumerate(p.read_text("utf-8", errors="replace").splitlines(), 1):
                s = line.strip()
                if not s or s.startswith(("#", "define ", "$ ", "jump ", "call ", "scene ", "show ", "hide ", "play ", "stop ", "pause", "return", "label ", "init ", "style ", "image ")):
                    continue
                m = say_re.match(line)
                if m and m.group(2):
                    rid += 1
                    rows.append({"id": f"{p.name}:{ln}", "file": str(p.relative_to(base)),
                                 "context": "dialogue", "source": m.group(2), "translation": ""})
                    continue
                if s.startswith(("menu", "textbutton", "caption")) or re.match(r'^".*"$', s):
                    for mm in STR_RE.finditer(s):
                        txt = mm.group(1)
                        if txt and not txt.startswith(("images/", "audio/", "gui/", "fonts/", "{")):
                            rid += 1
                            rows.append({"id": f"{p.name}:{ln}", "file": str(p.relative_to(base)),
                                         "context": "ui/choice", "source": txt, "translation": ""})
            rep((i + 1) / len(files), p.name)
        out_csv.parent.mkdir(parents=True, exist_ok=True)
        with open(out_csv, "w", newline="", encoding="utf-8-sig") as f:
            w = csv.DictWriter(f, fieldnames=["id", "file", "context", "source", "translation"])
            w.writeheader()
            w.writerows(rows)
        return OpResult(True, f"提取 {len(rows)} 条文本 → {out_csv}", files_done=len(rows))

    def do_text_import(self, root: Path, csv_path: Path, options: dict, progress: ProgressFn) -> OpResult:
        """按 id(file:line) 回填翻译到 .rpy（生成 .rpy.translated 副本，不覆盖原文件）。"""
        rows = list(csv.DictReader(open(csv_path, encoding="utf-8-sig")))
        by_file: dict[str, dict[int, str]] = {}
        for r in rows:
            if not r.get("translation"):
                continue
            fname, _, ln = r["id"].rpartition(":")
            try:
                by_file.setdefault(fname, {})[int(ln)] = r["translation"]
            except ValueError:
                continue
        base = Path(options.get("rpy_dir") or (root / "game"))
        done = 0
        out_root = Path(options.get("out_dir") or (base / "_translated"))
        out_root.mkdir(parents=True, exist_ok=True)
        for fname, edits in by_file.items():
            src = base / fname
            if not src.exists():
                continue
            lines = src.read_text("utf-8", errors="replace").splitlines(keepends=True)
            for ln, text in edits.items():
                if 1 <= ln <= len(lines):
                    m = say_re.match(lines[ln - 1])
                    if m:
                        indent = m.group(1) if m.group(1) else re.match(r"\s*", lines[ln - 1]).group(0)
                        lines[ln - 1] = f'{indent}"{text}"\n'
                        done += 1
            dst = out_root / fname
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_text("".join(lines), "utf-8")
        return OpResult(True, f"回填 {done} 条 → {out_root}（将目录复制回 game/ 即生效）", files_done=done)

    # ---- 能力：存档 ----
    def _save_locations(self, root: Path) -> list:
        locs = []
        appdata = Path(os.environ.get("APPDATA", ""))
        if appdata.exists():
            rp = appdata / "RenPy"
            if rp.is_dir():
                locs += [d for d in rp.iterdir() if d.is_dir()]
        return locs

    def do_save(self, root: Path, options: dict, progress: ProgressFn) -> OpResult:
        locs = self._save_locations(root)
        if not locs:
            return OpResult(False, "未找到 %AppData%/RenPy/ 存档目录")
        detail = []
        for d in locs:
            files = list(d.glob("**/*"))
            detail.append(f"{d.name}: {len(files)} 个文件")
        return OpResult(True, "；".join(detail), detail=detail)

    def do_unlock(self, root: Path, options: dict, progress: ProgressFn) -> OpResult:
        """persistent 全解锁辅助：列出所有键值供修改（布尔键可一键置 True）。"""
        locs = self._save_locations(root)
        target = Path(options.get("persistent") or "") if options.get("persistent") else None
        if not target:
            for d in locs:
                p = d / "persistent"
                if p.exists():
                    target = p
                    break
        if not target or not target.exists():
            return OpResult(False, "未找到 persistent 文件")
        try:
            obj = persistent_load(target)
        except Exception as e:
            return OpResult(False, f"persistent 解析失败: {e}")
        if isinstance(obj, dict):
            set_true = [k for k, v in obj.items() if isinstance(v, bool) and not v and any(
                t in str(k).lower() for t in ("unlock", "seen", "clear", "cg", "gallery", "replay", "end"))]
            if options.get("set_true_all"):
                for k in set_true:
                    obj[k] = True
                persistent_save(target, obj)
                return OpResult(True, f"已将 {len(set_true)} 个解锁布尔键置 True: {set_true}")
            return OpResult(True, f"共 {len(obj)} 键，可解锁布尔键 {len(set_true)} 个: {set_true[:30]}")
        return OpResult(True, "persistent 已解析（非字典结构，需手动处理）")
