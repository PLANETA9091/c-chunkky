#!/usr/bin/env python3
"""Replicate CrusstyChunkGenerator.extractPackData asset extraction for the
biomefill diagnostic: world datapack zips in name order, root data/ first,
then active pack.mcmeta overlays (PACK_FORMAT=81, Paper 1.21.8) on top.
Keeps data/<ns>/worldgen/{noise_settings,density_function,noise}/... and
data/minecraft/dimension/overworld.json -> <ns>/worldgen/...

Usage: extract_biome_assets.py <out_root> <pack.zip>...
"""
import json
import os
import sys
import zipfile

PACK_FORMAT = int(os.environ.get("CRUS_PACK_FORMAT", "81"))


def active_overlays(mcmeta_bytes):
    try:
        mc = json.loads(mcmeta_bytes.decode("utf-8"))
    except Exception:
        return []
    out = []
    entries = (mc.get("overlays") or {}).get("entries") or []
    for e in entries:
        lo = e.get("min_format", None)
        hi = e.get("max_format", None)
        fmt = e.get("formats")
        if fmt is not None:
            if isinstance(fmt, list) and len(fmt) == 2:
                lo, hi = fmt[0], fmt[1]
            elif isinstance(fmt, int):
                lo = hi = fmt
        if lo is None or hi is None:
            continue
        if lo <= PACK_FORMAT <= hi:
            d = e.get("directory")
            if d:
                out.append(d)
    return out


def pack_rel(rel):
    if not rel.startswith("data/") or not rel.endswith(".json"):
        return None
    r = rel[len("data/"):]
    keep = ("/worldgen/noise_settings/" in r
            or "/worldgen/density_function/" in r
            or "/worldgen/noise/" in r
            or r.endswith("/dimension/overworld.json"))
    return r if keep else None


def main():
    out_root = sys.argv[1]
    merged = {}
    for pack in sys.argv[2:]:
        with zipfile.ZipFile(pack) as zf:
            entries = sorted(zf.infolist(), key=lambda e: e.filename)
            mcmeta = None
            for e in entries:
                if e.filename == "pack.mcmeta":
                    mcmeta = zf.read(e)
                    break
            overlays = active_overlays(mcmeta) if mcmeta else []
            print(f"{os.path.basename(pack)}: overlays={overlays}")
            for e in entries:
                if e.is_dir():
                    continue
                rel = pack_rel(e.filename)
                if rel:
                    merged[rel] = zf.read(e)
            for ov in overlays:
                prefix = ov + "/data/"
                for e in entries:
                    if e.is_dir() or not e.filename.startswith(prefix):
                        continue
                    rel = pack_rel(e.filename[len(ov) + 1:])
                    if rel:
                        merged[rel] = zf.read(e)
    for rel, blob in merged.items():
        dst = os.path.join(out_root, "data", rel)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        with open(dst, "wb") as f:
            f.write(blob)
    print(f"total {len(merged)} worldgen jsons -> {out_root}")


if __name__ == "__main__":
    main()
