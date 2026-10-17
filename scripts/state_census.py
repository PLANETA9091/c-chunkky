#!/usr/bin/env python3
"""State census: compare palette+counts between ref and plugin .crsf dumps."""
import struct, sys, collections
from pathlib import Path

MAGIC = 0x46534243

def read_dump(path):
    data = Path(path).read_bytes()
    magic, version, cx, cz, miny, height = struct.unpack_from("<Iiiiii", data, 0)
    assert magic == MAGIC, f"bad magic in {path}"
    off = 24
    (pal_len,) = struct.unpack_from("<I", data, off); off += 4
    palette = []
    for _ in range(pal_len):
        (slen,) = struct.unpack_from("<H", data, off); off += 2
        palette.append(data[off:off+slen].decode()); off += slen
    n = 16*16*height
    idx = struct.unpack_from(f"<{n}H", data, off)
    return miny, height, palette, idx

def census(path):
    miny, height, palette, idx = read_dump(path)
    c = collections.Counter(palette[i] for i in idx)
    return miny, height, palette, c

a, b = sys.argv[1], sys.argv[2]
ma, ha, pa, ca = census(a)
mb, hb, pb, cb = census(b)
print(f"ref : miny={ma} h={ha} pal={len(pa)}")
print(f"ours: miny={mb} h={hb} pal={len(pb)}")
allnames = sorted(set(ca) | set(cb))
print(f"{'state':52s} {'ref':>8s} {'ours':>8s} {'delta':>8s}")
for n in allnames:
    d = cb.get(n, 0) - ca.get(n, 0)
    if abs(d) > 40 or n not in cb or n not in ca:
        print(f"{n:52s} {ca.get(n,0):8d} {cb.get(n,0):8d} {d:+8d}")
