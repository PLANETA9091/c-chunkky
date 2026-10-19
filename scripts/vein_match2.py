#!/usr/bin/env python3
"""Match native vein traces (server/run.out) against the diff positions of
the F2 probe to find the veins whose contours diverged."""
import re, sys
from pathlib import Path

DIFFS = [  # (x, y, z) from detail_diff, ore entries only
    (1600,12,1727),(1600,13,1727),
    (1627,7,1718),
    (1640,-12,1720),(1643,-12,1720),(1640,-11,1720),(1637,5,1717),
    (1639,6,1719),(1637,7,1718),(1646,7,1725),(1637,8,1719),
    (1637,8,1720),(1647,8,1726),(1638,10,1717),(1640,10,1718),
    (1656,-22,1723),(1656,-21,1721),(1655,-21,1722),(1654,-20,1720),
    (1651,-18,1725),(1659,-7,1727),(1659,-6,1727),(1659,2,1717),
    (1658,2,1718),(1658,2,1720),(1658,3,1716),(1658,3,1717),
    (1660,4,1719),(1654,5,1722),(1652,6,1723),
    (1676,-22,1665),(1676,-21,1665),(1677,-21,1666),(1670,-6,1676),
    (1668,1,1671),(1669,1,1673),(1668,2,1669),(1668,2,1673),
    (1670,2,1673),(1667,3,1668),(1669,3,1670),
    (1711,7,1671),(1710,8,1671),(1711,8,1671),(1711,8,1672),
    (1711,9,1671),(1711,9,1672),
    (1679,6,1631),(1681,8,1630),
    (1694,-7,1600),(1695,-7,1600),(1694,-6,1600),(1695,-6,1600),
]

veins = []
pat = re.compile(r"\[vein@(\d+)\] o=\((-?\d+),(-?\d+),(-?\d+)\) f=([\d.]+) box=\((-?\d+),(-?\d+),(-?\d+)\) w=(\d+) h=(\d+) size=(\d+) discard=([\d.]+)")
txt = Path("server/run.out").read_text()
for line in txt.splitlines():
    m = pat.search(line)
    if not m:
        continue
    cx = int(m.group(1))
    o = tuple(int(m.group(i)) for i in (2, 3, 4))
    f = float(m.group(5))
    box = tuple(int(m.group(i)) for i in (6, 7, 8))
    w, h, size = int(m.group(9)), int(m.group(10)), int(m.group(11))
    disc = float(m.group(12))
    veins.append((cx, o, f, box, w, h, size, disc))

print(f"{len(veins)} veins traced")
hit = {}
for cx, o, f, box, w, h, size, disc in veins:
    x0, y0, z0 = box
    for (dx, dy, dz) in DIFFS:
        if x0 <= dx <= x0 + w and y0 <= dy <= y0 + h and z0 <= dz <= z0 + w:
            hit.setdefault((dx, dy, dz), []).append((cx, o, f, box, size, disc))

mapped = {d for d in hit}
missed = [d for d in DIFFS if d not in mapped]
print(f"diff positions inside some traced vein box: {len(mapped)}/{len(DIFFS)}")
print(f"unmapped: {missed}")
for d, vs in sorted(hit.items()):
    for (cx, o, f, box, size, disc) in vs:
        print(f"  diff {d} <- vein chunk@{cx} o={o} f={f:.6f} box={box} size={size}")
