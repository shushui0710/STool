"""存档服务：统一入口定位/解码/编码各引擎存档，供 CLI 与 GUI 调用。"""
from __future__ import annotations

import io
import json
import os
import pickle
from pathlib import Path

from ..core.plugin import PluginRegistry


def find_save_locations(game_root: Path, registry: PluginRegistry) -> list:
    """返回 [(标签, 目录)]，覆盖游戏目录内与系统 AppData 位置。"""
    out = []
    appdata = Path(os.environ.get("APPDATA", ""))
    local_low = Path(os.environ.get("USERPROFILE", "")) / "AppData" / "LocalLow"
    # 目录内
    for rel in ("save", "www/save", "Saves", "Save"):
        p = game_root / rel
        if p.is_dir():
            out.append((f"游戏目录/{rel}", p))
    # Ren'Py
    rp = appdata / "RenPy"
    if rp.is_dir():
        out += [(f"RenPy/{d.name}", d) for d in rp.iterdir() if d.is_dir()]
    # Unity LocalLow
    if local_low.is_dir():
        out += [(f"LocalLow/{d.name}", d) for d in local_low.iterdir() if d.is_dir()]
    # 注册表提示
    out.append(("Windows 注册表 HKCU\\Software\\<厂商>\\<游戏名>（用 regedit 手动查看）", Path(".")))
    return out


SAVE_DECODE_HINTS = {
    ".rpgsave": "RPG Maker MV (lz-string)",
    ".rmmzsave": "RPG Maker MV",
    ".rmzsave": "RPG Maker MZ (zlib)",
    ".rxdata": "RPG Maker XP (Ruby Marshal)",
    ".rvdata": "RPG Maker VX (Ruby Marshal)",
    ".rvdata2": "RPG Maker VX Ace (Ruby Marshal)",
    "persistent": "Ren'Py (pickle)",
    ".sol": "Flash SharedObject",
    ".sav": "通用",
    ".json": "JSON",
}


def decode_save(path: Path, plugin=None):
    """尽力解码存档为 JSON 可读文本。返回 (json_text 或 None, 说明)。"""
    ext = path.suffix.lower()
    name = path.name
    if ext == ".json":
        try:
            return json.dumps(json.loads(path.read_text("utf-8")), ensure_ascii=False, indent=1), "JSON"
        except Exception as e:
            return None, f"JSON 解析失败: {e}"
    if name == "persistent" or ext == ".rupersist":
        from ..engines.renpy import persistent_load
        try:
            obj = persistent_load(path)
            return json.dumps(_safe_serialize(obj), ensure_ascii=False, indent=1, default=str), "Ren'Py persistent"
        except Exception as e:
            return None, f"persistent 解析失败: {e}"
    if ext in (".rpgsave", ".rmmzsave", ".rmzsave"):
        try:
            from ..engines.rpgmaker_mv import mv_save_decode, mz_save_decode
            raw = path.read_bytes()
            text = mz_save_decode(raw) if ext == ".rmzsave" else mv_save_decode(raw)
            return json.dumps(json.loads(text), ensure_ascii=False, indent=1), SAVE_DECODE_HINTS[ext]
        except Exception as e:
            return None, f"解码失败: {e}"
    if ext in (".rxdata", ".rvdata", ".rvdata2"):
        return None, "Ruby Marshal 存档：建议先改 data/ JSON 或用内存修改（Cheat Engine）"
    return None, f"未知格式 {ext or name}（{SAVE_DECODE_HINTS.get(name, '无匹配提示')}）"


def encode_save(path: Path, json_text: str, plugin=None):
    """把编辑后的 JSON 回写存档（自动备份原文件）。"""
    from ..core.util import backup_file
    ext = path.suffix.lower()
    backup_file(path)
    data = json.loads(json_text)
    if ext == ".json":
        path.write_text(json.dumps(data, ensure_ascii=False, indent=1), "utf-8")
        return "JSON 已写回"
    if ext in (".rpgsave", ".rmmzsave"):
        raise ValueError("MV 存档回写依赖精确 lz-string 压缩，建议只读编辑或改 data/ JSON")
    if ext == ".rmzsave":
        import zlib
        path.write_bytes(zlib.compress(json.dumps(data, ensure_ascii=False).encode("utf-8")))
        return "MZ 存档已写回（原文件备份为 .stool.bak）"
    raise ValueError(f"不支持的回写格式: {ext}")


def _safe_serialize(obj, depth=0):
    """把 pickle 出来的对象尽力转 JSON。"""
    if depth > 8:
        return "..."
    if isinstance(obj, dict):
        return {str(k): _safe_serialize(v, depth + 1) for k, v in list(obj.items())[:500]}
    if isinstance(obj, (list, tuple, set)):
        return [_safe_serialize(v, depth + 1) for v in list(obj)[:500]]
    if isinstance(obj, (int, float, bool, str)) or obj is None:
        return obj
    if isinstance(obj, bytes):
        return obj.decode("utf-8", "replace")[:200]
    return str(obj)
