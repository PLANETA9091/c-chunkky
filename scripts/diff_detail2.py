#!/usr/bin/env python3
"""Position-level diff between two .crsf chunk dumps: block transitions,
y-histogram, and a slice printout of the biggest divergent region."""
import struct, sys
from pathlib import Path
from collections import Counter

MAGIC = 0x46534243
HEIGHT = 384
MINY = -64

def read_dump(path):
    data = path.read_bytes()
    (magic, version, cx, cz, miny, height) = struct.unpack_from("<Iiiiii", data, 0)
    assert magic == MAGIC
    off = 24
    (pal_len,) = struct.unpack_from("<I", data, off)
    off += 4
    palette = []
    for _ in range(pal_len):
        (slen,) = struct.unpack_from("<H", data, off)
        off += 2
        palette.append(data[off:off + slen].decode("utf-8"))
        off += slen
    n = 16 * 16 * height
    idx = struct.unpack_from(f"<{n}H", data, off)
    return cx, cz, palette, idx

def base(name):
    return name.split("[")[0]

a, b = Path(sys.argv[1]), Path(sys.argv[2])
want = sys.argv[3] if len(sys.argv) > 3 else None
ca, cza, pa, ia = read_dump(a)
cb, czb, pb, ib = read_dump(b)
trans = Counter()
ys = Counter()
diffs = []
n = len(ia)
for y in range(HEIGHT):
    for lz in range(16):
        for lx in range(16):
            i = y * 256 + lz * 16 + lx
            na, nb = pa[ia[i]], pb[ib[i]]
            if na != nb:
                trans[(base(na), base(nb))] += 1
                ys[y + MINY] += 1
                diffs.append((lx, y + MINY, lz, na, nb))
total = sum(trans.values())
print(f"{a.name} vs {b.name}: {total} diffs")
for (na, nb), c in trans.most_common(15):
    print(f"  {c:6d}  {na} -> {nb}")
yv = sorted(ys.items())
if yv:
    lo, hi = yv[0][0], yv[-1][0]
    print(f"y range: {lo}..{hi} (top-10 y: {sorted(ys.items(), key=lambda kv: -kv[1])[:10]})")
# z-slice histogram: which (z) columns
zs = Counter(d[2] for d in diffs)
print("per-lz:", dict(sorted(zs.items())))
xs = Counter(d[0] for d in diffs)
print("per-lx:", dict(sorted(xs.items())))
