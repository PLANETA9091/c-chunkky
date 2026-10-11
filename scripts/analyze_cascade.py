#!/usr/bin/env python3
"""For bubble_column and obsidian diffs: inspect blocks below/neighbors in ref vs ours."""
import struct, collections
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
    n = 16*16*384
    idx = struct.unpack_from(f"<{n}H", data, off)
    return palette, idx

cache = {}
def get(d, name):
    key = (str(d), name)
    if key not in cache:
        cache[key] = read_dump(Path(d) / name)
    return cache[key]

def blk(pal, idx, x, y, z):
    if y < -64 or y > 319: return "OOR"
    return pal[idx[(y+64)*256 + z*16 + x]].split("[")[0]

lines = Path("/tmp/diff_detail_vanilla.txt").read_text().splitlines()
bc = [l for l in lines if "bubble_column" in l]
ob = [l for l in lines if "obsidian" in l]

print("=== bubble_column diffs: block below in ref vs ours ===")
below = collections.Counter()
for l in bc[:92]:
    toks = dict(p.split("=",1) for p in l.split() if "=" in p)
    name = l.split()[0]
    x, y, z = int(toks["x"]), int(toks["y"]), int(toks["z"])
    pa, ia = get(REF_V, name)
    pb, ib = get(REF_D, name)
    below[(blk(pa,ia,x,y-1,z), blk(pb,ib,x,y-1,z))] += 1
for k, n in below.most_common():
    print(f"  below ref={k[0]} ours={k[1]}: {n}")

print("=== obsidian diffs (ref=obsidian ours=lava): ref neighbors ===")
nb = collections.Counter()
for l in ob:
    toks = dict(p.split("=",1) for p in l.split() if "=" in p)
    name = l.split()[0]
    x, y, z = int(toks["x"]), int(toks["y"]), int(toks["z"])
    pa, ia = get(REF_V, name)
    pb, ib = get(REF_D, name)
    has_water_ref = any(blk(pa,ia,x+dx,y+dy,z+dz)=="water" for dx,dy,dz in ((1,0,0),(-1,0,0),(0,1,0),(0,-1,0),(0,0,1),(0,0,-1)))
    has_water_ours = any(blk(pb,ib,x+dx,y+dy,z+dz)=="water" for dx,dy,dz in ((1,0,0),(-1,0,0),(0,1,0),(0,-1,0),(0,0,1),(0,0,-1)))
    nb[(has_water_ref, has_water_ours)] += 1
print("  (ref has water neighbor, ours has water neighbor):", dict(nb))
