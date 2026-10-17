#!/usr/bin/env python3
"""make_dpext.py — rebuild /tmp/dpext/merged/{01,02} from the world datapack
zips with the SAME overlay-activation rule as the plugin's extractPackData:
per pack, pack-root data/** first, then each ACTIVE overlay dir's data/**
merged on top. An overlay is active when its pack.mcmeta format range covers
PACK_FORMAT (81 = MC 1.21.8). Pack order: zip name order, later wins."""

import json, os, shutil, sys, zipfile

PACK_FORMAT = 81
WORLD_DP = "/home/z/my-project/c-faster/server/dptt/datapacks"
OUT = "/tmp/dpext/merged"

def active_overlays(zf):
    mc = json.load(zf.open("pack.mcmeta"))
    out = []
    entries = mc.get("overlays", {}).get("entries",
              mc.get("overlays", {}).get("overlays", []))
    for ov in entries:
        d = ov.get("directory", "")
        fmt = ov.get("formats")
        lo = ov.get("min_format", fmt.get("min_inclusive") if isinstance(fmt, dict) else None)
        hi = ov.get("max_format", fmt.get("max_exclusive") if isinstance(fmt, dict) else None)
        if (lo is None or hi is None) and isinstance(fmt, list) and len(fmt) == 2:
            lo, hi = fmt
        if lo is None or hi is None:
            continue
        if int(lo) <= PACK_FORMAT <= int(hi) or (int(lo) <= PACK_FORMAT < int(hi) and "max_exclusive" in (fmt or {})):
            out.append(d)
    return out

def extract(zf, prefix, dst):
    n = 0
    for e in zf.namelist():
        if e.endswith("/") or not e.startswith(prefix):
            continue
        rel = e[len(prefix):]
        if not rel:
            continue
        # CLI tools (dpload/sfcmp/dpcmp) expect data/<ns>/... under the root
        tgt = os.path.join(dst, "data", rel)
        os.makedirs(os.path.dirname(tgt), exist_ok=True)
        with zf.open(e) as src, open(tgt, "wb") as f:
            shutil.copyfileobj(src, f)
        n += 1
    return n

def main():
    shutil.rmtree(OUT, ignore_errors=True)
    os.makedirs(OUT, exist_ok=True)
    packs = sorted(f for f in os.listdir(WORLD_DP) if f.endswith(".zip")
                   and not f.startswith("disabled_"))
    for i, name in enumerate(packs, 1):
        dst = os.path.join(OUT, f"{i:02d}")
        os.makedirs(dst, exist_ok=True)
        with zipfile.ZipFile(os.path.join(WORLD_DP, name)) as zf:
            n1 = extract(zf, "data/", dst)
            ovs = active_overlays(zf)
            n2 = 0
            for ov in ovs:
                # overlay data/ merges OVER the pack root data/ (same tree)
                n2 += extract(zf, ov + "/data/", dst)
            print(f"{name}: root={n1} overlays={ovs} (+{n2}) -> {dst}")

if __name__ == "__main__":
    main()
