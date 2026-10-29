#!/usr/bin/env python3
"""Print non-stone blocks in a local brick from both A (vanilla) and B
(native) dumps to reveal the actual vein contours near a divergence."""
import struct, sys
from pathlib import Path

MINY, H = -64, 384

def read_dump(p):
    d = p.read_bytes()
    (magic, ver, cx, cz, miny, height) = struct.unpack_from("<Iiiiii", d, 0)
    off = 24
    (pl,) = struct.unpack_from("<I", d, off); off += 4
    pal = []
    for _ in range(pl):
        (s,) = struct.unpack_from("<H", d, off); off += 2
        pal.append(d[off:off+s].decode()); off += s
    n = 16*16*height
    idx = struct.unpack_from(f"<{n}H", d, off)
    return cx, cz, pal, idx

def blocks(brick, A, B):
    x0, x1, y0, y1, z0, z1 = brick
    for name, D, tag in (("A", A, "A"), ("B", B, "B")):
        out = []
        for f in sorted(Path(D).glob("*.crsf")):
            cx, cz, pal, idx = read_dump(f)
            for y in range(y0, y1+1):
                for z in range(z0, z1+1):
                    for x in range(x0, x1+1):
                        ccx, ccz = x >> 4, z >> 4
                        if (ccx, ccz) != (cx, cz):
                            continue
                        lx, ly, lz = x & 15, y - MINY, z & 15
                        i = ly*256 + lz*16 + lx
                        s = pal[idx[i]]
                        b = s.split(":")[1].split("[")[0]
                        if b in ("stone", "deepslate", "air", "cave_air", "bedrock"):
                            continue
                        out.append((x, y, z, b))
        out.sort()
        print(f"--- {tag} ({len(out)} blocks) ---")
        for x, y, z, b in out:
            print(f"  ({x},{y},{z}) {b}")

if __name__ == "__main__":
    brick = tuple(int(v) for v in sys.argv[1:7])
    blocks(brick, sys.argv[7], sys.argv[8])
