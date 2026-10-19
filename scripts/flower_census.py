#!/usr/bin/env python3
"""Census flower blocks (by type) in A vs B dumps for given chunk names."""
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

FLOWERS = ["poppy", "cornflower", "lily_of_the_valley", "oxeye_daisy", "short_grass", "fern", "bone_block"]

def main():
    a, b = Path(sys.argv[1]), Path(sys.argv[2])
    chunks = sys.argv[3:]
    for name in chunks:
        for tag, d in (("A(van)", a), ("B(nat)", b)):
            cx, cz, pal, idx = read_dump(d / f"{name}.crsf")
            hits = {}
            for i in range(len(idx)):
                s = pal[idx[i]]
                base = s.split("[")[0]
                short = base.replace("minecraft:", "")
                if short in FLOWERS:
                    y = MINY + i // 256
                    rem = i % 256
                    z = cz * 16 + rem // 16
                    x = cx * 16 + rem % 16
                    hits.setdefault(short, []).append((x, y, z))
            print(f"--- {name} {tag}")
            for k in FLOWERS:
                v = hits.get(k, [])
                if v:
                    print(f"  {k}: {len(v)} @ {sorted(v)[:8]}{'...' if len(v) > 8 else ''}")

if __name__ == "__main__":
    main()
