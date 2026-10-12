#!/usr/bin/env python3
"""Cross-reference: candidate boundary-water positions (both worlds water,
adjacent to a diff block) vs our native marks (bench marks TSV).

Usage: python3 scripts/marks_xref.py <marks_tsv>...
"""
import struct, sys, glob
from pathlib import Path
from collections import defaultdict

BASE = Path("/home/z/my-project/c-faster/reference/dumps/vanilla")
OURS = Path("/tmp/judge_art/reference/dumps_plugin/vanilla")

def read_dump(path):
    data = Path(path).read_bytes()
    off = 24
    (n,) = struct.unpack_from("<I", data, off); off += 4
    pal = []
    for _ in range(n):
        (l,) = struct.unpack_from("<H", data, off); off += 2
        pal.append(data[off:off+l].decode()); off += l
    idx = struct.unpack_from(f"<{16*16*384}H", data, off)
    return pal, idx

def blk(p, i, x, y, z):
    if not (0 <= x < 16 and 0 <= z < 16 and -64 <= y < 320):
        return "OUT"
    return p[i[(y+64)*256 + z*16 + x]].replace("minecraft:", "").split("[")[0]

CHUNKS = [(2,-2),(2,-3),(3,-2),(3,-3)]

# world-coord dump caches
ref = {}
our = {}
for cx, cz in CHUNKS:
    ref[(cx,cz)] = read_dump(BASE / f"chunk_{cx}_{cz}.crsf")
    our[(cx,cz)] = read_dump(OURS / f"chunk_{cx}_{cz}.crsf")

def wblk(store, x, y, z):
    cx, cz = x >> 4, z >> 4
    if (cx,cz) not in store: return "OUT"
    pal, idx = store[(cx,cz)]
    return blk(pal, idx, x & 15, y, z & 15)

# collect diffs per chunk (world coords) + candidate noise-stage water boundary
diffs = set()
for (cx,cz) in CHUNKS:
    pa, ia = ref[(cx,cz)]
    pb, ib = our[(cx,cz)]
    for y in range(-64, 320):
        for z in range(16):
            for x in range(16):
                r = blk(pa, ia, x, y, z)
                o = blk(pb, ib, x, y, z)
                if r != o:
                    diffs.add((48 + x if cx==3 else 32 + x + (cx-2)*16, y, z))  # placeholder

# rebuild with proper world coords
diffs = set()
for (cx,cz) in CHUNKS:
    pa, ia = ref[(cx,cz)]
    pb, ib = our[(cx,cz)]
    wx0, wz0 = cx*16, cz*16
    for y in range(-64, 320):
        for z in range(16):
            for x in range(16):
                r = blk(pa, ia, x, y, z)
                o = blk(pb, ib, x, y, z)
                if r != o:
                    diffs.add((wx0+x, y, wz0+z))

print(f"total diff blocks in region: {len(diffs)}")

# candidate sources: noise-stage water (both worlds) adjacent (6-neighborhood) to a diff
cands = set()
for (x,y,z) in diffs:
    for dx,dy,dz in ((1,0,0),(-1,0,0),(0,1,0),(0,-1,0),(0,0,1),(0,0,-1)):
        p = (x+dx, y+dy, z+dz)
        if wblk(ref, *p) == "water" and wblk(our, *p) == "water":
            cands.add(p)

# also 2-block ring (flow reach) — water 2 blocks away from a diff through air
for (x,y,z) in list(diffs):
    for dx,dy,dz in ((2,0,0),(-2,0,0),(0,2,0),(0,-2,0),(0,0,2),(0,0,-2)):
        p = (x+dx, y+dy, z+dz)
        if wblk(ref, *p) == "water" and wblk(our, *p) == "water":
            cands.add(p)

print(f"candidate mark positions (noise-stage water near diff): {len(cands)}")

# our marks from TSVs
marks = set()
for tsv in sys.argv[1:]:
    for line in Path(tsv).read_text().splitlines():
        if line.startswith("#") or not line.strip(): continue
        x, y, z, b, name = line.split()
        marks.add((int(x), int(y), int(z)))

print(f"our marks (from TSVs): {len(marks)}")
hit = cands & marks
print(f"\ncandidates MARKED by us: {len(hit)}/{len(cands)}")
for p in sorted(hit, key=lambda t:(t[1],t[0],t[2])):
    print("  MARKED  ", p)
miss = cands - marks
print(f"candidates NOT marked: {len(miss)}")
for p in sorted(miss, key=lambda t:(t[1],t[0],t[2]))[:60]:
    print("  missing ", p)
