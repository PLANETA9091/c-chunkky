#!/usr/bin/env python3
"""Analyze diff detail positions: chunk-border clustering, fluids only."""
import sys, collections
from pathlib import Path

f = Path("/tmp/diff_detail_vanilla.txt")
fluids = {"minecraft:water", "minecraft:air", "minecraft:lava", "minecraft:obsidian", "minecraft:bubble_column"}
rows = []
for line in f.read_text().splitlines():
    toks = dict(p.split("=", 1) for p in line.split() if "=" in p)
    name = line.split()[0]
    rows.append({**toks, "chunk": name})

# fluids only (exclude cascade: bubble_column/granite/kelp)
fl = [r for r in rows if r["ref"].split("[")[0] in {"minecraft:water","minecraft:air","minecraft:lava","minecraft:obsidian"}]
print(f"fluid rows: {len(fl)}")
xborder = collections.Counter()
zborder = collections.Counter()
bychunk = collections.Counter()
ycount = collections.Counter()
for r in fl:
    x, y, z = int(r["x"]), int(r["y"]), int(r["z"])
    if x in (0, 15): xborder["x%d" % x] += 1
    if z in (0, 15): zborder["z%d" % z] += 1
    bychunk[r["chunk"]] += 1
    ycount[y] += 1
print("x=0/15 count:", sum(xborder.values()), " z=0/15 count:", sum(zborder.values()))
print("per-chunk (top 12):", bychunk.most_common(12))
print("y range:", min(ycount), "..", max(ycount))
print("distinct chunks affected:", len(bychunk))

# show 25 examples with transitions
trans = collections.Counter()
for r in fl:
    trans[(r["ref"].split("[")[0], r["ours"].split("[")[0])] += 1
print("transitions:", dict(trans))
for r in fl[:25]:
    print(f"  {r['chunk']} ({r['x']},{r['y']},{r['z']}) ref={r['ref'].split('[')[0]} ours={r['ours'].split('[')[0]}")
