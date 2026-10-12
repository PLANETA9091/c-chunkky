#!/usr/bin/env python3
"""Bubble column steady-state check: for each magma with water above,
compare column height vs available water column height (both dumps)."""
import struct
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

def blk(p, i, x, y, z):
    return p[i[(y+64)*256 + z*16 + x]].replace("minecraft:", "").split("[")[0]

for tag, base in [("REF", "reference/dumps/vanilla/"), ("OURS", "/tmp/judge2/reference/dumps_plugin/vanilla/")]:
    files = sorted(Path(base).glob("chunk_*.crsf"))
    total_mag = 0
    cols = []
    for f in files:
        pal, idx = read_dump(f)
        for x in range(16):
            for z in range(16):
                for y in range(-64, 320):
                    if blk(pal, idx, x, y, z) != "magma_block":
                        continue
                    above = blk(pal, idx, x, y+1, z) if y < 319 else "air"
                    if above not in ("water", "bubble_column"):
                        continue
                    total_mag += 1
                    # column height: consecutive bubble_column from y+1
                    h = 0
                    yy = y + 1
                    while yy < 320 and blk(pal, idx, x, yy, z) == "bubble_column":
                        h += 1
                        yy += 1
                    # available water above (until non-water)
                    avail = 0
                    yy = y + 1
                    while yy < 320 and blk(pal, idx, x, yy, z) in ("water", "bubble_column"):
                        avail += 1
                        yy += 1
                    cols.append((f.name, x, y, z, h, avail))
    print(f"{tag}: magma-with-fluid-above sites = {total_mag}")
    complete = sum(1 for c in cols if c[4] == c[5])
    print(f"  complete columns (h == available): {complete}/{len(cols)}")
    for c in cols[:12]:
        print(f"  {c[0]} ({c[1]},{c[2]},{c[3]}) col_h={c[4]} avail={c[5]}" + ("  <-- INCOMPLETE" if c[4] != c[5] else ""))
