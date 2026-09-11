#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""scan_games.py — 游戏目录引擎盘点，为 STool「全 CG 解锁」路线图提供输入。

背景
----
网盘下载的游戏常常做了「防和谐」处理：随机前缀、改扩展名、改 _Data 目录名、
加版本/汉化后缀、外面套一层壳目录、只留压缩包不落地……靠肉眼看目录名基本判不出发动机。

本脚本对指定根目录下的每个一级子目录：
  1. 单次遍历建立目录索引（复刻 STool `ScanCtx` 的语义）；
  2. 在「一级目录 + 其子孙目录」中挑出真正的游戏根目录（自动下钻壳目录）；
  3. 用与 STool `engines/recognize.rs` + 各正式插件一致的规则表打分识别引擎；
  4. 文件名识别不出来时，抽样读文件头（magic）兜底，处理「改扩展名」的伪装；
  5. 识别并记录伪装类型（随机前缀 / 改扩展名 / 改 _Data / 壳目录 / 版本后缀 …）；
  6. 按「引擎 → 解锁方式 → 是否可自动化」产出面向 CG 解锁的记录结构。

输出（默认写到 `D:\\STool\\verify\\game_inventory\\`，该目录被 .gitignore 排除）：
    games_inventory.json   机器可读，字段最全（后续可直接喂给 STool）
    games_inventory.md     人工阅读的盘点报告 + 解锁路线图
    games_inventory.csv    Excel 可直接打开的表格

用法
----
    python scan_games.py
    python scan_games.py --roots "D:\\baidudownload;D:\\ero" --out D:\\STool\\verify\\game_inventory
    python scan_games.py --limit 5 --quiet          # 只扫前 5 个目录，快速验证
"""

from __future__ import annotations

import argparse
import concurrent.futures as futures
import csv
import json
import os
import re
import sys
import unicodedata
from collections import Counter
from datetime import datetime

# ---------------------------------------------------------------------------
# 控制台编码（Windows 默认 GBK，打印日文/中文混合会炸）
# ---------------------------------------------------------------------------
for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8", errors="replace")  # type: ignore[attr-defined]
    except Exception:
        pass

DETECT_LINE = 60  # 与 STool engines::DETECT_LINE 保持一致

# ---------------------------------------------------------------------------
# 与 STool scan.rs 的 NOTABLE_FILES 对齐（任意深度命中的「特征文件名」，小写）
# 末尾几条是本脚本为覆盖更多引擎额外加的（STool 未内置，仅供识别参考）
# ---------------------------------------------------------------------------
NOTABLE_FILES = {
    # --- STool scan.rs 原有 ---
    "actors.json",
    "game.rpgproject",
    "assembly-csharp.dll",
    "gameassembly.dll",
    "project.godot",
    "config.tjs",
    "startup.tjs",
    "data.win",
    "gameexe.dat",
    "gameexe.ini",
    "seen.txt",
    "tyrano.js",
    "main.lua",
    # --- 本脚本补充（用于更细分的识别 / 变体判断）---
    "app.info",            # Unity: 公司名/产品名（PlayerPrefs 注册表路径）
    "globalgamemanagers",  # Unity: 资源清单
    "resources.assets",    # Unity
    "nscript.dat",         # NScripter 加密脚本
    "data.xp3",            # KiriKiri 主封包（常见名，帮助给出更具体的证据）
    "rpg_rt.ldb",          # RPG Maker 2000/2003 数据库
    "bregexp.dll",         # BGI/Ethornell（Buriko）特征 DLL
    "bgi.gdb",             # BGI/Ethornell 配置/存档（SDC FORMAT 1.00）
    "bgi.db",
    "エンジン設定.exe",     # BGI 系（ωstar 等重打包）的引擎设置工具
    "enginesetting.exe",   # 同上，英文名版本
}

# 判定扩展名 -> 说明（用于候选根目录挑选时快速打分）
STRONG_EXTS = {
    "rpa", "rpyc", "rpym", "rpy",
    "rgssad", "rgss2a", "rgss3a", "rxdata", "rvdata", "rvdata2",
    "rpgmvp", "rpgmvo", "rpgmvm", "png_", "ogg_", "m4a_",
    "pck", "xp3", "wolf", "nsa", "sar", "asar", "ks", "tjs",
    "mjo", "ypf", "pfs", "pfs2", "asb", "ald", "afa", "alk", "npk", "npa",
    "prt", "grp", "grs", "int", "cst", "utoc", "ucas", "uasset", "love",
    "lpk", "qsp", "qsps", "swf",
}

MEDIA_EXTS = {
    "jpg", "jpeg", "png", "bmp", "gif", "webp", "avif", "tif", "tiff",
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts",
    "mp3", "wav", "flac", "m4a", "aac", "ogg",
}
ARCHIVE_EXTS = {"zip", "7z", "rar", "001", "002", "003", "iso", "tar", "gz", "xz", "bz2", "z01"}
IMAGE_ONLY_SET_HINTS = ("artbook", "artbooks", "cg", "画集", "原画", "设定集", "写真", "photobook")

# 目录内自带「全 CG 存档 / 解锁补丁」是解锁最省事的捷径，命中即高置信标注
CG_SAVE_HINTS = ("全cg", "cg存档", "全cg存档", "全cg补丁", "cg全开", "全开存档",
                 "全cg解锁", "100%存档", "全回想", "全開", "cg回廊全开")
# 存档目录名（用于给出"存档在哪"的线索）
SAVE_DIR_HINTS = ("savedata", "savedat", "save", "saves", "存档", "userdata", "systemdata")
# 纯广告/推广目录里常见的扩展名（无游戏内容）
ALLOWED_JUNK_EXTS = {"url", "txt", "ds_store", "lnk", "html", "htm", "ini", "nfo"}

# 明显是非游戏的工具（目录名关键词，小写匹配）
TOOL_KEYWORDS = (
    "cheat engine", "mtool", "openspeedy", "ce 7", "翻译工具", "汉化工具",
    "translator", "autotranslator", "xunity", "toolbox", "工具",
)

# ---------------------------------------------------------------------------
# 文件头 magic 兜底（处理「改扩展名」的伪装）
# 命中即给出 (engine_id, 说明)
# ---------------------------------------------------------------------------
MAGIC_SIGS = [
    (b"UnityFS", 0, "unity", "UnityFS AssetBundle 头"),
    (b"UnityWeb", 0, "unity", "UnityWebData 头"),
    (b"RGSSAD", 0, "rpgmaker_rgss", "RGSSAD 魔数"),
    (b"RGSS2A", 0, "rpgmaker_rgss", "RGSS2A 魔数"),
    (b"RGSS3A", 0, "rpgmaker_rgss", "RGSS3A 魔数"),
    (b"XP3", 0, "kirikiri", "XP3 魔数"),
    (b"GDPC", 0, "godot", "Godot PCK 魔数"),
    (b"RPA-", 0, "renpy", "Ren'Py RPA 魔数"),
    (b"WOLF", 0, "wolf", "WOLF 魔数"),
    (b"\xe1\x12\x6f\x5a", 0, "unreal", "Unreal .pak 魔数"),
]
MAGIC_CONTAINS = [
    (b'"files"', 8192, "html_game", "asar/JSON 头（Electron app.asar）"),
]


# ===========================================================================
# 目录索引
# ===========================================================================
class Node:
    """一个目录节点 + 其子树聚合统计（复刻 STool ScanCtx 语义）。"""

    __slots__ = ("path", "name", "files", "dirs", "ext", "notable", "dirnames", "hints", "truncated")

    def __init__(self, path: str, name: str):
        self.path = path
        self.name = name
        self.files: list[tuple[str, str, str]] = []   # (原始名, 小写名, 小写扩展名)
        self.dirs: list["Node"] = []
        self.ext: Counter[str] = Counter()       # 子树内扩展名计数
        self.notable: set[str] = set()           # 子树内命中的特征文件名
        self.dirnames: set[str] = set()          # 子树内目录名（小写，限深度）
        self.hints: set[str] = set()             # 子树内的解锁线索（全CG存档/存档目录）
        self.truncated = False


MAX_FILES = 800_000
DIRNAME_DEPTH = 6


def build_tree(path: str, name: str, budget: list[int]) -> Node:
    node = Node(path, name)
    if budget[0] <= 0:
        node.truncated = True
        return node
    try:
        it = os.scandir(path)
    except OSError:
        return node
    with it:
        for e in it:
            if budget[0] <= 0:
                node.truncated = True
                break
            try:
                if e.is_dir(follow_symlinks=False):
                    node.dirs.append(build_tree(e.path, e.name, budget))
                elif e.is_file(follow_symlinks=False):
                    budget[0] -= 1
                    fnl = e.name.lower()
                    dot = fnl.rfind(".")
                    ext = fnl[dot + 1:] if dot >= 0 else ""
                    node.files.append((e.name, fnl, ext))
                    if fnl in NOTABLE_FILES:
                        node.notable.add(fnl)
                    # 分卷包推断：`x.pfs.002` / `x.7z.001` 这类，除末位数字外
                    # 再登记一层真实扩展名，否则分卷后的封包会被完全漏判。
                    if ext.isdigit() and 1 <= len(ext) <= 3:
                        dot2 = fnl.rfind(".", 0, dot)
                        if dot2 >= 0:
                            inner = fnl[dot2 + 1:dot]
                            if inner.isalnum():
                                node.files.append((e.name, fnl, inner))
            except OSError:
                continue
    return node


def hint_tags(text: str) -> list[str]:
    t = text.lower()
    out = []
    if any(h in t for h in CG_SAVE_HINTS):
        out.append("cg_save_bundled")
    if any(h in t for h in SAVE_DIR_HINTS):
        out.append("save_dir")
    return out


def aggregate(node: Node, depth: int = 0) -> None:
    """后序聚合：扩展名 / 特征文件 / 目录名 / 解锁线索 汇总到父节点。"""
    for _o, l, ext in node.files:
        if ext:
            node.ext[ext] += 1
        for h in hint_tags(l):
            node.hints.add(h)
    for c in node.dirs:
        aggregate(c, depth + 1)
        for k, v in c.ext.items():
            node.ext[k] += v
        node.notable |= c.notable
        node.dirnames |= c.dirnames
        node.hints |= c.hints
        if depth + 1 <= DIRNAME_DEPTH:
            node.dirnames.add(c.name.lower())
        for h in hint_tags(c.name):
            node.hints.add(h)


class View:
    """给规则表用的只读查询接口，方法名对齐 STool `ScanCtx`。"""

    def __init__(self, n: Node):
        self.n = n
        self._rootfiles = {l for _o, l, _e in n.files}
        self._rootdirs = {c.name.lower() for c in n.dirs}
        self._exes = [l for _o, l, _e in n.files if l.endswith(".exe")]

    def has_ext(self, e: str) -> bool:
        return self.n.ext.get(e, 0) > 0

    def ext_count(self, e: str) -> int:
        return self.n.ext.get(e, 0)

    def ext_sum(self, es) -> int:
        return sum(self.n.ext.get(e, 0) for e in es)

    def has_root_file(self, name: str) -> bool:
        return name.lower() in self._rootfiles

    def has_root_dir(self, name: str) -> bool:
        return name.lower() in self._rootdirs

    def root_dirs_ending(self, sfx: str) -> list[str]:
        sfx = sfx.lower()
        return sorted(d for d in self._rootdirs if d.endswith(sfx))

    def root_files_ending(self, sfx: str) -> list[str]:
        sfx = sfx.lower()
        return sorted(f for f in self._rootfiles if f.endswith(sfx))

    def exe_names(self) -> list[str]:
        return list(self._exes)

    def any_exe_contains(self, sub: str) -> bool:
        sub = sub.lower()
        return any(sub in e for e in self._exes)

    def has_file_named(self, name: str) -> bool:
        n = name.lower()
        return n in self._rootfiles or n in self.n.notable

    def has_dir(self, name: str) -> bool:
        return name.lower() in self.n.dirnames

    def has_dir_starting_with(self, prefix: str) -> bool:
        p = prefix.lower()
        return any(d.startswith(p) for d in self.n.dirnames)


# ===========================================================================
# 引擎识别规则表（对齐 STool：正式插件 + recognize.rs 识别器）
# 规则元组: (kind, arg, pts) / (kind, arg, n, pts)
#   ext / ext_any / ext_n / root_file / root_dir / root_dir_ends /
#   dir / dir_prefix / exe_contains / file_named
# ===========================================================================
ENGINES = [
    # ---------------- 正式插件（STool 有原生解析器） ----------------
    dict(id="renpy", name="Ren'Py", family="Ren'Py", prio=90, plugin=True, rules=[
        ("root_dir", "renpy", 45),
        ("ext", "rpa", 30),
        ("ext_calc", "rpyc_rpym", 25),
        ("ext", "rpy", 20),
        ("renpy_lib", None, 15),
        ("renpy_gui", None, 10),
    ], variant=""),
    dict(id="rpgmaker_mv", name="RPG Maker MV / MZ", family="RPG Maker", prio=85, plugin=True, rules=[
        ("file_named", "actors.json", 65),
        ("ext_calc", "mv_enc", 25),
        ("file_named", "game.rpgproject", 15),
        ("dir", "js", 10),
    ], variant=""),
    dict(id="rpgmaker_rgss", name="RPG Maker XP / VX / VX Ace", family="RPG Maker", prio=80, plugin=True, rules=[
        ("ext_any_rgss", None, 70),
        ("root_dir_multi", ["data", "graphics", "audio"], 10),
        ("ext_calc", "rgss_data", 50),
    ], variant=""),
    dict(id="kirikiri", name="KiriKiri / KAG", family="KiriKiri", prio=75, plugin=True, rules=[
        ("ext", "xp3", 70),
        ("exe_contains", "krkr", 20),
        ("kirikiri_loose", None, 45),
        ("file_named_any", ["config.tjs", "startup.tjs"], 25),
    ], variant=""),
    dict(id="godot", name="Godot", family="Godot", prio=70, plugin=True, rules=[
        ("ext", "pck", 70),
        ("godot_project", None, 65),
    ], variant=""),
    dict(id="nscripter", name="NScripter / ONScripter", family="NScripter", prio=65, plugin=True, rules=[
        ("file_named", "nscript.dat", 70),
        ("ext_any", ["nsa", "sar"], 20),
        ("exe_contains", "ons", 15),
    ], variant=""),
    dict(id="tyrano", name="TyranoBuilder / TyranoScript", family="Tyrano", prio=60, plugin=True, rules=[
        ("root_dir", "tyrano", 60),
        ("root_file", "index.html", 25),
        ("ext", "ks", 15),
        ("dir", "scenario", 10),
        ("file_named", "config.tjs", 10),
    ], variant=""),
    dict(id="html_game", name="HTML / JS / Electron", family="Web", prio=55, plugin=True, rules=[
        ("ext", "asar", 70),
        ("html_index", None, 60),
    ], variant=""),
    dict(id="wolf", name="Wolf RPG Editor (ウディタ)", family="Wolf RPG", prio=72, plugin=True, rules=[
        ("ext", "wolf", 75),
        ("wolf_basicdata", None, 45),
    ], variant=""),
    dict(id="unity", name="Unity", family="Unity", prio=78, plugin=True, rules=[
        ("root_dir_ends", "_data", 70),
        ("root_file", "unityplayer.dll", 60),
        ("unity_mono", None, 25),
    ], variant=""),

    # ---------------- 识别器（仅识别 + 委派外部工具） ----------------
    dict(id="bgi_ethornell", name="Ethornell / BGI（Buriko）", family="BGI", prio=36, plugin=False, rules=[
        ("file_named", "bregexp.dll", 65),      # Buriko 特征 DLL（BRegExp）
        ("exe_contains", "bgi", 60),
        ("file_named", "bgi.gdb", 60),          # BGI 的 SDC FORMAT 1.00 配置/存档
        ("file_named", "bgi.db", 60),
        ("file_named", "エンジン設定.exe", 50),  # ωstar 等重打包的 BGI 引擎设置工具
        ("file_named", "enginesetting.exe", 45),
        ("ext", "arc", 40),
        ("ext", "pso", 20),                     # BGI 着色器
        ("ext", "pack", 25),                    # BGI 早期封包 GameData/*.pack
        ("dir", "bgm", 15),
    ], variant=""),
    dict(id="siglus", name="SiglusEngine", family="Siglus", prio=32, plugin=False, rules=[
        ("root_file", "gameexe.dat", 55), ("ext", "pak", 30), ("ext", "g00", 15),
    ], variant=""),
    dict(id="reallive", name="RealLive（VisualArts / Key 旧作）", family="RealLive", prio=34, plugin=False, rules=[
        ("root_file", "gameexe.ini", 50), ("ext", "g00", 30), ("root_file", "seen.txt", 25),
    ], variant=""),
    dict(id="majiro", name="Majiro", family="Majiro", prio=33, plugin=False, rules=[
        ("ext", "mjo", 65), ("exe_contains", "majiro", 40), ("ext", "arc", 20),
    ], variant=""),
    dict(id="yuris", name="YU-RIS", family="YU-RIS", prio=33, plugin=False, rules=[
        ("ext", "ypf", 65), ("ext", "ybn", 20), ("ext", "yui", 15),
    ], variant=""),
    dict(id="catsystem2", name="CatSystem2", family="CatSystem2", prio=32, plugin=False, rules=[
        ("ext", "int", 45), ("ext", "cst", 25), ("ext", "noa", 15),
    ], variant=""),
    dict(id="artemis", name="Artemis Engine", family="Artemis", prio=32, plugin=False, rules=[
        ("ext", "pfs2", 65),
        ("ext_n", "pfs", 2, 65),   # `x.pfs` + `x.pfs.000/001` 分卷 = Artemis 独有形态
        ("ext_n", "pfs", 1, 38),   # 单个 .pfs 与 BGI 同名歧义，只作疑似
        ("ext", "asb", 25),
    ], variant=""),
    dict(id="alicesoft", name="AliceSoft System (.ald)", family="AliceSoft", prio=34, plugin=False, rules=[
        ("ext", "ald", 65), ("ext", "afa", 40), ("ext", "alk", 20),
    ], variant=""),
    dict(id="livemaker", name="LiveMaker", family="LiveMaker", prio=31, plugin=False, rules=[
        ("ext", "prt", 55), ("ext", "grp", 30), ("ext", "grs", 15),
    ], variant=""),
    dict(id="nitroplus", name="Nitroplus (.npk / .npa)", family="Nitroplus", prio=31, plugin=False, rules=[
        ("ext_any", ["npk", "npa"], 65), ("ext", "scr", 20),
    ], variant=""),
    dict(id="rpgmaker2k", name="RPG Maker 2000 / 2003", family="RPG Maker", prio=46, plugin=False, rules=[
        ("ext", "ldb", 60), ("ext_n", "lmt", 1, 30), ("exe_contains", "rpg_rt", 30),
    ], variant=""),
    dict(id="unreal", name="Unreal Engine", family="Unreal", prio=40, plugin=False, rules=[
        ("ext_any", ["utoc", "ucas"], 65),
        ("ext", "uasset", 45),
        ("dir", "engine", 45),        # UE 的 Engine/ 目录（几乎所有 UE 游戏都有）
        ("dir", "binaries", 25),      # <项目>/Binaries/Win64/
        ("dir", "content", 10),
    ], variant=""),
    dict(id="love2d", name="LÖVE (Love2D)", family="LÖVE", prio=30, plugin=False, rules=[
        ("ext", "love", 65), ("root_file", "main.lua", 30), ("ext", "lua", 15),
    ], variant=""),
    dict(id="gamemaker", name="GameMaker Studio", family="GameMaker", prio=30, plugin=False, rules=[
        ("root_file", "data.win", 65), ("ext", "win", 15),
    ], variant=""),
    dict(id="cocos2dx", name="Cocos2d-x / JSB", family="Cocos2d-x", prio=29, plugin=False, rules=[
        ("file_named", "libcocos2d.dll", 65),   # cocos2d-x Windows 导出独有 DLL
        ("dir", "resources", 15),
        ("dir", "plugins", 10),
        ("dir", "script", 10),
    ], variant=""),
    dict(id="purple", name="Purple Software (.lpk)", family="Purple", prio=28, plugin=False, rules=[
        ("ext", "lpk", 60),
    ], variant=""),

    # ---------------- 本脚本补充（STool 未收录，供路线图参考） ----------------
    dict(id="flash", name="Flash / SWF", family="Flash", prio=20, plugin=False, rules=[
        ("ext", "swf", 60),
    ], variant=""),
    dict(id="qsp", name="QSP (Quest Soft Player)", family="QSP", prio=20, plugin=False, rules=[
        ("ext_any", ["qsp", "qsps"], 60),
    ], variant=""),
]


def detect(v: View, eng: dict) -> tuple[int, list[str], str]:
    """返回 (score, evidence, variant)。规则求值与 STool 保持同样的口径。"""
    pts = 0
    ev: list[str] = []
    variant = ""
    for r in eng["rules"]:
        kind = r[0]
        if kind == "ext":
            e, p = r[1], r[2]
            if v.has_ext(e):
                pts += p
                ev.append(f"*.{e} × {v.ext_count(e)}")
        elif kind == "ext_n":
            e, n, p = r[1], r[2], r[3]
            if v.ext_count(e) >= n:
                pts += p
                ev.append(f"*.{e} × {v.ext_count(e)}")
        elif kind == "ext_any":
            es, p = r[1], r[2]
            for e in es:
                if v.has_ext(e):
                    pts += p
                    ev.append(f"*.{e} × {v.ext_count(e)}")
                    break
        elif kind == "root_file":
            f, p = r[1], r[2]
            if v.has_root_file(f):
                pts += p
                ev.append(f"根目录 {f}")
        elif kind == "root_dir":
            d, p = r[1], r[2]
            if v.has_root_dir(d):
                pts += p
                ev.append(f"{d}/ 目录")
        elif kind == "root_dir_multi":
            hits = [d for d in r[1] if v.has_root_dir(d)]
            if hits:
                pts += r[2] * len(hits)
                ev.append("/".join(f"{d}" for d in hits) + "/ 目录")
        elif kind == "root_dir_ends":
            s, p = r[1], r[2]
            ds = v.root_dirs_ending(s)
            if ds:
                pts += p
                ev.append(f"{ds[0]}/ 目录（引擎数据目录）")
        elif kind == "dir":
            d, p = r[1], r[2]
            if v.has_dir(d):
                pts += p
                ev.append(f"{d}/ 目录")
        elif kind == "dir_prefix":
            pf, p = r[1], r[2]
            if v.has_dir_starting_with(pf):
                pts += p
                ev.append(f"{pf}*/ 目录")
        elif kind == "exe_contains":
            sub, p = r[1], r[2]
            if v.any_exe_contains(sub):
                hit = next((e for e in v.exe_names() if sub in e), "")
                pts += p
                ev.append(f"{hit}")
        elif kind == "file_named":
            f, p = r[1], r[2]
            if v.has_file_named(f):
                pts += p
                ev.append(f)
        elif kind == "file_named_any":
            for f in r[1]:
                if v.has_file_named(f):
                    pts += r[2]
                    ev.append(f)
                    break
        # ---- 复合判据 ----
        elif kind == "ext_calc":
            tag = r[1]
            if tag == "rpyc_rpym":
                n = v.ext_sum(["rpyc", "rpym"])
                if n:
                    pts += 25
                    ev.append(f"{n} 个 .rpyc/.rpym 脚本")
            elif tag == "mv_enc":
                n = v.ext_sum(["rpgmvp", "rpgmvo", "rpgmvm", "png_", "ogg_", "m4a_"])
                if n:
                    pts += 25
                    ev.append(f"{n} 个 MV/MZ 加密素材")
            elif tag == "rgss_data":
                n = v.ext_sum(["rxdata", "rvdata", "rvdata2"])
                if n:
                    pts += 50
                    ev.append(f"{n} 个 .rxdata/.rvdata/.rvdata2 游戏数据（已解包）")
        elif kind == "ext_any_rgss":
            for e, lab in (("rgssad", "RGSS1"), ("rgss2a", "RGSS1"), ("rgss3a", "RGSS3")):
                if v.has_ext(e):
                    pts += 70
                    ev.append(f".{e} 封包")
                    variant = lab
        elif kind == "renpy_lib":
            if v.has_root_dir("lib") and v.has_dir_starting_with("py"):
                pts += 15
                ev.append("lib/py*-windows-* 运行时")
        elif kind == "renpy_gui":
            if v.has_dir("gui") and v.has_dir("images"):
                pts += 10
                ev.append("game/gui + game/images 资源结构")
        elif kind == "kirikiri_loose":
            if v.has_root_dir("tyrano"):
                continue  # 交给 Tyrano 判定
            n = v.ext_sum(["tjs", "ks"])
            if n:
                pts += 45 if n >= 5 else 20
                ev.append(f"{n} 个 .tjs/.ks 明文脚本（未打包）")
        elif kind == "godot_project":
            if v.has_file_named("project.godot"):
                pts += 15 if v.has_ext("pck") else 65
                ev.append("project.godot 工程文件")
            elif v.has_dir(".godot"):
                pts += 25
                ev.append(".godot/ 导入缓存")
        elif kind == "html_index":
            owned_tyrano = v.has_root_dir("tyrano")
            owned_mv = v.has_file_named("actors.json")
            if v.has_root_file("index.html") and not owned_tyrano and not owned_mv:
                pts += 60
                ev.append("index.html Web 入口")
                if v.has_dir("assets"):
                    pts += 15
                    ev.append("assets/ 资源目录")
                if v.has_dir("js"):
                    pts += 10
                    ev.append("js/ 脚本目录")
        elif kind == "wolf_basicdata":
            if v.has_root_dir("data") and v.has_dir("basicdata"):
                pts += 45
                ev.append("Data/BasicData 数据布局")
                if v.has_root_file("game.exe"):
                    pts += 15
                    ev.append("Game.exe 运行时")
        elif kind == "unity_mono":
            if v.has_file_named("assembly-csharp.dll"):
                pts += 25
                ev.append("Managed/Assembly-CSharp.dll（Mono 版）")
                variant = "Mono"
            elif v.has_file_named("gameassembly.dll"):
                pts += 25
                ev.append("GameAssembly.dll（IL2CPP 版）")
                variant = "IL2CPP"
    return pts, ev, variant


# ===========================================================================
# 伪装识别
# ===========================================================================
RE_RANDOM_PREFIX = re.compile(r"^[A-Za-z0-9]{6,12}_")
RE_BRACKET_TAG = re.compile(r"^\[[^\]]{1,40}\]\s*")
RE_PC_PREFIX = re.compile(r"^(?:pc|PC|PC版|电脑版)\s+")
RE_VERSION_SUFFIX = re.compile(r"[\s_\-]*(?:ver|Ver|VER|v|V)\s*\.?\s*\d[\w.\-]*$")
RE_NUMVER_SUFFIX = re.compile(r"[\s_\-]\d+(?:\.\d+){1,3}[a-z]?$")
RE_LOC_SUFFIX = re.compile(
    r"[\s_\-]*(?:汉化版|汉化|内置汉化|中文版|官中|生肉|原版|重置版|重制版|完结|完全版|最终版|"
    r"DL版|正式版|体验版|完整版|破解版)$"
)
RE_PART_SUFFIX = re.compile(r"[\s_\-]*[_\-]\d{1,2}$")


def clean_title(name: str) -> tuple[str, list[str]]:
    """从目录名剥离常见伪装，返回 (干净标题, 命中的伪装标签)。"""
    tags: list[str] = []
    s = name.strip().strip("\u3000 ")

    m = RE_RANDOM_PREFIX.match(s)
    if m and len(s) > len(m.group(0)) and not s[len(m.group(0))].isascii():
        tags.append("random_prefix")
        s = s[m.end():]

    while True:
        m = RE_BRACKET_TAG.match(s)
        if not m:
            break
        tags.append("bracket_tag")
        s = s[m.end():]

    m = RE_PC_PREFIX.match(s)
    if m:
        tags.append("pc_prefix")
        s = s[m.end():]

    for pat, tag in (
        (RE_VERSION_SUFFIX, "version_suffix"),
        (RE_NUMVER_SUFFIX, "numver_suffix"),
        (RE_LOC_SUFFIX, "localized_suffix"),
    ):
        m = pat.search(s)
        if m and len(s[: m.start()].strip()) >= 3:
            tags.append(tag)
            s = s[: m.start()]

    if RE_PART_SUFFIX.search(s) and len(s) > 6:
        tags.append("part_suffix")

    return s.strip().strip("\u3000 "), sorted(set(tags))


# ===========================================================================
# 游戏根目录下钻
# ===========================================================================
WRAPPER_DIR_NAMES = {"pc", "game", "games", "游戏", "bin", "release", "build",
                     "windows", "windowsnoeditor", "win", "data", "app"}


def pick_candidate_dirs(top: Node, max_descent: int) -> list[Node]:
    """顶目录 + 若干「像游戏根」的子孙目录，控制候选数量。"""
    cands: list[Node] = [top]
    stack = [(top, 1)]
    while stack and len(cands) < 300:
        n, d = stack.pop()
        if d > max_descent:
            continue
        for c in n.dirs:
            name_l = c.name.lower()
            looks = (
                bool(c.notable)
                or any(l.endswith(".exe") for _o, l, _e in c.files)
                or name_l in WRAPPER_DIR_NAMES
                or any(c.ext.get(e, 0) > 0 for e in STRONG_EXTS)
            )
            if looks:
                cands.append(c)
            stack.append((c, d + 1))
    return cands


def score_node(n: Node) -> tuple[int, dict | None, list[tuple[int, dict, list[str], str]]]:
    """对所有引擎打分，返回 (最高分, 最高分引擎, 全部结果)。"""
    v = View(n)
    results: list[tuple[int, dict, list[str], str]] = []
    for eng in ENGINES:
        pts, ev, var = detect(v, eng)
        if pts > 0:
            results.append((pts, eng, ev, var))
    results.sort(key=lambda x: (-x[0], -x[1]["prio"]))
    if not results:
        return 0, None, []
    return results[0][0], results[0][1], results


def confidence_of(score: int) -> str:
    if score >= 85:
        return "high"
    if score >= DETECT_LINE:
        return "medium"
    if score >= 30:
        return "low"
    return "none"


# ---------------------------------------------------------------------------
# magic 兜底
# ---------------------------------------------------------------------------
def sniff_magic(top: Node, budget: int) -> tuple[str | None, str, str | None]:
    """抽样读文件头。返回 (engine_id, 证据, 提示)。

    只在文件名规则完全识别不出时调用，避免无谓 IO。
    """
    cands: list[tuple[int, str]] = []
    # 明显不是"引擎证据"的通用文件类型（跳过，省 IO）
    GENERIC = {
        "dll", "ttf", "otf", "ini", "txt", "md", "xlsx", "xls", "docx", "pdf",
        "json", "xml", "cfg", "log", "db", "sqlite", "bat", "cmd", "vbs",
        "lnk", "url", "ico", "cur", "sys", "pdb", "dmp", "csv",
    }

    def walk(n: Node):
        for orig, _l, ext in n.files:
            if (ext in MEDIA_EXTS or ext in ARCHIVE_EXTS or ext in GENERIC) and ext != "exe":
                continue
            p = os.path.join(n.path, orig)
            try:
                sz = os.path.getsize(p)
            except OSError:
                continue
            if sz < 32 * 1024:
                continue  # 太小的文件不可能是封包
            cands.append((sz, p))
        for c in n.dirs:
            walk(c)

    walk(top)
    # 大文件优先（封包/资源通常很大，改名后依然大）
    cands.sort(key=lambda x: -x[0])
    opened = 0
    for _sz, p in cands:
        if opened >= budget:
            break
        opened += 1
        try:
            with open(p, "rb") as f:
                head = f.read(8192)
        except OSError:
            continue
        if not head:
            continue
        for sig, off, eid, desc in MAGIC_SIGS:
            if head[off: off + len(sig)] == sig:
                return eid, f"{desc}（{os.path.basename(p)}）", os.path.basename(p)
        for sig, lim, eid, desc in MAGIC_CONTAINS:
            if sig in head[:lim]:
                return eid, f"{desc}（{os.path.basename(p)}）", os.path.basename(p)
    # 没有任何收录引擎的魔数：探测"改了扩展名的 exe"
    for _sz, p in cands:
        if p.lower().endswith(".exe"):
            continue
        try:
            with open(p, "rb") as f:
                if f.read(2) == b"MZ":
                    return None, f"PE 可执行体但无 .exe 后缀（{os.path.basename(p)}）", "renamed_exe"
        except OSError:
            continue
    # 未收录但可辨认的自研封包魔数（给出线索，供人工排查）
    for _sz, p in cands[: budget * 2]:
        try:
            with open(p, "rb") as f:
                head = f.read(16)
        except OSError:
            continue
        for sig, label in UNKNOWN_MAGICS:
            if head[: len(sig)] == sig:
                return None, f"{label}（{os.path.basename(p)}）", "custom_pack"
    return None, "", None


UNKNOWN_MAGICS = [
    (b"PAC ", "PAC 自研封包（未收录引擎）"),
]


def big_root_files(node: Node, min_mb: int = 100) -> list[tuple[int, str]]:
    """根目录一级里体积很大的 **exe**（自解压/内嵌资源安装包线索）。"""
    out = []
    for orig, l, _e in node.files:
        if not l.endswith(".exe"):
            continue
        p = os.path.join(node.path, orig)
        try:
            sz = os.path.getsize(p)
        except OSError:
            continue
        if sz >= min_mb * 1024 * 1024:
            out.append((sz, orig))
    out.sort(reverse=True)
    return out


# ===========================================================================
# 引擎目录（含 CG 解锁能力），面向「解锁路线图」
# ===========================================================================
def unlock_catalog() -> dict:
    D = lambda **kw: kw
    cat = {
        "renpy": D(name="Ren'Py", family="Ren'Py", stool_plugin=True,
                   unlock_method="persistent（pickle 文本）+ 开发者控制台",
                   unlock_state="%AppData%/RenPy/<游戏内部名>/persistent",
                   unlock_support="feasible", auto=False,
                   note="persistent 多为可直接编辑的 pickle，改 cg_unlocked/seen_images；"
                        "亦可反编译 .rpyc 后开 console 执行解锁命令"),
        "rpgmaker_mv": D(name="RPG Maker MV / MZ", family="RPG Maker", stool_plugin=True,
                         unlock_method="save 存档（lz-string JSON）或直接改 data/*.json + 运行时 DevTools",
                         unlock_state="游戏目录 save/*.rmmzsave 或 %AppData%",
                         unlock_support="feasible", auto=False,
                         note="数据全明文，改 data/ JSON 或 F12 控制台最省事；存档为 lz-string 压缩 JSON"),
        "rpgmaker_rgss": D(name="RPG Maker XP / VX / VX Ace", family="RPG Maker", stool_plugin=True,
                           unlock_method="SaveNN.rxdata/rvdata2（Ruby Marshal）里的开关变量",
                           unlock_state="游戏目录 Save*.rxdata / .rvdata2",
                           unlock_support="feasible", auto=False,
                           note="可解析 Marshal 找出回廊解锁用的 $game_switches 并置位；STool 已有 pickle/Marshal 基础"),
        "rpgmaker2k": D(name="RPG Maker 2000 / 2003", family="RPG Maker", stool_plugin=False,
                        unlock_method="SaveNN.lsd 存档变量 / RPG_RT.ldb",
                        unlock_state="游戏目录 Save*.lsd",
                        unlock_support="manual", auto=False,
                        note="2000/2003 存档为二进制 .lsd，需专用编辑器或 EasyRPG 工具链"),
        "kirikiri": D(name="KiriKiri / KAG", family="KiriKiri", stool_plugin=True,
                      unlock_method="存档（.ksd/.sav）+ 脚本 flag（.ks/.tjs 可 grep 全局变量）",
                      unlock_state="游戏目录 save/ 与 .ks/.tjs 脚本",
                      unlock_support="manual", auto=False,
                      note="无统一存档格式；多数可在 .ks 里找 f.xxx / 全局变量，或直接改存档 flag"),
        "godot": D(name="Godot", family="Godot", stool_plugin=True,
                   unlock_method="user:// 存档（cfg/json）+ 解包 .pck 改 .gd 脚本",
                   unlock_state="%AppData%/Godot/app_userdata/<游戏名>/",
                   unlock_support="feasible", auto=False,
                   note="save 多为明文 json/cfg；也可解包 pck 改解锁逻辑后 repack"),
        "nscripter": D(name="NScripter / ONScripter", family="NScripter", stool_plugin=True,
                       unlock_method="save*.dat 存档变量 + nscript.dat 解密后改 globalon 标记",
                       unlock_state="游戏目录 save*.dat",
                       unlock_support="manual", auto=False,
                       note="nscript.dat 可按字节 XOR 还原为文本，搜索 *define/回廊变量"),
        "wolf": D(name="Wolf RPG Editor", family="Wolf RPG", stool_plugin=True,
                  unlock_method="SaveData/*.sav（WolfDec 解密）+ Data/BasicData 开关",
                  unlock_state="游戏目录 SaveData/",
                  unlock_support="manual", auto=False,
                  note="需 WolfDec 解密；开关多在 CommonEvent/变量表里"),
        "tyrano": D(name="TyranoBuilder / TyranoScript", family="Tyrano", stool_plugin=True,
                    unlock_method="KAG 存档（json）+ data/scenario/*.ks 剧本变量",
                    unlock_state="游戏目录 save/ 或 localStorage",
                    unlock_support="manual", auto=False,
                    note="与 KiriKiri 共用 KAG 语法，解锁逻辑在 .ks 的场景/标记里"),
        "html_game": D(name="HTML / JS / Electron", family="Web", stool_plugin=True,
                       unlock_method="localStorage / IndexedDB / .sol；Electron 解 app.asar 改 JS",
                       unlock_state="浏览器存储或 %AppData%/<游戏>/Local Storage",
                       unlock_support="feasible", auto=False,
                       note="源码即前端代码，F12 控制台或解 asar 改源码最直接"),
        "unity": D(name="Unity", family="Unity", stool_plugin=True,
                   unlock_method="PlayerPrefs 注册表（HKCU\\Software\\<company>\\<product>）键置 1",
                   unlock_state="注册表 + %USERPROFILE%/AppData/LocalLow/<company>/<product>/",
                   unlock_support="native", auto=True,
                   note="STool 已实现：散列自检 + 备份 + 回读校验；也可先找游戏自带全开 toggle"),
        "bgi_ethornell": D(name="Ethornell / BGI", family="BGI", stool_plugin=False,
                           unlock_method="存档 .sav + .arc 内脚本全局标记",
                           unlock_state="游戏目录 save/",
                           unlock_support="manual", auto=False, note="需解 .arc 后改脚本标记或改存档"),
        "siglus": D(name="SiglusEngine", family="Siglus", stool_plugin=False,
                    unlock_method=".sav 存档 + Scene.pak 脚本",
                    unlock_state="游戏目录 SAVEDATA/",
                    unlock_support="manual", auto=False, note="Key 系，解锁多在脚本 flag；GARbro 解包后可改"),
        "reallive": D(name="RealLive", family="RealLive", stool_plugin=False,
                      unlock_method="Seen.txt（已读记录）+ SaveNN.sav",
                      unlock_state="游戏目录 SaveData/ 与 Seen.txt",
                      unlock_support="feasible", auto=False,
                      note="Seen.txt 是已读/解锁记录，结构简单，改它可批量置已看"),
        "majiro": D(name="Majiro", family="Majiro", stool_plugin=False,
                    unlock_method="存档 + .mjo 脚本字节码",
                    unlock_state="游戏目录 save/", unlock_support="manual", auto=False, note="脚本为编译字节码，改开关较难"),
        "yuris": D(name="YU-RIS", family="YU-RIS", stool_plugin=False,
                   unlock_method="存档（.sav/.dat）",
                   unlock_state="游戏目录 save/", unlock_support="manual", auto=False, note="封包 .ypf 常带密钥"),
        "catsystem2": D(name="CatSystem2", family="CatSystem2", stool_plugin=False,
                        unlock_method="存档 + .int 脚本",
                        unlock_state="游戏目录 save/", unlock_support="manual", auto=False, note="脚本编译在 .int"),
        "artemis": D(name="Artemis Engine", family="Artemis", stool_plugin=False,
                     unlock_method="存档 + .pfs 内脚本",
                     unlock_state="游戏目录 save/", unlock_support="manual", auto=False, note=".asb 为脚本字节码"),
        "alicesoft": D(name="AliceSoft System", family="AliceSoft", stool_plugin=False,
                       unlock_method="System40.ain 脚本 + .asd 存档",
                       unlock_state="游戏目录 SaveData/", unlock_support="manual", auto=False, note="Ain 为脚本语言，可改解锁 flag"),
        "livemaker": D(name="LiveMaker", family="LiveMaker", stool_plugin=False,
                       unlock_method="存档 + .lsc/.lse 脚本",
                       unlock_state="游戏目录 save/", unlock_support="manual", auto=False, note=""),
        "nitroplus": D(name="Nitroplus", family="Nitroplus", stool_plugin=False,
                       unlock_method="存档 + .scr 脚本",
                       unlock_state="游戏目录 save/", unlock_support="manual", auto=False, note=""),
        "unreal": D(name="Unreal Engine", family="Unreal", stool_plugin=False,
                    unlock_method="SaveGame（.sav）二进制",
                    unlock_state="%LocalAppData%/<项目>/Saved/SaveGames/",
                    unlock_support="unknown", auto=False, note="SaveGame 为 UE 序列化格式，需引擎侧解析"),
        "love2d": D(name="LÖVE (Love2D)", family="LÖVE", stool_plugin=False,
                    unlock_method="love.filesystem 存档目录（明文 lua/json）",
                    unlock_state="%AppData%/LOVE/<游戏名>/",
                    unlock_support="feasible", auto=False, note="存档明文，改完即可"),
        "gamemaker": D(name="GameMaker Studio", family="GameMaker", stool_plugin=False,
                       unlock_method="data.win 字符串 + save.ini",
                       unlock_state="游戏目录 save.ini / %AppData%",
                       unlock_support="manual", auto=False, note="可用 UndertaleModTool 改 data.win 里的解锁变量"),
        "purple": D(name="Purple Software", family="Purple", stool_plugin=False,
                    unlock_method="存档 + .lpk 内脚本",
                    unlock_state="游戏目录 save/", unlock_support="manual", auto=False, note=""),
        "cocos2dx": D(name="Cocos2d-x / JSB", family="Cocos2d-x", stool_plugin=False,
                      unlock_method="Resources/ 或用户目录下的存档（多为 JSON/csv 明文）",
                      unlock_state="游戏目录 Resources/ 或 %APPDATA%",
                      unlock_support="manual", auto=False,
                      note="脚本为 jsb*.js，可在 Resources/script 里 grep 解锁相关变量"),
        "flash": D(name="Flash / SWF", family="Flash", stool_plugin=False,
                   unlock_method=".sol SharedObject（可编辑）+ SWF 反编译改代码",
                   unlock_state="%AppData%/Macromedia/Flash Player/#SharedObjects/<路径>/<游戏>.sol",
                   unlock_support="feasible", auto=False, note=".sol 有现成编辑器；FFDec 可反编译改逻辑"),
        "qsp": D(name="QSP", family="QSP", stool_plugin=False,
                 unlock_method=".qsps 明文源码 / 存档变量",
                 unlock_state="游戏目录 *.qsps 或 save", unlock_support="manual", auto=False,
                 note=".qsps 是明文文本，直接改条件即可"),
    }
    return cat


# ===========================================================================
# 单目录盘点
# ===========================================================================
def analyze(folder: str, args, catalog: dict) -> dict:
    name = os.path.basename(folder.rstrip("\\/"))
    rec: dict = {
        "folder": name,
        "path": folder,
        "id": "",
        "clean_name": name,
        "disguise": [],
        "kind": "unknown",
        "game_root": folder,
        "wrapped_in_subdir": False,
        "engine_id": "",
        "engine_name": "",
        "engine_family": "",
        "engine_variant": "",
        "engine_plugin": False,
        "confidence": "none",
        "score": 0,
        "evidence": [],
        "runners_up": [],
        "stool_support": "none",
        "cg_unlock": {},
        "unlock_priority": "unknown",
        "structure": {},
        "notes": [],
        "last_scan": datetime.now().astimezone().isoformat(timespec="seconds"),
    }

    clean, tags = clean_title(name)
    rec["clean_name"] = clean or name
    rec["disguise"] = list(tags)

    if not os.path.isdir(folder):
        rec["kind"] = "file"
        rec["notes"].append("非目录，跳过")
        return rec

    budget = [MAX_FILES]
    top = build_tree(folder, name, budget)
    aggregate(top)
    if top.truncated:
        rec["notes"].append(f"文件数超过 {MAX_FILES}，结果可能不完整")

    # ---- 选游戏根 ----
    cands = pick_candidate_dirs(top, args.max_descent)
    best_node, best_score, best_eng, best_res = top, -1, None, []
    for c in cands:
        sc, eng, res = score_node(c)
        if sc > best_score:
            best_node, best_score, best_eng, best_res = c, sc, eng, res
    chosen = best_node if best_score >= DETECT_LINE else top
    if best_score >= DETECT_LINE:
        chosen = best_node
    rec["game_root"] = chosen.path
    rec["wrapped_in_subdir"] = os.path.normcase(chosen.path) != os.path.normcase(folder)
    if rec["wrapped_in_subdir"]:
        rec["disguise"].append("wrapped_in_subdir")

    v = View(chosen)
    sc, eng, res = score_node(chosen)

    # ---- magic 兜底 ----
    custom_pack = False
    if sc < DETECT_LINE:
        eid, ev, hint = sniff_magic(chosen, args.magic_budget)
        if eid:
            rec["disguise"].append("renamed_ext")
            rec["notes"].append(f"文件名无特征，靠文件头识别：{ev}")
            fake = next((e for e in ENGINES if e["id"] == eid), None)
            if fake:
                res = [(max(sc, 60), fake, [ev], "")] + res
                sc, eng = max(sc, 60), fake
                rec["structure"]["magic_hint"] = ev
        elif hint == "renamed_exe":
            rec["disguise"].append("renamed_exe")
            rec["notes"].append(ev)
        elif hint == "custom_pack":
            custom_pack = True
            rec["notes"].append(f"检测到未收录的自研封包：{ev}（需人工确认引擎）")
            rec["structure"]["magic_hint"] = ev

    # ---- 分类 ----
    rec["score"] = sc
    rec["confidence"] = confidence_of(sc)
    if eng:
        rec["engine_id"] = eng["id"]
        rec["engine_name"] = eng["name"]
        rec["engine_family"] = eng["family"]
        rec["engine_plugin"] = bool(eng["plugin"])
        rec["evidence"] = next((e for s, g, e, _ in res if g["id"] == eng["id"]), [])
        rec["engine_variant"] = next((va for s, g, _e, va in res if g["id"] == eng["id"]), "")
        rec["runners_up"] = [
            {"engine_id": g["id"], "name": g["name"], "score": s}
            for s, g, _e, _v in res if g["id"] != eng["id"] and s >= 30
        ][:4]

    total_files = max(0, sum(chosen.ext.values()))
    media = sum(chosen.ext.get(e, 0) for e in MEDIA_EXTS)
    media_ratio = media / total_files if total_files else 0.0
    has_exe = any(l.endswith(".exe") for _o, l, _e in chosen.files) or any(
        l.endswith(".exe") for c in chosen.dirs for _o, l, _e in c.files
    )
    archives = sum(chosen.ext.get(e, 0) for e in ARCHIVE_EXTS)

    name_l = name.lower()
    is_tool = any(k in name_l for k in TOOL_KEYWORDS)

    if is_tool:
        rec["kind"] = "tool"  # 工具优先（Cheat Engine / MTool / 汉化工具 …）
    elif sc >= DETECT_LINE:
        rec["kind"] = "game"
    elif total_files == 0:
        rec["kind"] = "empty"
        rec["notes"].append("目录为空（可能只建了壳，内容未下载/未解压）")
    elif (not has_exe and archives == 0 and media == 0 and total_files <= 20
          and set(chosen.ext) <= ALLOWED_JUNK_EXTS):
        rec["kind"] = "junk"
        rec["notes"].append("仅含说明/广告等文本与链接，无游戏内容（推广目录）")
    elif archives > 0 and not has_exe:
        rec["kind"] = "archive_only"
        rec["notes"].append("仅有压缩包，未解压（需先解压再识别）")
    elif media_ratio >= 0.8 and not has_exe:
        rec["kind"] = "media_set"
        rec["notes"].append("以图片/视频为主且无主程序，判为素材包（画集/写真/视频）")
    elif any(h in name_l for h in IMAGE_ONLY_SET_HINTS) and not has_exe:
        rec["kind"] = "media_set"
        rec["notes"].append("目录名疑似画集/写真的素材包")
    elif custom_pack and has_exe:
        rec["kind"] = "game_unknown_engine"
        rec["notes"].append("有主程序但引擎未收录，需人工确认")
    elif has_exe:
        rec["kind"] = "unknown"
    else:
        rec["kind"] = "no_exe"

    # 大体积 exe（可能是自解压/内嵌资源），提示先释放
    if rec["kind"] not in GAME_KINDS:
        bigs = big_root_files(chosen, 64)
        if bigs:
            mb = bigs[0][0] // (1024 * 1024)
            rec["disguise"].append("self_extract_exe")
            rec["notes"].append(f"根目录有 {mb}MB 的大 exe（{bigs[0][1]}），可能是自解压包，需先运行释放资源")
        # `<游戏>.exe` + `<游戏>.console.exe` 是封装/内嵌资源的常见形态
        exes = v.exe_names()
        if any(e.endswith(".console.exe") for e in exes) and len(exes) <= 4:
            rec["disguise"].append("wrapped_exe")
            rec["notes"].append("仅有 <游戏>.exe + <游戏>.console.exe，资源疑内嵌在 exe 中，需先运行或用解包工具释放")

    # ---- 结构摘要（给解锁用）----
    save_dirs = sorted(d for d in chosen.dirnames if any(h in d for h in SAVE_DIR_HINTS))
    rec["structure"] = {
        "total_files": total_files,
        "media_ratio": round(media_ratio, 3),
        "has_exe": has_exe,
        "exe_count": len(v.exe_names()),
        "exe_names": v.exe_names()[:6],
        "archive_count": archives,
        "top_exts": [f".{e}×{c}" for e, c in chosen.ext.most_common(8)],
        "notable_files": sorted(chosen.notable),
        "data_dirs": v.root_dirs_ending("_data"),
        "save_dirs": save_dirs[:6],
        "hints": sorted(chosen.hints),
    }

    # ---- Unity 专属：app.info 公司/产品（PlayerPrefs 路径）----
    if rec["engine_id"] == "unity":
        info = read_app_info(chosen)
        if info:
            rec["structure"]["unity_app_info"] = info
            rec["cg_unlock"] = {
                "registry_path": rf"HKCU\Software\{info.get('company','?')}\{info.get('product','?')}"
            }
        # 只有 UnityPlayer.dll 却没有 *_Data → 目录被改名
        if v.has_root_file("unityplayer.dll") and not v.root_dirs_ending("_data"):
            rec["disguise"].append("renamed_data_dir")
            rec["notes"].append("有 UnityPlayer.dll 但无 *_Data 目录，_Data 很可能被改名")

    # ---- CG 解锁能力 ----
    eid = rec["engine_id"]
    if eid and eid in catalog:
        c = dict(catalog[eid])
        support = c.pop("unlock_support")
        c.pop("auto")
        c["support"] = support
        c["stool_can_auto_unlock"] = support == "native"
        base = rec.get("cg_unlock", {})
        if isinstance(base, dict):
            c.update(base)
        rec["cg_unlock"] = c
        rec["stool_support"] = "native" if eng and eng["plugin"] else "recognize"
    elif eid:
        rec["stool_support"] = "recognize"
        rec["cg_unlock"] = {"support": "unknown", "note": "未收录的引擎，需人工排查封包/存档格式",
                            "unlock_method": "通用：自带全CG存档替换 / 内存修改（Cheat Engine）",
                            "unlock_state": "见 structure.save_dirs"}
    else:
        rec["cg_unlock"] = {"support": "unknown", "note": "未识别引擎，需人工排查",
                            "unlock_method": "通用：自带全CG存档替换 / 内存修改（Cheat Engine）",
                            "unlock_state": "见 structure.save_dirs"}

    # 自带全CG存档 = 最省事的解锁路径（直接覆盖存档目录即可）
    cu = rec["cg_unlock"]
    sup = cu.get("support", "unknown") if isinstance(cu, dict) else "unknown"
    bundled = "cg_save_bundled" in chosen.hints
    if bundled and isinstance(cu, dict):
        cu["bundled_cg_save"] = True
        rec["notes"].append("目录内自带「全CG存档」，覆盖到存档目录即可解锁（最省事）")
    if bundled:
        rec["unlock_priority"] = "quick"
    elif sup == "native":
        rec["unlock_priority"] = "auto"
    elif sup == "feasible":
        rec["unlock_priority"] = "feasible"
    elif sup == "manual":
        rec["unlock_priority"] = "manual"
    else:
        rec["unlock_priority"] = "unknown"

    # ---- id ----
    rec["id"] = make_id(name, rec["clean_name"])

    # 兜底备注
    if rec["kind"] == "game" and not rec["engine_plugin"] and eng is not None:
        rec["notes"].append("该引擎 STool 仅能识别，解包/处理需外部工具（GARbro 等）")
    if sc > 0 and sc < DETECT_LINE and rec["kind"] == "game":
        rec["notes"].append(f"分数 {sc} 未达判定线 {DETECT_LINE}，建议人工复核")

    return rec


def make_id(folder: str, clean: str) -> str:
    """稳定短 id：拉丁/数字保留，CJK 用拼音无关的 hash 后缀避免重名。"""
    base = re.sub(r"[^0-9A-Za-z]+", "_", clean).strip("_").lower() or "game"
    base = base[:32]
    import hashlib
    h = hashlib.sha1(folder.encode("utf-8")).hexdigest()[:6]
    return f"{base}_{h}"


def read_app_info(node: Node) -> dict | None:
    """Unity: 读 <游戏>_Data/app.info 前两行 = company / product。"""
    for c in node.dirs:
        if c.name.lower().endswith("_data"):
            p = os.path.join(c.path, "app.info")
            try:
                with open(p, "r", encoding="utf-8", errors="replace") as f:
                    lines = [ln.strip() for ln in f.read().splitlines()]
                if len(lines) >= 2 and lines[0] and lines[1]:
                    return {"company": lines[0], "product": lines[1]}
            except OSError:
                pass
    # 有些发行版把 app.info 放在 _Data 之外，尝试根目录
    for _o, l, _e in node.files:
        if l == "app.info":
            try:
                with open(os.path.join(node.path, _o), "r", encoding="utf-8", errors="replace") as fh:
                    lines = [ln.strip() for ln in fh.read().splitlines()]
                if len(lines) >= 2:
                    return {"company": lines[0], "product": lines[1]}
            except OSError:
                pass
    return None


# ===========================================================================
# 汇总与输出
# ===========================================================================
GAME_KINDS = {"game", "game_unknown_engine"}


def summarize(records: list[dict], catalog: dict) -> dict:
    games = [r for r in records if r["kind"] in GAME_KINDS]
    by_engine = Counter(r["engine_id"] or "(未识别)" for r in games)
    by_family = Counter(r["engine_family"] or "(未识别)" for r in games)
    by_kind = Counter(r["kind"] for r in records)
    by_support = Counter(r["cg_unlock"].get("support", "unknown") for r in games)
    by_priority = Counter(r.get("unlock_priority", "unknown") for r in games)

    # 路线图：按引擎统计「STool 能否自动解锁」+ 缺口排序
    roadmap = []
    for eid, cnt in by_engine.most_common():
        meta = catalog.get(eid, {})
        support = meta.get("unlock_support", "unknown") if eid in catalog else "unknown"
        roadmap.append({
            "engine_id": eid,
            "engine": meta.get("name", eid) if eid in catalog else eid,
            "family": meta.get("family", "") if eid in catalog else "",
            "stool_plugin": bool(meta.get("stool_plugin")) if eid in catalog else False,
            "unlock_support": support,
            "auto_unlock": support == "native",
            "game_count": cnt,
            "games": sorted(r["folder"] for r in games if (r["engine_id"] or "(未识别)") == eid),
            "note": meta.get("note", "") if eid in catalog else "引擎未收录，需人工排查",
        })

    # 快捷解锁：目录自带全CG存档
    quick = sorted(r["folder"] for r in games if r.get("unlock_priority") == "quick")

    return {
        "total_entries": len(records),
        "games": len(games),
        "by_kind": dict(by_kind.most_common()),
        "by_engine": dict(by_engine.most_common()),
        "by_family": dict(by_family.most_common()),
        "by_unlock_support": dict(by_support.most_common()),
        "by_unlock_priority": dict(by_priority.most_common()),
        "quick_unlock_games": quick,
        "roadmap": roadmap,
    }


def write_json(path: str, payload: dict):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(payload, f, ensure_ascii=False, indent=2)


def write_csv(path: str, records: list[dict]):
    cols = ["folder", "clean_name", "kind", "engine_id", "engine_name", "engine_family",
            "engine_variant", "confidence", "score", "stool_support", "unlock_priority",
            "cg_unlock_support", "cg_unlock_method", "bundled_cg_save", "disguise",
            "game_root", "evidence", "notes"]
    with open(path, "w", encoding="utf-8-sig", newline="") as f:
        w = csv.writer(f)
        w.writerow(cols)
        for r in records:
            cu = r.get("cg_unlock", {}) or {}
            w.writerow([
                r["folder"], r["clean_name"], r["kind"], r["engine_id"], r["engine_name"],
                r["engine_family"], r["engine_variant"], r["confidence"], r["score"],
                r["stool_support"], r.get("unlock_priority", ""),
                cu.get("support", ""), cu.get("unlock_method", ""),
                "是" if cu.get("bundled_cg_save") else "",
                ";".join(r["disguise"]), r["game_root"], ";".join(r["evidence"]),
                ";".join(r["notes"]),
            ])


def write_md(path: str, payload: dict, records: list[dict], roots: list[str]):
    s = payload["summary"]
    L: list[str] = []
    A = L.append
    A("# 游戏引擎盘点报告（STool「全 CG 解锁」路线图输入）\n")
    A(f"- 生成时间：{payload['generated_at']}")
    A(f"- 扫描根目录：{'、'.join(roots)}")
    A(f"- 条目总数：{s['total_entries']}（识别为游戏 {s['games']}）\n")

    A("## 一、总览\n")
    A("| 维度 | 分布 |")
    A("| --- | --- |")
    A(f"| 类型 | {fmt_counter(s['by_kind'])} |")
    A(f"| 引擎家族 | {fmt_counter(s['by_family'])} |")
    A(f"| CG 解锁可自动化程度 | {fmt_counter(s['by_unlock_support'])} |")
    A(f"| 解锁优先级 | {fmt_counter(s['by_unlock_priority'])} |")
    A("")
    A("> 类型口径：`game`=已确认引擎；`game_unknown_engine`=有主程序但引擎未收录（两者都计入「游戏」）；")
    A("> `media_set`=画集/写真/视频素材；`tool`=工具；`archive_only`=仅压缩包未解压；`junk`=推广目录。")
    A("> 解锁优先级：★quick=目录自带全CG存档（最省事）；auto=STool 可自动解锁；feasible=自动化可行；manual=需人工。")
    A("")

    A("## 一·五、解锁总原则（通用，优先照此执行）\n")
    A("1. **优先用游戏自带的「全 CG 存档」**：本盘点已标出哪些目录自带（见下方「快捷解锁」），")
    A("   把存档文件覆盖到存档目录即可，不用碰游戏本体。")
    A("2. **其次找游戏内的隐藏全开开关**：不少作品在设置/隐藏界面有 toggle，打开即全解锁。")
    A("3. **再考虑改存档**：运行游戏存一次档以创建存档目录，再按引擎对应的存档格式改（见目录字段）。")
    A("4. **最后才改脚本/封包**：解包 → 改解锁 flag → 回填，风险与工作量都最大。")
    A("")
    if s["quick_unlock_games"]:
        A("### 快捷解锁：目录内已自带全 CG 存档\n")
        for f in s["quick_unlock_games"]:
            A(f"- {f}")
        A("")

    A("## 二、解锁路线图（按引擎出现次数排序）\n")
    A("> 「自动」= STool 已有自动解锁能力；「可行」= 有明确自动化路径、值得实现；")
    A("> 「人工」= 需手工/脚本改；「未知」= 格式不明。\n")
    A("| 引擎 | 家族 | STool 插件 | 解锁自动化 | 数量 | 说明 |")
    A("| --- | --- | --- | --- | --- | --- |")
    for row in s["roadmap"]:
        A(f"| {row['engine']} | {row['family']} | {'✔' if row['stool_plugin'] else '—'} "
          f"| {SUPPORT_LABEL.get(row['unlock_support'], row['unlock_support'])} "
          f"| {row['game_count']} | {row['note']} |")
    A("")

    gaps = [r for r in s["roadmap"] if not r["auto_unlock"]]
    if gaps:
        A("### 待补齐的解锁能力（缺口）\n")
        for row in gaps:
            A(f"- **{row['engine']}**（{row['game_count']} 个，{SUPPORT_LABEL.get(row['unlock_support'])}）："
              f"{row['note'] or '—'}")
        A("")

    A("## 三、逐条明细\n")
    A("| 目录 | 识别引擎 | 变体 | 置信 | 类型 | 解锁优先级 | 伪装 |")
    A("| --- | --- | --- | --- | --- | --- | --- |")
    for r in records:
        A(f"| {r['folder']} | {r['engine_name'] or '—'} | {r['engine_variant'] or ''} "
          f"| {r['confidence']} | {r['kind']} "
          f"| {PRIORITY_LABEL.get(r.get('unlock_priority',''), r.get('unlock_priority',''))} "
          f"| {'、'.join(r['disguise']) or '—'} |")
    A("")

    # 需人工排查
    manual = [r for r in records if r["kind"] in ("unknown", "no_exe", "empty", "archive_only",
                                                  "game_unknown_engine", "error")
              or (r["kind"] == "game" and r["confidence"] in ("low", "none"))]
    if manual:
        A("## 四、需人工排查\n")
        for r in manual:
            why = "; ".join(r["notes"]) or f"kind={r['kind']}"
            hint = (r.get("structure") or {}).get("top_exts", [])
            extra = f"（主要类型：{', '.join(hint[:5])}）" if hint else ""
            A(f"- `{r['folder']}` → {why}{extra}")
        A("")

    # 伪装统计
    dis = Counter(t for r in records for t in r["disguise"])
    if dis:
        A("## 五、检测到的伪装类型\n")
        for k, v in dis.most_common():
            A(f"- {k}：{v} 个")
        A("")

    A("---\n")
    A("## 附：如何复现本报告\n")
    A("```")
    A("python D:\\STool\\scripts\\scan_games.py")
    A("# 输出：games_inventory.json / .csv / .md（本文件）")
    A("```")
    A("识别规则与 STool `src/engines/`（正式插件 + `recognize.rs` 识别器表）保持一致，")
    A("并额外补充了分卷包推断、文件头 magic 兜底、防和谐伪装识别与「全CG存档」探测；")
    A("引擎 `engine_id` 与 STool 的 plugin id 对齐，可直接据此扩展 `gallery` 解锁能力。")

    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(L))


SUPPORT_LABEL = {
    "native": "自动（已实现）",
    "feasible": "可行（值得做）",
    "manual": "人工",
    "unknown": "未知",
    "recognize": "仅识别",
}

PRIORITY_LABEL = {
    "quick": "★自带全CG存档",
    "auto": "自动解锁",
    "feasible": "可行",
    "manual": "人工",
    "unknown": "未知",
}


def fmt_counter(c: dict) -> str:
    return "、".join(f"{k}×{v}" for k, v in c.items()) or "—"


# ===========================================================================
# main
# ===========================================================================
def main():
    ap = argparse.ArgumentParser(description="游戏目录引擎盘点（STool 全CG解锁路线图输入）")
    ap.add_argument("--roots", default=r"D:\baidudownload;D:\ero",
                    help="扫描根目录，分号分隔")
    ap.add_argument("--out", default=r"D:\STool\verify\game_inventory", help="输出目录")
    ap.add_argument("--max-descent", type=int, default=3, help="壳目录下钻最大层数")
    ap.add_argument("--magic-budget", type=int, default=40,
                    help="文件名识别失败时，每个目录最多抽读多少个文件头")
    ap.add_argument("--limit", type=int, default=0, help="只扫描前 N 个一级目录（调试用）")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    roots = [r.strip() for r in args.roots.split(";") if r.strip()]
    catalog = unlock_catalog()

    folders: list[str] = []
    for root in roots:
        if not os.path.isdir(root):
            print(f"⚠ 跳过不存在的根目录：{root}", file=sys.stderr)
            continue
        for e in sorted(os.scandir(root), key=lambda x: x.name.lower()):
            if e.name.startswith("."):
                continue  # .accelerate 等隐藏缓存
            folders.append(e.path)
    if args.limit:
        folders = folders[: args.limit]

    print(f"发现 {len(folders)} 个一级条目，开始识别……", file=sys.stderr)

    records: list[dict] = []
    done = 0
    with futures.ThreadPoolExecutor(max_workers=min(8, (os.cpu_count() or 4))) as ex:
        futs = {ex.submit(analyze, f, args, catalog): f for f in folders}
        for fut in futures.as_completed(futs):
            f = futs[fut]
            try:
                rec = fut.result()
            except Exception as exc:  # 单目录失败不影响整体
                rec = {"folder": os.path.basename(f), "path": f, "kind": "error",
                       "clean_name": os.path.basename(f), "engine_id": "", "engine_name": "",
                       "engine_family": "", "engine_variant": "", "confidence": "none",
                       "score": 0, "disguise": [], "evidence": [], "runners_up": [],
                       "stool_support": "none", "cg_unlock": {}, "unlock_priority": "unknown",
                       "structure": {}, "game_root": f, "wrapped_in_subdir": False, "id": "",
                       "notes": [f"扫描异常：{exc}"],
                       "last_scan": datetime.now().astimezone().isoformat(timespec="seconds")}
            records.append(rec)
            done += 1
            if not args.quiet:
                if rec.get("kind") in GAME_KINDS:
                    tag = rec.get("engine_name") or "未识别引擎"
                else:
                    tag = rec.get("kind")
                print(f"[{done}/{len(folders)}] {rec['folder']} → {tag}", file=sys.stderr)

    records.sort(key=lambda r: (r["kind"] != "game", r.get("folder", "").lower()))

    summary = summarize(records, catalog)
    payload = {
        "schema_version": 1,
        "generated_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "roots": roots,
        "engine_catalog": catalog,
        "summary": summary,
        "games": records,
    }

    os.makedirs(args.out, exist_ok=True)
    jp = os.path.join(args.out, "games_inventory.json")
    cp = os.path.join(args.out, "games_inventory.csv")
    mp = os.path.join(args.out, "games_inventory.md")
    write_json(jp, payload)
    write_csv(cp, records)
    write_md(mp, payload, records, roots)

    print("\n===== 汇总 =====", file=sys.stderr)
    print(f"条目 {summary['total_entries']}（游戏 {summary['games']}）", file=sys.stderr)
    print(f"引擎分布：{fmt_counter(summary['by_engine'])}", file=sys.stderr)
    print(f"解锁自动化：{fmt_counter(summary['by_unlock_support'])}", file=sys.stderr)
    print(f"\n已写出：\n  {jp}\n  {cp}\n  {mp}", file=sys.stderr)


if __name__ == "__main__":
    main()
