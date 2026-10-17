#!/usr/bin/env python3
"""diff_hist.py <dirA_ref> <dirB_ours> — count (ref_state, ours_state) pairs
for differing blocks + per-column delta histogram. Top-30 pairs."""
import struct, sys
from pathlib import Path
from collections import Counter

MAGIC = 0x46534243
H = 384
MINY = -64

def read_dump(path):
    data = path.read_bytes()
    (magic, ver, cx, cz, miny, height) = struct.unpack_from("<Iiiiii", data, 0)
    off = 24
    (pl,) = struct.unpack_from("<I", data, off); off += 4
    pal = []
    for _ in range(pl):
        (n,) = struct.unpack_from("<H", data, off); off += 2
        pal.append(data[off:off+n].decode()); off += n
    blocks = struct.unpack_from(f"<{H*256}H", data, off)
    return cx, cz, pal, blocks

a_dir, b_dir = Path(sys.argv[1]), Path(sys.argv[2])
pairs = Counter()
cols_moved = Counter()
tot = 0
for fa in sorted(a_dir.glob("*.crsf")):
    fb = b_dir / fa.name
    if not fb.exists():
        continue
    cx, cz, pa, A = read_dump(fa)
    _, _, pb, B = read_dump(fb)
    for idx, (va, vb) in enumerate(zip(A, B)):
        sa, sb = pa[va], pb[vb]
        if sa != sb:
            tot += 1
            pairs[(sa, sb)] += 1
            if sa != "minecraft:air" and sb == "minecraft:air":
                cols_moved["ours_removed"] += 1
            elif sa == "minecraft:air" and sb != "minecraft:air":
                cols_moved["ours_added"] += 1
print(f"total diff blocks: {tot}")
print(f"air-boundary: {dict(cols_moved)}")
print("top state pairs (ref -> ours):")
for (sa, sb), n in pairs.most_common(30):
    print(f"  {n:7d}  {sa}  ->  {sb}")
