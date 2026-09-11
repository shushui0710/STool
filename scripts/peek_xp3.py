#!/usr/bin/env python3
"""一次性：按 GARbro/KiriKiri Z 规范解析 XP3，校验我们对格式的理解。

会做三件事：
1. 解析头部（v1: magic+u64；v2: magic + 0x17 扩展头 + u64@32）；
2. 解析 TOC（[u8 压缩标记][u64 压缩长][u64 原始长][zlib] + File/info/segm/adlr 条目流）；
3. 取前 N 个条目，读出数据段（按需 zlib 解压），比对 adlr 是「压缩前」还是「压缩后」的 Adler32。

用法: python peek_xp3.py <file.xp3> [n]
"""
import sys
import struct
import zlib

MAGIC = b"XP3\r\n \n\x1a\x8b\x67\x01"


def adler32(b):
    return zlib.adler32(b) & 0xFFFFFFFF


def parse_header(data):
    if not data.startswith(MAGIC):
        raise SystemExit("不是 XP3")
    if len(data) < 13:
        raise SystemExit("头部过短")
    if data[11] == 0x17 and len(data) >= 40:
        index_pos = struct.unpack_from("<Q", data, 32)[0]
        return index_pos, "v2(0x17/KiriKiri Z)"
    index_pos = struct.unpack_from("<Q", data, 11)[0]
    return index_pos, "v1(裸 u64@11)"


def read_toc(data, index_pos):
    raw = data[index_pos:]
    marker = raw[0]
    if marker == 0x01:
        packed = struct.unpack_from("<Q", raw, 1)[0]
        unpacked = struct.unpack_from("<Q", raw, 9)[0]
        toc = zlib.decompress(raw[17:17 + packed])
        assert len(toc) == unpacked, f"TOC 长度不符: {len(toc)} != {unpacked}"
    else:
        size = struct.unpack_from("<Q", raw, 1)[0]
        toc = raw[9:9 + size]
    if toc.startswith(b"Hxv4"):
        raise SystemExit("Hxv4 保护变体（HxCrypt）：TOC 与数据段均加密，本脚本不解析")
    return toc


def walk(toc):
    entries = []
    p = 0
    while p + 12 <= len(toc):
        tag = toc[p:p + 4]
        if tag != b"File":
            break
        content = struct.unpack_from("<Q", toc, p + 4)[0]
        p += 12
        end = p + content
        q = p
        info = segm = adlr = None
        while q + 12 <= end:
            t = toc[q:q + 4]
            ln = struct.unpack_from("<Q", toc, q + 4)[0]
            body = toc[q + 12:q + 12 + ln]
            if t == b"info":
                info = body
            elif t == b"segm":
                segm = body
            elif t == b"adlr":
                adlr = body
            q += 12 + ln
        p = end
        entries.append((info, segm, adlr))
    return entries


def main():
    path = sys.argv[1]
    n = int(sys.argv[2]) if len(sys.argv) > 2 else 5
    with open(path, "rb") as fh:
        data = fh.read()
    index_pos, ver = parse_header(data)
    toc = read_toc(data, index_pos)
    entries = walk(toc)
    print(f"文件: {path}")
    print(f"  大小={len(data)}  版本={ver}  index@{index_pos}  toc_len={len(toc)}  条目数={len(entries)}")
    flag_hist = {}
    ok_before = ok_after = 0
    for info, segm, adlr in entries:
        flags = struct.unpack_from("<I", info, 0)[0]
        flag_hist[flags] = flag_hist.get(flags, 0) + 1
    print(f"  info flags 分布: {flag_hist}")
    for i, (info, segm, adlr) in enumerate(entries[:n]):
        flags = struct.unpack_from("<I", info, 0)[0]
        unpacked = struct.unpack_from("<Q", info, 4)[0]
        packed_info = struct.unpack_from("<Q", info, 12)[0]
        nlen = struct.unpack_from("<H", info, 20)[0]
        name = info[22:22 + nlen * 2].decode("utf-16-le", "replace")
        seg_count = struct.unpack_from("<I", segm, 0)[0]
        cid, off, size, psize = struct.unpack_from("<IQQQ", segm, 0)
        blob = data[off:off + psize]
        if cid == 1 and psize != size:
            content = zlib.decompress(blob)
        else:
            content = blob
        stored = struct.unpack_from("<I", adlr, 0)[0]
        a_before = adler32(content)
        a_after = adler32(blob)
        tag_b = "✔压缩前" if a_before == stored else "✘"
        tag_a = "✔压缩后" if a_after == stored else "✘"
        print(f"  [{i}] flags=0x{flags:08x} cid={cid} 名={name}")
        print(f"      段数={seg_count} unpacked={unpacked}(实测{len(content)}) packed={psize}(info packed={packed_info})")
        print(f"      adlr stored=0x{stored:08x}  压缩前adler=0x{a_before:08x}{tag_b}  压缩后adler=0x{a_after:08x}{tag_a}")
    # 全量统计 adlr 含义
    for info, segm, adlr in entries:
        if not (info and segm and adlr and len(segm) >= 28):
            continue
        cid, off, size, psize = struct.unpack_from("<IQQQ", segm, 0)
        blob = data[off:off + psize]
        if cid == 1 and psize != size:
            try:
                content = zlib.decompress(blob)
            except zlib.error:
                continue
        else:
            content = blob
        stored = struct.unpack_from("<I", adlr, 0)[0]
        if adler32(content) == stored:
            ok_before += 1
        if adler32(blob) == stored:
            ok_after += 1
    print(f"  全量：adlr==压缩前adler {ok_before}/{len(entries)}；adlr==压缩后adler {ok_after}/{len(entries)}")


if __name__ == "__main__":
    main()
