#!/usr/bin/env python3
"""Block-level detail of .crsf divergence: coordinates + states for each diff."""
import struct
import sys
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


def main() -> int:
    a, b = Path(sys.argv[1]), Path(sys.argv[2])
    for name in sorted({p.name for p in a.glob("*.crsf")} & {p.name for p in b.glob("*.crsf")}):
        cxa, cza, pal_a, idx_a = read_dump(a / name)
        cxb, czb, pal_b, idx_b = read_dump(b / name)
        diffs = []
        for i in range(len(idx_a)):
            sa, sb = pal_a[idx_a[i]], pal_b[idx_b[i]]
            if sa != sb:
                y, rem = divmod(i, 256)
                z, x = divmod(rem, 16)
                diffs.append((x + cxa * 16, y + MINY, z + cza * 16, sa, sb))
        if diffs:
            print(f"== {name} ({len(diffs)}):")
            for x, y, z, sa, sb in diffs:
                print(f"  ({x},{y},{z})  A={sa}  B={sb}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
