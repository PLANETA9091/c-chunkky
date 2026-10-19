#!/usr/bin/env python3
"""Vein-level analysis of the F2 probe divergence: per chunk, per ore type,
count A-only / B-only / shared ore blocks. A vein that shifted wholesale
shows up as A-only ~ B-only with shared ~ 0; a contour dither shows up as
shared >> misses."""
import struct, sys
from pathlib import Path
from collections import Counter

MINY, H = -64, 384

def read_dump(p):
    d = p.read_bytes()
    (magic, ver, cx, cz, miny, height) = struct.unpack_from("<Iiiiii", d, 0)
    off = 24
    (pl,) = struct.unpack_from("<I", d, off); off += 4
    pal = []
    for _ in range(pl):
        (s,) = struct.unpack_from("<H", d, off); off += 2
        pal.append(d[off:off+s].decode()); off += s
    n = 16*16*height
    idx = struct.unpack_from(f"<{n}H", d, off)
    return cx, cz, pal, idx

def base(name):
    n = name.split("[")[0]
    return n.rsplit(":", 1)[-1]

A = Path("reference/dumps/f1probe_vanilla")
B = Path("reference/dumps_plugin/f1probe")

tot = Counter()
for name in sorted(p.name for p in A.glob("*.crsf")):
    cxa, cza, pa, ia = read_dump(A/name)
    cxb, czb, pb, ib = read_dump(B/name)
    orea = Counter(); oreb = Counter()
    for i in range(len(ia)):
        sa = base(pa[ia[i]]); sb = base(pb[ib[i]])
        if sa.endswith("_ore"): orea[sa] += 1
        if sb.endswith("_ore"): oreb[sb] += 1
    if not orea and not oreb:
        continue
    # shared positions per ore kind
    posa = {}; posb = {}
    for i in range(len(ia)):
        sa = base(pa[ia[i]]); sb = base(pb[ib[i]])
        if sa.endswith("_ore"): posa[i] = sa
        if sb.endswith("_ore"): posb[i] = sb
    shared = sum(1 for i, s in posa.items() if posb.get(i) == s)
    lost = len(posa) - shared      # in A not in B (or different ore)
    gained = len(posb) - shared    # in B not in A
    print(f"{name}: A={len(posa):5d} B={len(posb):5d} shared={shared:5d} "
          f"A-lost={lost:4d} B-gained={gained:4d}  "
          f"Akinds={dict(orea)} Bkinds={dict(oreb)}")
    tot["A"] += len(posa); tot["B"] += len(posb); tot["sh"] += shared
    tot["lost"] += lost; tot["gained"] += gained

print(f"\nTOTAL: A={tot['A']} B={tot['B']} shared={tot['sh']} "
      f"A-lost={tot['lost']} B-gained={tot['gained']} "
      f"({100*tot['sh']/max(1,tot['A']):.3f}% of A shared)")
