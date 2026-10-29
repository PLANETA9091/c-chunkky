#!/usr/bin/env python3
"""Print full detail (x,y,z + both states) of divergent blocks between two crsf dirs."""
import struct, sys
from pathlib import Path

MAGIC = 0x46534243
HEIGHT = 384
MINY = -64

def read_dump(path: Path):
    data = path.read_bytes()
    (magic, version, cx, cz, miny, height) = struct.unpack_from("<Iiiiii", data, 0)
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

def main():
    a, b = Path(sys.argv[1]), Path(sys.argv[2])
    files_a = {p.name for p in a.glob("*.crsf")}
    files_b = {p.name for p in b.glob("*.crsf")}
    common = sorted(files_a & files_b)
    total_diff = 0
    for name in common:
        cxa, cza, pal_a, idx_a = read_dump(a / name)
        cxb, czb, pal_b, idx_b = read_dump(b / name)
        n = len(idx_a)
        d = 0
        for i in range(n):
            if pal_a[idx_a[i]] != pal_b[idx_b[i]]:
                y = MINY + i // 256
                rem = i % 256
                z = rem // 16
                x = rem % 16
                wx = cxa * 16 + x
                wz = cza * 16 + z
                sa = pal_a[idx_a[i]]
                sb = pal_b[idx_b[i]]
                print(f"{name} block ({wx},{y},{wz}): A={sa} B={sb}")
                d += 1
        total_diff += d
    print(f"TOTAL: {total_diff}")

if __name__ == "__main__":
    main()
