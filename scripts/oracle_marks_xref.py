#!/usr/bin/env python3
"""Cross-reference vanilla aquifer marks (oracle dump) vs our native marks.

Vanilla file format (oracle_marks_out/chunk_X_Y_marks.txt):
  "x y z blockname true|false"  for every placed block with fluid or flag=true
  "AQ idx src x y z level block" lines = aquifer status cache dump at EOF

Our file format (bench marks TSV):
  "# comment" + "x y z blockid blockname"

Prints:
  - vanilla marked (flag=true) positions and how many we also marked
  - UNDER-MARKED: vanilla=true, ours=false  <- the flag-port bug list
  - OVER-MARKED:  ours=true, vanilla=false
"""
import sys
from pathlib import Path

oracle_dir = Path(sys.argv[1])
ours_files = sys.argv[2:]

ours = set()
for f in ours_files:
    for line in Path(f).read_text().splitlines():
        if line.startswith("#") or not line.strip():
            continue
        x, y, z, b, name = line.split()
        ours.add((int(x), int(y), int(z)))

van_all = {}       # (x,y,z) -> (block, flag) for fluid-or-flagged blocks
for f in sorted(oracle_dir.glob("*_marks.txt")):
    for line in f.read_text().splitlines():
        if line.startswith("AQ") or not line.strip():
            continue
        parts = line.split()
        x, y, z = int(parts[0]), int(parts[1]), int(parts[2])
        block, flag = parts[3], parts[4] == "true"
        van_all[(x, y, z)] = (block, flag)

van_marked = {p for p, (b, f) in van_all.items() if f}
our_marked = ours

print(f"vanilla fluid/flagged blocks: {len(van_all)}  flagged: {len(van_marked)}")
print(f"our marks: {len(our_marked)}")

under = van_marked - our_marked
over = our_marked - van_marked
print(f"\nUNDER-MARKED (vanilla=true, ours=false): {len(under)}")
for p in sorted(under, key=lambda t: (t[1], t[0], t[2])):
    b = van_all.get(p, ("?",))[0]
    print(f"  {p[0]} {p[1]} {p[2]}  {b}")
print(f"\nOVER-MARKED (ours=true, vanilla=false): {len(over)}")
for p in sorted(over, key=lambda t: (t[1], t[0], t[2]))[:80]:
    print(f"  {p[0]} {p[1]} {p[2]}")

# pocket candidates (known from local analysis): noise-stage water adjacent to
# the unfilled pocket; print their vanilla flags explicitly
CANDS = [(60,-14,-29),(61,-14,-30),(61,-13,-30),(61,-13,-29),(61,-12,-29),
         (62,-11,-29),(63,-10,-29),(62,-9,-27),(63,-9,-28)]
print("\npocket-boundary candidates (vanilla view):")
for p in CANDS:
    b, f = van_all.get(p, ("NOT-FLUID?", None))
    print(f"  {p} block={b} vanilla_flag={f} ours_marked={p in ours}")
