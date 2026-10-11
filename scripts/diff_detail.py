#!/usr/bin/env python3
"""Full block-level diff detail: reference vs plugin dumps. Prints every differing block."""
import struct, sys, collections
from pathlib import Path

MAGIC = 0x46534243

def read_dump(path):
    data = path.read_bytes()
    magic, version, cx, cz, miny, height = struct.unpack_from("<Iiiiii", data, 0)
    assert magic == MAGIC
    off = 24
    (pal_len,) = struct.unpack_from("<I", data, off); off += 4
    palette = []
    for _ in range(pal_len):
        (slen,) = struct.unpack_from("<H", data, off); off += 2
        palette.append(data[off:off+slen].decode()); off += slen
    n = 16*16*height
    idx = struct.unpack_from(f"<{n}H", data, off)
    return cx, cz, palette, idx

def main():
    ref_dir, plug_dir = Path(sys.argv[1]), Path(sys.argv[2])
    world = sys.argv[3] if len(sys.argv) > 3 else "vanilla"
    plug_files = {p.name: p for p in plug_dir.glob("*.crsf")}
    total = 0
    pairs = collections.Counter()
    per_y = collections.Counter()
    details = []
    for f in sorted(ref_dir.glob("*.crsf")):
        pf = plug_files.get(f.name)
        if not pf:
            continue
        cx, cz, pal_a, idx_a = read_dump(f)
        _, _, pal_b, idx_b = read_dump(pf)
        for i in range(len(idx_a)):
            ba, bb = pal_a[idx_a[i]], pal_b[idx_b[i]]
            if ba != bb:
                total += 1
                y = i // 256 - 64
                rem = i % 256
                z, x = divmod(rem, 16)
                pairs[(ba.split("[")[0], bb.split("[")[0])] += 1
                per_y[y] += 1
                details.append((f.name, x, y, z, ba, bb))
    print(f"WORLD {world}: {total} blocks differ")
    print("--- transition pairs (ref -> ours) ---")
    for (a, b), n in pairs.most_common(20):
        print(f"  {n:6d}  {a}  ->  {b}")
    print("--- per-Y histogram (top 30) ---")
    for y, n in sorted(per_y.items(), key=lambda kv: -kv[1])[:30]:
        print(f"  y={y:4d}: {n}")
    out = Path(f"/tmp/diff_detail_{world}.txt")
    with out.open("w") as fh:
        for d in details:
            fh.write(f"{d[0]} x={d[1]} y={d[2]} z={d[3]} ref={d[4]} ours={d[5]}\n")
    print(f"details -> {out}")

main()
