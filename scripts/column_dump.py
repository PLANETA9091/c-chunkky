#!/usr/bin/env python3
"""Dump a column (x,z) y range from both dump dirs."""
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
    x, z = int(sys.argv[3]), int(sys.argv[4])
    y0, y1 = int(sys.argv[5]), int(sys.argv[6])
    for tag, d in (("A(van)", a), ("B(nat)", b)):
        cx, cz, pal, idx = read_dump(d / sys.argv[7])
        lx, lz = x - cx * 16, z - cz * 16
        print(f"--- column ({x},{z}) {tag}")
        for y in range(y1, y0 - 1, -1):
            i = (y - MINY) * 256 + lz * 16 + lx
            s = pal[idx[i]]
            if not s.endswith("minecraft:air"):
                print(f"  y={y}: {s}")

if __name__ == "__main__":
    main()
