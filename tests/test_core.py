"""STool 核心回环测试：合成各引擎封包样本 → 检测 → 解包 → 比对内容一致。

运行： python tests/test_core.py
"""
from __future__ import annotations

import io
import json
import pickle
import shutil
import struct
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from stool.core.plugin import PluginRegistry
from stool.core.job import make_job
from stool.engines.html_game import asar_pack

TMP = Path(__file__).resolve().parent / "_fixtures"


def fresh(name: str) -> Path:
    d = TMP / name
    shutil.rmtree(d, ignore_errors=True)
    d.mkdir(parents=True)
    return d


# ---------- 样本生成器 ----------

def make_rpa(game: Path, files: dict, key: int = 0xDEADBEEF):
    """RPA-3.0（现代式：前缀字节明文存储于索引）。"""
    index = {}
    body = io.BytesIO()
    for name, data in files.items():
        off = body.tell()
        body.write(data)
        index[name.encode()] = [(off ^ key, len(data) ^ key)]
    arc = game / "game" / "archive.rpa"
    arc.parent.mkdir(exist_ok=True)
    with open(arc, "wb") as f:
        f.write(f"RPA-3.0 {body.tell()+34:016x} {key:08x}\n".encode())
        f.write(body.getvalue())
        f.write(zlib.compress(pickle.dumps(index, protocol=2)))
    return arc


def make_mv_game(game: Path):
    www = game / "www"
    (www / "data").mkdir(parents=True)
    (www / "data" / "Actors.json").write_text(
        json.dumps([None, {"id": 1, "name": "勇者"}], ensure_ascii=False), encoding="utf-8")
    (www / "data" / "Map001.json").write_text(json.dumps({
        "events": [{"id": 1, "name": "村人", "pages": [{"list": [
            {"code": 401, "parameters": ["今天天气不错。"]},
            {"code": 102, "parameters": [["是", "否"]]},
        ]}]}]}, ensure_ascii=False), encoding="utf-8")
    (game / "Game.rpgproject").write_bytes(b"{}")
    png = b"\x89PNG\r\n\x1a\n" + b"FAKEPNGDATA" * 4
    fake = bytes([0x52, 0x50, 0x47, 0x4D, 0x56] + [0] * 11)
    head = bytes(a ^ b for a, b in zip(png[:16], fake))
    (www / "img").mkdir()
    (www / "img" / "hero.rpgmvp").write_bytes(fake + head + png[16:])
    return png


def make_rgss1(game: Path, files: dict):
    def keygen(base=0xDEADCAFE):
        k = base
        while True:
            yield k
            k = (k * 7 + 3) & 0xFFFFFFFF
    kg = keygen()
    out = io.BytesIO()
    out.write(b"RGSSAD\x00\x01")
    for name, data in files.items():
        nb = name.encode()
        out.write(struct.pack("<I", len(nb) ^ next(kg)))
        enc_name = bytearray()
        for b in nb:
            enc_name.append(b ^ (next(kg) & 0xFF))
        out.write(bytes(enc_name))
        out.write(struct.pack("<I", len(data) ^ next(kg)))
        kg2 = keygen()
        for i in range(len(data)):
            if i % 4 == 0:
                k = next(kg2)
            out.write(bytes([data[i] ^ (k & 0xFF)]))
    arc = game / "Game.rgssad"
    arc.write_bytes(out.getvalue())
    return arc


def make_xp3(game: Path, files: dict):
    def chunk(tag: bytes, body: bytes) -> bytes:
        return tag + struct.pack("<I", len(body)) + body

    toc_entries = []
    data_blob = io.BytesIO()
    for name, content in files.items():
        comp = zlib.compress(content)
        aoff = data_blob.tell()
        data_blob.write(comp)
        nb = name.encode("utf-16-le")
        info = struct.pack("<IQQH", 0, len(content), len(comp), len(name)) + nb
        segm = struct.pack("<IQQQQ", 1, 0, aoff, len(content), len(comp))
        adlr = struct.pack("<I", zlib.adler32(content) & 0xFFFFFFFF)
        entry = chunk(b"info", info) + chunk(b"segm", segm) + chunk(b"adlr", adlr)
        toc_entries.append(struct.pack("<I", len(entry)) + entry)
    toc = struct.pack("<I", len(toc_entries)) + b"".join(toc_entries)
    comp_toc = zlib.compress(toc)
    body = b"XP3\r\n \n\x1a\x8b\x67\x01" + b"\x80" + struct.pack("<Q", 0)  # 占位，后面补 offset
    arc = game / "data.xp3"
    payload = b"\x01" + comp_toc + data_blob.getvalue()
    index_pos = len(body) + 7  # 修正：11字节魔数 + 1 flag + 8 offset
    header = b"XP3\r\n \n\x1a\x8b\x67\x01" + b"\x80" + struct.pack("<Q", 11 + 1 + 8)
    arc.write_bytes(header + payload)
    return arc


def make_pck(game: Path, files: dict):
    header = io.BytesIO()
    header.write(b"GDPC")
    header.write(struct.pack("<IIII", 1, 4, 0, 0))
    header.write(b"\x00" * 64)
    header.write(struct.pack("<I", len(files)))
    # 预估条目区大小
    est = 0
    for path in files:
        n = len(path) + 1 + 4 + ((4 - (len(path) + 1) % 4) % 4) + 16 + 16 + 16
        est += n
    base = header.tell() + est
    entries = io.BytesIO()
    data = io.BytesIO()
    for path, content in files.items():
        pb = path.encode() + b"\x00"
        entries.write(struct.pack("<I", len(pb)))
        entries.write(pb)
        entries.write(b"\x00" * ((4 - len(pb) % 4) % 4))
        entries.write(struct.pack("<QQ", base + data.tell(), len(content)))
        entries.write(b"\x00" * 16)  # md5 占位
        data.write(content)
    arc = game / "data.pck"
    arc.write_bytes(header.getvalue() + entries.getvalue() + data.getvalue())
    return arc


def make_nscript(game: Path, text: str):
    data = text.encode("shift-jis")
    key = 0x1234
    enc = bytearray([key >> 8, key & 0xFF])
    for i, b in enumerate(data):
        enc.append(b ^ ((key + i) & 0xFF))
    # ONScripter 变体：文件本身前两字节即密钥，解密器从 offset 0 处理全部字节
    arc = game / "nscript.dat"
    arc.write_bytes(bytes(enc))
    return arc


# ---------- 断言辅助 ----------

def extract_files(game: Path, out: Path, plugin_id: str | None = None):
    reg = PluginRegistry(extra_dirs=[])
    reg.discover()
    det = reg.detect_all(game)
    best = det[0]
    if plugin_id:
        best = next(d for d in det if d.plugin_id == plugin_id)
    job = make_job(reg, best.plugin_id, "extract", game, out, {})
    res = job.run()
    return best, res


def flatten(root: Path) -> dict:
    return {str(p.relative_to(root)).replace("\\", "/"): p.read_bytes()
            for p in root.rglob("*") if p.is_file()}


def test_all():
    passed, failed = 0, []

    def check(name, cond, extra=""):
        nonlocal passed
        if cond:
            passed += 1
            print(f"  ✔ {name}")
        else:
            failed.append(name + (" :: " + extra if extra else ""))
            print(f"  ✘ {name} {extra}")

    # 1. Ren'Py RPA
    print("[Ren'Py RPA]")
    g = fresh("renpy_game")
    (g / "renpy").mkdir()
    sample = {"images/bg.png": b"\x89PNG" + b"\x00" * 20,
              "scripts/story.rpyc": b"FAKE_RPYC" * 10}
    make_rpa(g, sample)
    (g / "game" / "script.rpyc").write_bytes(b"x")
    out = fresh("renpy_out")
    det, res = extract_files(g, out)
    check("检测为 Ren'Py", det.plugin_id == "renpy", det.plugin_id)
    check("RPA 解包内容一致", res.success and
          all(flatten(out).get(k) == v for k, v in sample.items()),
          str(flatten(out).keys()))

    # 2. RPG Maker MV
    print("[RPG Maker MV]")
    g = fresh("mv_game")
    png = make_mv_game(g)
    out = fresh("mv_out")
    det, res = extract_files(g, out)
    check("检测为 MV/MZ", det.plugin_id == "rpgmaker_mv", det.plugin_id)
    dec = flatten(out)
    got = dec.get("www/img/hero.png") or list(dec.values())[0] if dec else b""
    check("素材解密还原 PNG", got == png, f"{len(got)}B")

    # 3. RGSS1
    print("[RPG Maker XP/VX RGSSAD]")
    g = fresh("rgss_game")
    sample = {"Graphics/Battlesets/hero.png": b"\x89PNGDATA" * 3,
              "Data/Actors.rxdata": b"marshal\x00\x01"}
    make_rgss1(g, sample)
    out = fresh("rgss_out")
    det, res = extract_files(g, out)
    check("检测为 RGSS", det.plugin_id == "rpgmaker_rgss", det.plugin_id)
    got = flatten(out)
    check("RGSSAD 解包一致", all(got.get(k) == v for k, v in sample.items()),
          str(got.keys()))

    # 4. XP3
    print("[KiriKiri XP3]")
    g = fresh("xp3_game")
    sample = {"scene1.ks": "「こんにちは、世界。」\n@test\n第二行\r\n".encode("shift-jis"),
              "bg001.jpg": b"\xff\xd8\xff\xe0JFIF" + b"\x00" * 30}
    make_xp3(g, sample)
    out = fresh("xp3_out")
    det, res = extract_files(g, out)
    check("检测为 KiriKiri", det.plugin_id == "kirikiri", det.plugin_id)
    got = flatten(out)
    check("XP3 解包一致", all(got.get(k) == v for k, v in sample.items()),
          str(got.keys()))

    # 5. PCK
    print("[Godot PCK]")
    g = fresh("godot_game")
    sample = {"res://scripts/main.gd": b"extends Node\nfunc _ready(): pass\n",
              "res://assets/logo.png": b"\x89PNG" + b"A" * 12}
    make_pck(g, sample)
    out = fresh("godot_out")
    det, res = extract_files(g, out)
    check("检测为 Godot", det.plugin_id == "godot", det.plugin_id)
    got = flatten(out)
    check("PCK 解包一致", all(got.get(k.lstrip("res:/")) == v for k, v in sample.items()),
          str(got.keys()))

    # 6. asar
    print("[Electron asar]")
    g = fresh("asar_game")
    (g / "resources").mkdir()
    appdir = g / "_app_src"
    appdir.mkdir()
    sample = {"index.html": b"<html><body>hi</body></html>",
              "js/main.js": b"console.log('hello')"}
    for k, v in sample.items():
        dst = appdir / k
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_bytes(v)
    asar_pack(appdir, g / "resources" / "app.asar")
    out = fresh("asar_out")
    det, res = extract_files(g, out)
    check("检测为 HTML/Electron", det.plugin_id == "html_game", det.plugin_id)
    got = flatten(out)
    check("asar 解包一致", all(got.get(k) == v for k, v in sample.items()),
          str(got.keys()))

    # 7. NScripter
    print("[NScripter]")
    g = fresh("ns_game")
    text = "*define\ncaption \"テスト\"\n*start\n「こんにちは。」\n"
    make_nscript(g, text)
    out = fresh("ns_out")
    det, res = extract_files(g, out)
    check("检测为 NScripter", det.plugin_id == "nscripter", det.plugin_id)
    got = (out / "nscript.txt")
    check("nscript.dat 解密还原", got.exists() and "「こんにちは。」" in got.read_text("shift-jis", errors="replace"),
          got.read_text("shift-jis", errors="replace")[:50] if got.exists() else "missing")

    # 8. MV 文本提取
    print("[MV 文本提取]")
    g = fresh("mv_text_game")
    make_mv_game(g)
    out = fresh("mv_text_out")
    reg = PluginRegistry(extra_dirs=[]); reg.discover()
    job = make_job(reg, "rpgmaker_mv", "text_extract", g, out / "text.csv", {})
    res = job.run()
    import csv as _csv
    rows = list(_csv.DictReader(open(out / "text.csv", encoding="utf-8-sig"))) if res.success else []
    srcs = {r["source"] for r in rows}
    check("提取对话与选项", res.success and "今天天气不错。" in srcs and "是" in srcs,
          str(srcs))

    print()
    print(f"通过 {passed} 项，失败 {len(failed)} 项")
    for f in failed:
        print("  ✘", f)
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(test_all())
