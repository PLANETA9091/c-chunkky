#!/usr/bin/env python3
"""Compare two .crsf dump sets (e.g. reference/dumps/vanilla vs reference/dumps/datapack).

Usage: python3 diff_dumps.py <dirA> <dirB>

For every chunk file present in BOTH dirs, resolves palette indices to block
state strings and counts block-for-block differences. Also compares per-column
heightmaps (topmost non-air block Y). Prints:
  - per-chunk diff %
  - overall diff % across all compared chunks
  - heightmap delta stats (mean |dh| per column, % columns changed)
"""
import struct
import sys
from pathlib import Path

MAGIC = 0x46534243
HEIGHT = 384
MINY = -64


def read_dump(path: Path):
    data = path.read_bytes()
    (magic, version, cx, cz, miny, height) = struct.unpack_from("<Iiiiii", data, 0)
    if magic != MAGIC:
        raise ValueError(f"bad magic in {path}")
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


def heightmap(pal, idx):
    hm = []
    for z in range(16):
        for x in range(16):
            top = MINY - 1
            for y in range(HEIGHT - 1, -1, -1):
                i = y * 256 + z * 16 + x
                if pal[idx[i]] != "minecraft:air":
                    top = y + MINY
                    break
            hm.append(top)
    return hm


def main() -> int:
    a, b = Path(sys.argv[1]), Path(sys.argv[2])
    files_a = {p.name for p in a.glob("*.crsf")}
    files_b = {p.name for p in b.glob("*.crsf")}
    common = sorted(files_a & files_b)
    only_a = sorted(files_a - files_b)
    only_b = sorted(files_b - files_a)
    if only_a:
        print(f"only in {a}: {only_a[:5]}{'...' if len(only_a) > 5 else ''} ({len(only_a)})")
    if only_b:
        print(f"only in {b}: {only_b[:5]}{'...' if len(only_b) > 5 else ''} ({len(only_b)})")
    if not common:
        print("no common chunk files — nothing to compare")
        return 1
    total = 0
    diff = 0
    hm_changed = 0
    hm_abs_sum = 0
    for name in common:
        cxa, cza, pal_a, idx_a = read_dump(a / name)
        cxb, czb, pal_b, idx_b = read_dump(b / name)
        n = len(idx_a)
        d = 0
        for i in range(n):
            if pal_a[idx_a[i]] != pal_b[idx_b[i]]:
                d += 1
        ha, hb = heightmap(pal_a, idx_a), heightmap(pal_b, idx_b)
        for u, v in zip(ha, hb):
            if u != v:
                hm_changed += 1
                hm_abs_sum += abs(u - v)
        total += n
        diff += d
        print(f"  {name}: diff {d}/{n} = {100.0 * d / n:.2f}%")
    print(f"OVERALL: {diff}/{total} blocks differ = {100.0 * diff / total:.3f}% across {len(common)} chunks")
    cols = 256 * len(common)
    print(f"HEIGHTMAP: {hm_changed}/{cols} columns changed; mean |dh| = {hm_abs_sum / cols:.2f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
