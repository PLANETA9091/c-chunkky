#!/usr/bin/env python3
"""Inspect reference dumps: top water block per chunk (ocean surface level)."""
import struct, sys
from pathlib import Path

MAGIC = 0x46534243

def read_dump(path):
    data = path.read_bytes()
    magic, version, cx, cz, miny, height = struct.unpack_from("<Iiiiii", data, 0)
    assert magic == MAGIC
    off = 24
    (pal_len,) = struct.unpack_from("<I", data, off); off += 4
    palette = []
    for _ in range(pal_len):
        (slen,) = struct.unpack_from("<H", data, off); off += 2
        palette.append(data[off:off+slen].decode()); off += slen
    n = 16*16*height
    idx = struct.unpack_from(f"<{n}H", data, off)
    return palette, idx

def water_top(pal, idx, x, z):
    for y in range(383, -1, -1):
        i = y*256 + z*16 + x
        if pal[idx[i]] == "minecraft:water":
            return y - 64
    return None

def top_nonair(pal, idx, x, z):
    for y in range(383, -1, -1):
        i = y*256 + z*16 + x
        if pal[idx[i]] != "minecraft:air":
            return y - 64, pal[idx[i]]
    return -65, "air"

d = Path(sys.argv[1])
files = sorted(d.glob("*.crsf"))
print(f"{d}: {len(files)} files")
for f in files[:6]:
    pal, idx = read_dump(f)
    ws = [water_top(pal, idx, x, z) for x in range(16) for z in range(16)]
    maxw = max((w for w in ws if w is not None), default=None)
    wcols = sum(1 for w in ws if w is not None)
    # sample one land column top
    ty, tb = top_nonair(pal, idx, 8, 8)
    print(f"  {f.name}: max water top = {maxw}, water cols = {wcols}/256, center top = {ty}:{tb}")
