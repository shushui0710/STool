"""RPG Maker MV / MZ 插件：素材解密/加密、数据 JSON 编辑、文本提取回填、存档解码。"""
from __future__ import annotations

import csv
import json
import re
import zlib
from pathlib import Path

from ..core.formats import MV_FAKE, MZ_FAKE, rpgmmv_decrypt, rpgmmv_encrypt
from ..core.plugin import Detection, OpResult
from ..core.util import ProgressFn, ProgressReporter, safe_out_path, backup_file
from .base import EngineBase

DEC_MAP = {".rpgmvp": ".png", ".rpgmvo": ".ogg", ".rpgmvm": ".m4a",
           ".png_": ".png", ".ogg_": ".ogg", ".m4a_": ".m4a"}
ENC_MAP = {v: k for k, v in DEC_MAP.items() if not k.endswith("_")}  # MV 后缀
ENC_MAP_MZ = {v: k for k, v in DEC_MAP.items() if k.endswith("_")}   # MZ 后缀


# ---------- 存档：MV lz-string / MZ zlib ----------

from lzstring import LZString as _LZ

_LZ_INST = _LZ()


def lzstring_decompress_from_base64(s: str) -> str:
    """lz-string compressToBase64 解压（使用 lzstring 包）。"""
    s = s.replace(" ", "+").replace("_", "/").replace("-", "+")
    return _LZ_INST.decompressFromBase64(s.strip()) or ""


def mv_save_decode(data: bytes) -> str:
    """RPG Maker MV 存档 → JSON 字符串。"""
    if data[:1] == b"\x00" or b"lzd" in data[:8] or data[:1] == b"l":
        # MV: "lzd"+base64? 实际为 LZString.compressToBase64 文本
        text = data.decode("ascii", "replace")
        for prefix in ("lzd", "LZ", ""):
            if text.startswith(prefix):
                text = text[len(prefix):]
                break
        return lzstring_decompress_from_base64(text.strip())
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return lzstring_decompress_from_base64(data.decode("ascii", "replace"))


def mz_save_decode(data: bytes) -> str:
    """RPG Maker MZ 存档 → JSON 字符串（pako/zlib）。"""
    if data[:1] == b"\x78":
        return zlib.decompress(data).decode("utf-8")
    return data.decode("utf-8", "replace")


class RpgMakerMVPlugin(EngineBase):
    id = "rpgmaker_mv"
    name = "RPG Maker MV / MZ"
    priority = 85

    def _data_dir(self, root: Path):
        for c in (root / "www" / "data", root / "data"):
            if (c / "Actors.json").exists():
                return c
        return None

    def detect(self, root: Path) -> Detection:
        ev, score = [], 0
        if self._data_dir(root):
            score += 60; ev.append("data/Actors.json 游戏数据")
        enc = [p for p in root.rglob("*") if p.suffix.lower() in DEC_MAP or p.name.endswith((".png_", ".ogg_"))]
        if enc:
            score += 25; ev.append(f"{len(enc)} 个加密素材")
        if list(root.glob("*.rpgproject")):
            score += 15; ev.append("Game.rpgproject")
        if (root / "www" / "js").is_dir() or (root / "js").is_dir():
            score += 10; ev.append("js/ 脚本目录")
        return Detection(notes="MV/MZ", score=score, evidence=ev)

    def describe(self, root: Path) -> str:
        d = self._data_dir(root)
        return f"数据目录: {d}" if d else ""

    def do_extract(self, root: Path, out_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        enc = [p for p in root.rglob("*") if p.is_file() and
               (p.suffix.lower() in DEC_MAP or p.name.endswith((".png_", ".ogg_", ".m4a_")))]
        if not enc:
            return OpResult(False, "未发现加密素材")
        rep = ProgressReporter(progress)
        done = 0
        for i, p in enumerate(enc):
            if p.suffix.lower() in DEC_MAP:
                new_ext = DEC_MAP[p.suffix.lower()]
            else:
                new_ext = p.suffix[:-1]
            dst = safe_out_path(out_dir, p.relative_to(root)).with_suffix(new_ext)
            dst.write_bytes(rpgmmv_decrypt(p.read_bytes()))
            done += 1
            rep((i + 1) / len(enc), p.name)
        return OpResult(True, f"解密 {done} 个素材 → {out_dir}", files_done=done)

    def do_repack(self, root: Path, src_dir: Path, options: dict, progress: ProgressFn) -> OpResult:
        mz = "mz" in options
        emap = ENC_MAP_MZ if mz else ENC_MAP
        done = 0
        for p in src_dir.rglob("*"):
            if p.is_file() and p.suffix in emap:
                rel = p.relative_to(src_dir)
                dst = root / rel.with_suffix(emap[p.suffix])
                dst.parent.mkdir(parents=True, exist_ok=True)
                if dst.exists():
                    backup_file(dst)
                dst.write_bytes(rpgmmv_encrypt(p.read_bytes(), mz=mz))
                done += 1
                progress(done / max(1, done), dst.name)
        return OpResult(True, f"加密回写 {done} 个素材", files_done=done)

    # ---- 文本提取/回填 ----
    def do_text_extract(self, root: Path, out_csv: Path, options: dict, progress: ProgressFn) -> OpResult:
        d = self._data_dir(root)
        if not d:
            return OpResult(False, "未找到 data/ 目录")
        rep = ProgressReporter(progress)
        maps = sorted(d.glob("Map*.json"))
        rows = []
        for i, f in enumerate(maps):
            try:
                data = json.loads(f.read_text("utf-8"))
            except Exception:
                continue
            ctx = data.get("displayName") or data.get("name") or f.stem
            for ev in data.get("events") or []:
                if not ev:
                    continue
                ename = ev.get("name") or ""
                pages = ev.get("pages") or ([ev] if ev.get("list") else [])
                for pi, page in enumerate(pages):
                    for li, cmd in enumerate(page.get("list") or []):
                        code = cmd.get("code")
                        if code == 401:  # 对话
                            txt = "".join(p.get("parameters", [""])[0] if p.get("parameters") else ""
                                          for p in [cmd])
                            # 401 每条命令一行，合并由回填侧处理；此处逐行提取
                            for para in cmd.get("parameters", []):
                                if isinstance(para, str) and para:
                                    rows.append({"id": f"{f.name}|{ev.get('id')}|{pi}|{li}",
                                                 "file": f.name, "context": f"{ctx}/{ename}",
                                                 "source": para, "translation": ""})
                        elif code == 102:  # 选项
                            for para in cmd.get("parameters", []):
                                if isinstance(para, list):
                                    for ci, choice in enumerate(para):
                                        if isinstance(choice, str) and choice:
                                            rows.append({"id": f"{f.name}|{ev.get('id')}|{pi}|{li}|c{ci}",
                                                         "file": f.name, "context": f"{ctx}/{ename}/选项",
                                                         "source": choice, "translation": ""})
                        elif code == 405:  # 滚动文字
                            for para in cmd.get("parameters", []):
                                if isinstance(para, str) and para:
                                    rows.append({"id": f"{f.name}|{ev.get('id')}|{pi}|{li}|s",
                                                 "file": f.name, "context": f"{ctx}/滚动", "source": para, "translation": ""})
            rep((i + 1) / max(1, len(maps)), f.name)
        # 系统词条（技能/物品名）
        for sysfile in ("Items.json", "Skills.json", "Armors.json", "Weapons.json", "Classes.json", "Actors.json"):
            fp = d / sysfile
            if not fp.exists():
                continue
            try:
                for item in json.loads(fp.read_text("utf-8")):
                    if item and item.get("name"):
                        rows.append({"id": f"{sysfile}|{item.get('id')}|name", "file": sysfile,
                                     "context": "词条", "source": item["name"], "translation": ""})
            except Exception:
                pass
        out_csv.parent.mkdir(parents=True, exist_ok=True)
        with open(out_csv, "w", newline="", encoding="utf-8-sig") as fo:
            w = csv.DictWriter(fo, fieldnames=["id", "file", "context", "source", "translation"])
            w.writeheader()
            w.writerows(rows)
        return OpResult(True, f"提取 {len(rows)} 条文本 → {out_csv}", files_done=len(rows))

    def do_text_import(self, root: Path, csv_path: Path, options: dict, progress: ProgressFn) -> OpResult:
        d = self._data_dir(root)
        if not d:
            return OpResult(False, "未找到 data/ 目录")
        rows = [r for r in csv.DictReader(open(csv_path, encoding="utf-8-sig")) if r.get("translation")]
        edits: dict[str, dict] = {}
        for r in rows:
            edits.setdefault(r["file"], {})[r["id"]] = r["translation"]
        out_dir = Path(options.get("out_dir") or (d.parent / "data_translated"))
        out_dir.mkdir(parents=True, exist_ok=True)
        done = 0
        # 词条文件
        for sysfile, m in edits.items():
            if sysfile.endswith(".json") and not sysfile.startswith("Map"):
                fp = d / sysfile
                if not fp.exists():
                    continue
                data = json.loads(fp.read_text("utf-8"))
                for key, val in m.items():
                    try:
                        _, iid, _ = key.split("|", 2)
                        for item in data:
                            if item and str(item.get("id")) == iid:
                                item["name"] = val
                                done += 1
                    except ValueError:
                        continue
                (out_dir / sysfile).write_text(json.dumps(data, ensure_ascii=False, indent=1), "utf-8")
                continue
            # 地图事件
            fp = d / sysfile
            if not fp.exists():
                continue
            data = json.loads(fp.read_text("utf-8"))
            # id 形如 Map001.json|3|0|12 或 ...|c0；回填 401 行 / 102 选项
            map_edits: dict[tuple, str] = {}
            for key, val in m.items():
                parts = key.split("|")
                if len(parts) >= 4:
                    try:
                        eid, pi, li = int(parts[1]), int(parts[2]), int(parts[3])
                    except ValueError:
                        continue
                    map_edits[(eid, pi, li, parts[3] if len(parts) > 4 else "")] = val
            # 直接替换 401 的 parameters[0]（一行一 cmd 的情形，与提取一致）
            for ev in data.get("events") or []:
                if not ev:
                    continue
                pages = ev.get("pages") or ([ev] if ev.get("list") else [])
                for pi, page in enumerate(pages):
                    lst = page.get("list") or []
                    for li, cmd in enumerate(lst):
                        key4 = (ev.get("id"), pi, li, "")
                        key5c = (ev.get("id"), pi, li, "")
                        if key4 in map_edits and cmd.get("code") == 401:
                            cmd["parameters"][0] = map_edits[key4]
                            done += 1
                        if cmd.get("code") == 102:
                            ckey = None
                            for k, v in map_edits.items():
                                if k[:3] == (ev.get("id"), pi, li) and k[3].startswith("c"):
                                    idx = int(k[3][1:])
                                    try:
                                        cmd["parameters"][0][idx] = v
                                        done += 1
                                    except Exception:
                                        pass
            (out_dir / sysfile).write_text(json.dumps(data, ensure_ascii=False, indent=1), "utf-8")
        return OpResult(True, f"回填 {done} 条 → {out_dir}（重命名目录为 data 前请备份原目录）", files_done=done)

    # ---- 存档 ----
    def _save_dirs(self, root: Path):
        dirs = []
        for c in (root / "www" / "save", root / "save"):
            if c.is_dir():
                dirs.append(c)
        return dirs

    def do_save(self, root: Path, options: dict, progress: ProgressFn) -> OpResult:
        action = options.get("action", "decode")
        sdirs = self._save_dirs(root)
        files = [p for d in sdirs for p in d.glob("*.rmmzsave")] + \
                [p for d in sdirs for p in d.glob("*.rmzsave")] + \
                [p for d in sdirs for p in d.glob("*.rpgsave")]
        if not files:
            return OpResult(False, "未找到 save/ 存档文件")
        out_dir = Path(options.get("out_dir") or sdirs[0] / "_stool_json")
        out_dir.mkdir(parents=True, exist_ok=True)
        done = 0
        for p in files:
            raw = p.read_bytes()
            try:
                js = mz_save_decode(raw) if p.suffix == ".rmzsave" else mv_save_decode(raw)
                json.loads(js)  # 校验
            except Exception as e:
                continue
            dst = out_dir / (p.stem + ".json")
            if action == "encode":
                src = out_dir / (p.stem + ".json")
                if src.exists():
                    text = src.read_text("utf-8")
                    enc = zlib.compress(text.encode("utf-8")) if p.suffix == ".rmzsave" else \
                        text.encode("utf-8")  # MV 回写：占位（多数引擎兼容 MZ 格式有限）
                    backup_file(p)
                    p.write_bytes(enc)
                    done += 1
            else:
                dst.write_text(json.dumps(json.loads(js), ensure_ascii=False, indent=1), "utf-8")
                done += 1
        if action == "encode":
            return OpResult(True, f"回写 {done} 个存档（原文件已备份 .stool.bak）", files_done=done)
        return OpResult(True, f"解码 {done} 个存档 → {out_dir}", files_done=done)
