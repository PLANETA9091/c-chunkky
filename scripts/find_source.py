#!/usr/bin/env python3
"""Find water[level=0] source blocks near a flowing-water pocket in ref."""
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

name = sys.argv[1] if len(sys.argv) > 1 else "chunk_-1_0.crsf"
base = "/home/z/my-project/c-faster/reference/dumps/vanilla/"
pa, ia = read_dump(base + name)

def blk(pal, idx, x, y, z):
    return pal[idx[(y+64)*256 + z*16 + x]]

# pocket: local (5..9, -56..-52, 2..9); find level=0 water nearby
pocket = [(x, y, z) for x in range(4, 10) for y in range(-57, -51) for z in range(1, 10)]
sources = []
for (x, y, z) in pocket:
    b = blk(pa, ia, x, y, z)
    if b.startswith("minecraft:water[level=0]"):
        sources.append((x, y, z))
print(f"{name}: water[level=0] sources near pocket:")
for s in sources:
    print("  local", s, "world", (s[0]-16 if name.startswith('chunk_-1_0') else s[0]), s[1], s[2])
# print the full 3D neighborhood block types for context
print("\nNeighborhood map (local coords), y layers -58..-51, x 2..12, z 0..10:")
for y in range(-51, -59, -1):
    print(f" y={y}")
    for z in range(0, 11):
        row = []
        for x in range(2, 13):
            b = blk(pa, ia, x, y, z).replace("minecraft:", "").split("[")[0]
            c = {"water": "w", "lava": "L", "air": ".", "stone": "s", "deepslate": "d",
                 "obsidian": "O", "bubble_column": "B", "magma_block": "M"}.get(b, "?")
            row.append(c)
        print("  " + "".join(row))
