"""核心格式解析工具：若干小型二进制/编码算法，供多个引擎插件复用。"""
from __future__ import annotations

import struct
import zlib


# ---------- RGSSAD (RPG Maker XP/VX/Ace) ----------

def rgss_key_stream(base: int = 0xDEADCAFE):
    """key = key*7+3 (mod 2^32) 无限流。"""
    k = base & 0xFFFFFFFF
    while True:
        yield k
        k = (k * 7 + 3) & 0xFFFFFFFF


def rgss_read_u32(data: bytes, pos: int, keygen) -> int:
    val = struct.unpack_from("<I", data, pos)[0] ^ next(keygen)
    return val, pos + 4


def rgss_advance_per_byte(keygen, n: int):
    for _ in range(n):
        next(keygen)


def rgss1_decrypt(data: bytes) -> dict:
    """XP/VX 的 .rgssad/.rgss2a：返回 {name: (offset, size, filekey)}。"""
    assert data[:7] == b"RGSSAD\x00", "不是 RGSSAD 封包"
    pos = 8  # 7 魔数 + 1 版本
    keygen = rgss_key_stream()
    entries = {}
    while pos + 4 <= len(data):
        name_len, pos = rgss_read_u32(data, pos, keygen)
        if name_len <= 0 or name_len > 512 or pos + name_len + 4 > len(data):
            break
        raw_name = data[pos:pos + name_len]
        name = bytearray()
        for b in raw_name:
            name.append(b ^ (next(keygen) & 0xFF))
        pos += name_len
        size, pos = rgss_read_u32(data, pos, keygen)
        entries[name.decode("utf-8", "replace")] = (pos, size, None)
        # 数据区：key 每推进 4 字节前进一次
        kpos = pos
        blocks = (size + 3) // 4
        for _ in range(blocks):
            next(keygen)
        pos += size
    return entries


def rgss1_extract_file(data: bytes, offset: int, size: int) -> bytes:
    keygen = rgss_key_stream()
    out = bytearray(size)
    for i in range(size):
        if i % 4 == 0:
            k = next(keygen)
        out[i] = data[offset + i] ^ (k & 0xFF)
    return bytes(out)


def rgss3_parse(data: bytes) -> list:
    """VX Ace 的 .rgss3a：返回 [(name, offset, size, filekey)]。"""
    assert data[:8] == b"RGSSAD\x00\x01", "不是 RGSS3A 封包"
    pos = 8
    keygen = rgss_key_stream()
    entries = []
    while pos + 16 <= len(data):
        # offset(8) 与 size(8) 交错加密，各 4 字节一组
        vals = []
        for _ in range(4):  # offset_lo, offset_hi, size_lo, size_hi
            v = struct.unpack_from("<I", data, pos)[0] ^ next(keygen)
            pos += 4
            vals.append(v)
        offset = vals[0] | (vals[1] << 32)
        size = vals[2] | (vals[3] << 32)
        if offset == 0 and size == 0:
            break
        filekey = (size * 9 + 3) & 0xFFFFFFFF
        fk = filekey
        name_len = struct.unpack_from("<I", data, pos)[0] ^ fk
        pos += 4
        if name_len <= 0 or name_len > 512 or pos + name_len > len(data):
            break
        name = bytearray()
        for b in data[pos:pos + name_len]:
            fk = (fk * 7 + 3) & 0xFFFFFFFF
            name.append(b ^ (fk & 0xFF))
        pos += name_len
        entries.append((name.decode("utf-8", "replace"), offset, size, filekey))
    return entries


def rgss3_extract_file(data: bytes, offset: int, size: int, filekey: int) -> bytes:
    fk = filekey
    out = bytearray(size)
    for i in range(size):
        if i % 4 == 0:
            fk = (fk * 7 + 3) & 0xFFFFFFFF
        if offset + i < len(data):
            out[i] = data[offset + i] ^ (fk & 0xFF)
    return bytes(out)


# ---------- XP3 (KiriKiri) ----------

def xp3_parse(data: bytes) -> dict:
    """解析（未加密的）XP3 封包。返回 {name: {"segments": [(off,size,asize,csize)], "protected": bool}}"""
    if data[:11] != b"XP3\r\n \n\x1a\x8b\x67\x01" and data[:3] != b"XP3":
        raise ValueError("不是 XP3 封包")
    pos = 11
    flag = data[pos]
    pos += 1
    if flag == 0x80:
        index_pos = struct.unpack_from("<Q", data, pos)[0]
        pos += 8
    else:
        index_pos = struct.unpack_from("<I", data, pos)[0]
        pos += 4
    raw = data[index_pos:]
    comp = raw[0] == 0x01 or raw[0] == 0x80
    body = raw[1:]
    toc = zlib.decompress(body) if comp else body
    count = struct.unpack_from("<I", toc, 0)[0]
    p = 4
    files = {}
    for _ in range(count):
        chunk_size = struct.unpack_from("<I", toc, p)[0]
        p += 4
        chunk = toc[p:p + chunk_size]
        p += chunk_size
        q = 0
        name, segments, protected = "", [], 0
        while q + 4 <= len(chunk):
            tag = chunk[q:q + 4]
            size = struct.unpack_from("<I", chunk, q + 4)[0]
            body_c = chunk[q + 8:q + 8 + size]
            if tag == b"info":
                protected = struct.unpack_from("<I", body_c, 0)[0]
                osize, csize = struct.unpack_from("<QQ", body_c, 4)
                nlen = struct.unpack_from("<H", body_c, 20)[0]
                name = body_c[22:22 + nlen * 2].decode("utf-16-le", "replace")
            elif tag == b"segm":
                n = struct.unpack_from("<I", body_c, 0)[0]
                for i in range(n):
                    off = 4 + i * 32
                    start, aoff, osize, asize = struct.unpack_from("<QQQQ", body_c, off)
                    segments.append((aoff, osize, asize))
            elif tag == b"adlr":
                pass  # adler32 校验（加密变体会不匹配）
            q += 8 + size
        files[name] = {"segments": segments, "protected": protected}
    return files


def xp3_read_file(data: bytes, info: dict) -> bytes:
    out = bytearray()
    for aoff, osize, asize in info["segments"]:
        blob = data[aoff:aoff + asize]
        if asize == 0:
            continue
        if asize < osize:  # 压缩段
            out += zlib.decompress(blob)
        else:
            out += blob
    return bytes(out)


# ---------- asar (Electron) ----------

def asar_parse(data: bytes) -> tuple:
    """返回 (files_dict, header_size, data_start)。files: {path: {"offset": int, "size": int}}"""
    if struct.unpack_from("<I", data, 0)[0] != 4:
        raise ValueError("不是 asar 文件")
    json_size = struct.unpack_from("<I", data, 4)[0]
    header = data[16:16 + json_size].decode("utf-8", "replace")
    meta = __import__("json").loads(header)
    files = {}

    def walk(node: dict, prefix: str):
        for name, child in node.get("files", {}).items():
            rel = f"{prefix}/{name}" if prefix else name
            if "files" in child:
                walk(child, rel)
            elif "offset" in child:
                files[rel] = {"offset": int(child["offset"]), "size": int(child["size"]),
                              "unpacked": child.get("unpacked", False)}
    walk(meta, "")
    return files, 8 + json_size, 16 + json_size


def asar_read_file(data: bytes, data_start: int, entry: dict) -> bytes:
    if entry.get("unpacked"):
        raise ValueError("unpacked 文件存储在 asar.unpacked 目录")
    off = data_start + entry["offset"]
    return data[off:off + entry["size"]]


# ---------- RPG Maker MV/MZ 加密素材 ----------

MV_FAKE = bytes([0x52, 0x50, 0x47, 0x4D, 0x56, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
MZ_FAKE = bytes([0x52, 0x50, 0x47, 0x4D, 0x5A, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])


def rpgmmv_decrypt(data: bytes) -> bytes:
    """MV/MZ 加密素材解密（16 字节假头 + 16 字节 XOR 真头）。"""
    if len(data) < 32:
        return data
    fake = data[:16]
    if fake[:4] != b"RPGM":
        return data
    rest = data[16:32]
    return bytes(a ^ b for a, b in zip(rest, fake)) + data[32:]


def rpgmmv_encrypt(data: bytes, mz: bool = False) -> bytes:
    fake = MZ_FAKE if mz else MV_FAKE
    head = bytes(a ^ b for a, b in zip(data[:16], fake))
    return fake + head + data[16:]


# ---------- NScripter nscript.dat ----------

def nscript_decode(data: bytes) -> bytes:
    """ONScripter 加密脚本：前两字节为大端密钥，第 i 字节 XOR (key+i)&0xFF。
    自动尝试并选择可打印率最高的变体。"""
    if len(data) < 4:
        return data
    key = (data[0] << 8) | data[1]

    def printable_ratio(b: bytes) -> float:
        if not b:
            return 0.0
        ok = sum(1 for c in b[:4096] if 0x20 <= c < 0x7f or c in (9, 10, 13) or c >= 0x80)
        return ok / min(len(b), 4096)

    variants = [
        bytes((c ^ ((key + i) & 0xFF)) for i, c in enumerate(data)),
        bytes((c ^ (key & 0xFF)) for c in data),
        data,
    ]
    best = max(variants, key=printable_ratio)
    return best


# ---------- Godot PCK ----------

def pck_parse(data: bytes) -> list:
    """解析 Godot .pck。返回 [(path, offset, size)]，offset 为绝对偏移。"""
    if data[:4] != b"GDPC":
        raise ValueError("不是 PCK 封包")
    fmt_ver = struct.unpack_from("<I", data, 4)[0]
    ver_major, ver_minor, ver_patch = struct.unpack_from("<III", data, 8)
    pos = 20
    if fmt_ver == 2:
        pack_flags, file_base = struct.unpack_from("<IQ", data, pos)
        pos += 12
    else:
        pos += 16 * 4  # 16 个 u32 保留字段
        file_base = 0
    count = struct.unpack_from("<I", data, pos)[0]
    pos += 4
    entries = []
    for _ in range(count):
        plen = struct.unpack_from("<I", data, pos)[0]
        pos += 4
        path = data[pos:pos + plen].rstrip(b"\x00").decode("utf-8", "replace")
        pos += plen
        offset, size = struct.unpack_from("<QQ", data, pos)
        pos += 16 + 16  # offset, size, md5
        if fmt_ver == 2:
            pos += 8  # flags + delta
        entries.append((path, file_base + offset, size))
    return entries


# ---------- Ruby Marshal 最小解析（Scripts.rxdata 提取用） ----------

def rgss_scripts_extract(data: bytes) -> list:
    """从 Data/Scripts.rxdata|rvdata|rvdata2 提取 zlib 压缩的 Ruby 源码列表。
    数据结构为 Marshal 的 Array[[magic_id, name, zlib_code], ...]，此处做容错解析。"""
    import io
    from rubymarshal.reader import load
    obj = load(io.BytesIO(data))
    items = obj.get("d") if isinstance(obj, dict) else obj
    scripts = []
    for i, item in enumerate(items or []):
        if isinstance(item, dict) and "d" in item:
            item = item["d"]
        if isinstance(item, (list, tuple)) and len(item) >= 3:
            code = item[2]
        elif isinstance(item, (bytes, bytearray, str)):
            code = item
        else:
            continue
        if isinstance(code, str):
            code = code.encode("latin1", "replace")
        try:
            raw = zlib.decompress(bytes(code))
        except Exception:
            raw = bytes(code)
        scripts.append(raw)
    return scripts
