#!/usr/bin/env python3
"""Map water/lava in neighborhood: ref vs ours (CI artifact)."""
import struct, sys
from pathlib import Path

def read_dump(path):
    data = Path(path).read_bytes()
    off = 24
    (pal_len,) = struct.unpack_from("<I", data, off); off += 4
    palette = []
    for _ in range(pal_len):
        (slen,) = struct.unpack_from("<H", data, off); off += 2
        palette.append(data[off:off+slen].decode()); off += slen
    idx = struct.unpack_from(f"<{16*16*384}H", data, off)
    return palette, idx

base_ref = "/home/z/my-project/c-faster/reference/dumps/vanilla/"
base_our = "/tmp/judge_art/reference/dumps_plugin/vanilla/"
pa, ia = read_dump(base_ref + "chunk_-1_0.crsf")
pb, ib = read_dump(base_our + "chunk_-1_0.crsf")

def blk(p, i, x, y, z):
    return p[i[(y+64)*256 + z*16 + x]].replace("minecraft:", "").split("[")[0]

C = {"water": "w", "lava": "L", "air": ".", "stone": "s", "deepslate": "d",
     "obsidian": "O", "bubble_column": "B", "magma_block": "M", "gravel": "g",
     "tuff": "t", "dirt": "D", "clay": "c"}

for y in range(-51, -59, -1):
    print(f"y={y}  REF        OURS")
    for z in range(0, 11):
        r = "".join(C.get(blk(pa, ia, x, y, z), "?") for x in range(2, 13))
        o = "".join(C.get(blk(pb, ib, x, y, z), "?") for x in range(2, 13))
        mark = "  <<<" if r != o else ""
        print(f"  z={z:2d} {r}   {o}{mark}")
