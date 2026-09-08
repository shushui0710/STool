"""补丁 / MOD 管理器：基于文件覆盖 + 清单备份，可安装/卸载/启停。

每个已安装 MOD 在游戏目录的 stool_mods/<mod_name>/ 存放文件副本，
stool_mods/registry.json 记录清单。应用 = 覆盖到游戏目录；卸载 = 还原备份。
"""
from __future__ import annotations

import json
import shutil
from pathlib import Path

from ..core.util import copy_tree, null_progress

REGISTRY = "stool_mods/registry.json"


def _registry_path(game_root: Path) -> Path:
    return game_root / REGISTRY


def list_mods(game_root: Path) -> list:
    rp = _registry_path(game_root)
    if not rp.exists():
        return []
    try:
        return json.loads(rp.read_text("utf-8"))
    except Exception:
        return []


def _save_registry(game_root: Path, mods: list) -> None:
    rp = _registry_path(game_root)
    rp.parent.mkdir(parents=True, exist_ok=True)
    rp.write_text(json.dumps(mods, ensure_ascii=False, indent=2), "utf-8")


def install_mod(game_root: Path, mod_dir: Path, name: str = "", backup: bool = True,
                progress=null_progress) -> dict:
    """把 mod_dir 的文件作为覆盖补丁安装。原文件备份到 stool_mods/_backups/<name>/。"""
    game_root = Path(game_root)
    mod_dir = Path(mod_dir)
    name = name or mod_dir.name
    mods = list_mods(game_root)
    if any(m["name"] == name for m in mods):
        raise ValueError(f"MOD '{name}' 已存在，请先卸载或改名")
    files = [p for p in mod_dir.rglob("*") if p.is_file()]
    if not files:
        raise ValueError("补丁目录为空")
    overwritten = []
    for p in files:
        rel = p.relative_to(mod_dir)
        target = game_root / rel
        if target.exists():
            overwritten.append(str(rel))
    # 备份将被覆盖的文件
    if backup and overwritten:
        bak = game_root / "stool_mods" / "_backups" / name
        for rel in overwritten:
            src, dst = game_root / rel, bak / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)
    # 复制补丁文件
    copy_tree(mod_dir, game_root, progress)
    # 留存补丁副本供启停切换
    store = game_root / "stool_mods" / name
    copy_tree(mod_dir, store, progress)
    entry = {"name": name, "files": [str(p.relative_to(mod_dir)).replace("\\", "/") for p in files],
             "overwritten": overwritten, "enabled": True}
    mods.append(entry)
    _save_registry(game_root, mods)
    return entry


def uninstall_mod(game_root: Path, name: str) -> None:
    """卸载 MOD：还原被覆盖的原始文件并删除新增文件。"""
    game_root = Path(game_root)
    mods = list_mods(game_root)
    entry = next((m for m in mods if m["name"] == name), None)
    if not entry:
        raise ValueError(f"MOD '{name}' 不存在")
    bak = game_root / "stool_mods" / "_backups" / name
    for rel in entry["files"]:
        target = game_root / rel
        orig = bak / rel
        if orig.exists():
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(orig, target)
        else:
            if target.exists():
                target.unlink()
    shutil.rmtree(bak, ignore_errors=True)
    shutil.rmtree(game_root / "stool_mods" / name, ignore_errors=True)
    mods.remove(entry)
    _save_registry(game_root, mods)


def toggle_mod(game_root: Path, name: str, enabled: bool) -> None:
    """启停 MOD：停用 = 用备份还原；启用 = 再次覆盖。"""
    game_root = Path(game_root)
    mods = list_mods(game_root)
    entry = next((m for m in mods if m["name"] == name), None)
    if not entry:
        raise ValueError(f"MOD '{name}' 不存在")
    bak = game_root / "stool_mods" / "_backups" / name
    if enabled:
        for rel in entry["files"]:
            src = game_root / "stool_mods" / name / rel
            if not src.exists():
                continue
            dst = game_root / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)
    else:
        for rel in entry["files"]:
            orig = bak / rel
            target = game_root / rel
            if orig.exists():
                shutil.copy2(orig, target)
            elif target.exists():
                target.unlink()
    entry["enabled"] = enabled
    _save_registry(game_root, mods)
