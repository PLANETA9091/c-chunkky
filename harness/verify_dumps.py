#!/usr/bin/env python3
"""Verify a directory of .crsf chunk dumps (format + non-degeneracy checks).

Usage: python3 verify_dumps.py <dir> [expected-count]

Checks per file:
  - magic 0x46534243, version 1
  - dims: 16x16x384 with minY=-64 (overworld frozen params)
  - palette non-trivial: >1 entry, contains at least one of
    stone/dirt/grass_block/water/deepslate/sand/gravel
  - not fully air
  - bottom layer (y=-64) center block is bedrock (validates y layout)
Exit 0 if all checks pass, 1 otherwise.
"""
import struct
import sys
from pathlib import Path

MAGIC = 0x46534243
EXPECT_NONTRIVIAL = ("stone", "dirt", "grass_block", "water", "deepslate", "sand", "gravel")


def read_dump(path: Path):
    data = path.read_bytes()
    off = 0
    (magic, version, cx, cz, miny, height) = struct.unpack_from("<Iiiiii", data, off)
    off += 24
    if magic != MAGIC:
        raise ValueError(f"bad magic {magic:#x}")
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
    off += n * 2
    if off != len(data):
        raise ValueError(f"trailing bytes: {len(data) - off}")
    return dict(version=version, cx=cx, cz=cz, miny=miny, height=height,
                palette=palette, idx=idx)


def main() -> int:
    d = Path(sys.argv[1])
    expect = int(sys.argv[2]) if len(sys.argv) > 2 else None
    files = sorted(d.glob("*.crsf"))
    print(f"verify {d}: {len(files)} files" + (f" (expect {expect})" if expect else ""))
    fails = []
    if expect is not None and len(files) != expect:
        fails.append(f"count {len(files)} != {expect}")
    for f in files:
        try:
            dump = read_dump(f)
        except Exception as e:  # noqa: BLE001
            fails.append(f"{f.name}: FORMAT {e}")
            continue
        pal = dump["palette"]
        idx = dump["idx"]
        errs = []
        if dump["version"] != 1:
            errs.append(f"version={dump['version']}")
        if dump["miny"] != -64 or dump["height"] != 384:
            errs.append(f"dims minY={dump['miny']} h={dump['height']}")
        if len(pal) < 2:
            errs.append(f"degenerate palette ({len(pal)})")
        if not any(any(name in s for s in pal) for name in EXPECT_NONTRIVIAL):
            errs.append("palette lacks any expected terrain block")
        air_ids = [i for i, s in enumerate(pal) if s == "minecraft:air"]
        if air_ids and all(i == air_ids[0] for i in idx):
            errs.append("chunk fully air")
        # y-layout sanity: bedrock floor at y=-64 (offset plane 0), center column
        # idx layout is (y-min_y)*256 + z*16 + x, so y=minY maps to plane 0.
        bottom = idx[8 * 16 + 8]
        if pal[bottom] != "minecraft:bedrock":
            errs.append(f"y=-64 center is {pal[bottom]}, expected bedrock (y-layout off?)")
        if errs:
            fails.append(f"{f.name}: " + "; ".join(errs))
    for x in fails:
        print("FAIL", x)
    print("PASS" if not fails else f"{len(fails)} FAILURES")
    return 0 if not fails else 1


if __name__ == "__main__":
    sys.exit(main())
