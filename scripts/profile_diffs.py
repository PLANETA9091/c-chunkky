#!/usr/bin/env python3
"""Vertical profiles and neighborhoods for the weirdest diffs."""
import struct
from pathlib import Path

MAGIC = 0x46534243
REF_V = Path("/home/z/my-project/c-faster/reference/dumps/vanilla")
REF_D = Path("/tmp/judge_art/reference/dumps_plugin/vanilla")

def read_dump(path):
    data = path.read_bytes()
    off = 24
    (pal_len,) = struct.unpack_from("<I", data, off); off += 4
    palette = []
    for _ in range(pal_len):
        (slen,) = struct.unpack_from("<H", data, off); off += 2
        palette.append(data[off:off+slen].decode()); off += slen
    idx = struct.unpack_from(f"<{16*16*384}H", data, off)
    return palette, idx

cache = {}
def get(d, name):
    key = (str(d), name)
    if key not in cache:
        cache[key] = read_dump(Path(d) / name)
    return cache[key]

def blk(pal, idx, x, y, z):
    return pal[idx[(y+64)*256 + z*16 + x]].split("[")[0].replace("minecraft:", "")

def profile(name, x, z, y0, y1):
    pa, ia = get(REF_V, name)
    pb, ib = get(REF_D, name)
    print(f"--- {name} col x={x} z={z} y {y0}..{y1} ---")
    for y in range(y1, y0-1, -1):
        a, b = blk(pa,ia,x,y,z), blk(pb,ib,x,y,z)
        mark = " <<<" if a != b else ""
        print(f"  y={y:4d} ref={a:14s} ours={b:14s}{mark}")

lines = Path("/tmp/diff_detail_vanilla.txt").read_text().splitlines()
bc = [l for l in lines if "bubble_column" in l]
ob = [l for l in lines if "obsidian" in l]

# first bubble column diff: full column profile
import re
toks = dict(p.split("=",1) for p in bc[0].split() if "=" in p)
name = bc[0].split()[0]
x, y, z = int(toks["x"]), int(toks["y"]), int(toks["z"])
profile(name, x, z, y-3, y+15)

# first obsidian diff: 3x3x3 neighborhood both sides
toks = dict(p.split("=",1) for p in ob[0].split() if "=" in p)
name2 = ob[0].split()[0]
x, y, z = int(toks["x"]), int(toks["y"]), int(toks["z"])
pa, ia = get(REF_V, name2)
pb, ib = get(REF_D, name2)
print(f"--- obsidian diff {name2} ({x},{y},{z}) 3x3 slice dy=0 ---")
for dz in range(2, -1, -1):
    row_r = " ".join(f"{blk(pa,ia,x+dx,y,z+dz):9s}" for dx in range(-1, 2))
    row_o = " ".join(f"{blk(pb,ib,x+dx,y,z+dz):9s}" for dx in range(-1, 2))
    print(f"  z{z+dz:+d} ref: {row_r}   ours: {row_o}")
print("  dy=+1 slice:")
for dz in range(1, -1, -1):
    row_r = " ".join(f"{blk(pa,ia,x+dx,y+1,z+dz):9s}" for dx in range(-1, 2))
    row_o = " ".join(f"{blk(pb,ib,x+dx,y+1,z+dz):9s}" for dx in range(-1, 2))
    print(f"  z{z+dz:+d} ref: {row_r}   ours: {row_o}")
