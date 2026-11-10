#!/usr/bin/env python3
"""cont-134 fill bit-parity: parent (d7b1b514) filldump vs working-tree filldump
over the judge chunk coords from reference/dumps_plugin/dptt/. Bit-exact means
every (x,y,z)->block line identical (noise+surface fill, no deco).

Usage: python3 scripts/parity_filldump.py [n_chunks]
"""
import subprocess, sys, re
from pathlib import Path

ROOT = Path("/home/z/my-project/c-faster")
NEW = str(ROOT / "native/target/release/filldump")
PAR = "/tmp/parent_tgt/release/filldump"
DPREF = ROOT / "reference/dumps_plugin/dptt"
ARGS = ["workdir/dpext/01", "workdir/dpext/02", "--"]  # 01 tectonic + 02 terralith

coords = []
for f in sorted(DPREF.glob("chunk_*.crsf")):
    m = re.match(r"chunk_(-?\d+)_(-?\d+)\.crsf", f.name)
    if m:
        coords.append((int(m.group(1)), int(m.group(2))))
n = int(sys.argv[1]) if len(sys.argv) > 1 else len(coords)
coords = coords[:n]

outdir = Path("/tmp/parity_filldump")
(outdir / "new").mkdir(parents=True, exist_ok=True)
(outdir / "par").mkdir(parents=True, exist_ok=True)

bad = 0
for i, (cx, cz) in enumerate(coords):
    tag = f"{cx}_{cz}"
    rn = outdir / "new" / f"{tag}.txt"
    rp = outdir / "par" / f"{tag}.txt"
    subprocess.run([NEW, *ARGS, str(cx), str(cz), "3257840388504953787", str(rn)],
                   cwd=ROOT, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run([PAR, *ARGS, str(cx), str(cz), "3257840388504953787", str(rp)],
                   cwd=ROOT, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    a = rn.read_bytes()
    b = rp.read_bytes()
    status = "OK " if a == b else "DIFF"
    if a != b:
        bad += 1
        # first differing line
        la, lb = a.decode().splitlines(), b.decode().splitlines()
        for j, (x, y) in enumerate(zip(la, lb)):
            if x != y:
                print(f"  first diff line {j}: new={x!r} par={y!r}")
                break
        else:
            print(f"  length diff: new={len(la)} par={len(lb)}")
    print(f"[{i+1}/{len(coords)}] chunk {tag}: {status} ({len(a)} bytes)")
print(f"PARITY: {len(coords)-bad}/{len(coords)} bit-exact" + ("" if bad == 0 else f"  *** {bad} DIFF ***"))
sys.exit(1 if bad else 0)
