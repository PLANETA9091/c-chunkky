#!/usr/bin/env python3
"""Cluster gold-ore blocks in both dumps; print per-chunk vein centers/sizes."""
import struct, sys
from pathlib import Path
from collections import defaultdict

MAGIC = 0x46534243
HEIGHT = 384
MINY = -64

def read_dump(path):
    data = path.read_bytes()
    (magic, version, cx, cz, miny, height) = struct.unpack_from("<Iiiiii", data, 0)
    off = 24
    (pal_len,) = struct.unpack_from("<I", data, off)
    off += 4
    palette = []
    for _ in range(pal_len):
        (slen,) = struct.unpack_from("<H", data, off)
        off += 2
        palette.append(data[off:off + slen].decode())
        off += slen
    n = 16 * 16 * height
    idx = struct.unpack_from(f"<{n}H", data, off)
    return cx, cz, palette, idx

GOLD = {"minecraft:gold_ore", "minecraft:deepslate_gold_ore"}
COPPER = {"minecraft:copper_ore", "minecraft:deepslate_copper_ore"}
IRON = {"minecraft:iron_ore", "minecraft:deepslate_iron_ore"}

def ore_blocks(d, names):
    blocks = set()
    for f in sorted(Path(d).glob("*.crsf")):
        cx, cz, pal, idx = read_dump(f)
        for i, v in enumerate(idx):
            if pal[v] in names:
                y, rem = divmod(i, 256)
                z, x = divmod(rem, 16)
                blocks.add((x + cx * 16, y + MINY, z + cz * 16))
    return blocks

def clusters(blocks):
    out = []
    seen = set()
    for b in blocks:
        if b in seen:
            continue
        stack, cl = [b], set()
        seen.add(b)
        while stack:
            p = stack.pop()
            cl.add(p)
            x, y, z = p
            for dx in (-1, 0, 1):
                for dy in (-1, 0, 1):
                    for dz in (-1, 0, 1):
                        q = (x + dx, y + dy, z + dz)
                        if q in blocks and q not in seen:
                            seen.add(q)
                            stack.append(q)
        out.append(sorted(cl))
    return out

a, b = sys.argv[1], sys.argv[2]
for label, names in (("GOLD", GOLD), ("COPPER", COPPER), ("IRON", IRON)):
    ba, bb = ore_blocks(a, names), ore_blocks(b, names)
    ca, cb = clusters(ba), clusters(bb)
    print(f"== {label}: A={len(ba)} blocks in {len(ca)} clusters; B={len(bb)} in {len(cb)}")
    for tag, cl in (("A", ca), ("B", cb)):
        for c in sorted(cl, key=lambda c: (min(x for x, y, z in c), min(z for x, y, z in c), min(y for x, y, z in c))):
            xs = [x for x, y, z in c]; ys = [y for x, y, z in c]; zs = [z for x, y, z in c]
            print(f"  {tag}: n={len(c)} xrange={min(xs)}..{max(xs)} y={min(ys)}..{max(ys)} z={min(zs)}..{max(zs)}")
