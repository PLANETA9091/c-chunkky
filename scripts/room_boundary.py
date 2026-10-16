#!/usr/bin/env python3
"""Analyze the unfilled-room region in chunk (3,-2): ref vs ours.

Prints: room extent (ref=water ours=air), boundary water ring in both worlds,
and (optionally) marks TSV overlay (bench marks <cx> <cz> output).
"""
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

def blk(pal, idx, x, y, z):
    return pal[idx[(y+64)*256 + z*16 + x]].replace("minecraft:", "").split("[")[0]

REF = Path("/home/z/my-project/c-faster/reference/dumps/vanilla/chunk_3_-2.crsf")
pa, ia = read_dump(REF)

# find all positions where ref has water/lava (the interesting region is deep)
# first: count diffs by material pair using our dump if present
OUR = Path("/tmp/judge_art/reference/dumps_plugin/vanilla/chunk_3_-2.crsf")
our_pa = our_ib = None
if OUR.exists():
    our_pa, our_ib = read_dump(OUR)

# find ref-only water pockets: scan deep y for water clusters
min_x = min_y = min_z = 10**9
max_x = max_y = max_z = -10**9
pocket = []
for y in range(-64, 320):
    for z in range(16):
        for x in range(16):
            if blk(pa, ia, x, y, z) == "water":
                if -60 <= y <= -20:  # deep aquifer zone
                    pocket.append((x, y, z))
                    min_x, min_y, min_z = min(min_x, x), min(min_y, y), min(min_z, z)
                    max_x, max_y, max_z = max(max_x, x), max(max_y, y), max(max_z, z)

print(f"deep water blocks (y -60..-20) in ref chunk_3_-2: {len(pocket)}")
print(f"extent x {min_x}..{max_x} y {min_y}..{max_y} z {min_z}..{max_z}")

# cluster: print per-y horizontal slices of deep water, compressed
from collections import Counter
per_y = Counter(y for _, y, _ in pocket)
ys = sorted(per_y)
print("per-y water counts:", {y: per_y[y] for y in ys if per_y[y] > 20})

# find the big room: connected air-in-ours / water-in-ref around x4-11 z0-6 y-50..-27
if our_pa:
    room = []
    for y in range(-64, 0):
        for z in range(16):
            for x in range(16):
                r = blk(pa, ia, x, y, z)
                o = blk(our_pa, our_ib, x, y, z)
                if r != o:
                    room.append((x, y, z, r, o))
    print(f"\nref-vs-ours diffs in this chunk: {len(room)}")
    mc = Counter((r, o) for _, _, _, r, o in room)
    print("material pairs:", dict(mc))
    xs = [t[0] for t in room]; ys2 = [t[1] for t in room]; zs = [t[2] for t in room]
    if room:
        print(f"extent x {min(xs)}..{max(xs)} y {min(ys2)}..{max(ys2)} z {min(zs)}..{max(zs)}")
