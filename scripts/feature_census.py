#!/usr/bin/env python3
"""Census of the effective dptt feature pipeline: biomes -> placed -> configured types."""
import json, zipfile, sys
from collections import Counter, defaultdict

ZIPS = ["/home/z/my-project/c-faster/server_datapacks/01_tectonic.zip",
        "/home/z/my-project/c-faster/server_datapacks/02_terralith.zip"]

def pack_view(zpath):
    z = zipfile.ZipFile(zpath)
    names = [n for n in z.namelist() if n.endswith(".json")]
    mcmeta = json.loads(z.read("pack.mcmeta"))
    view = {}
    for n in names:
        view[n] = z.read(n)
    for ov in mcmeta["pack"].get("overlays", {}).get("entries", []):
        d = ov.get("directory", "")
        def asint(v): return v[0] if isinstance(v, list) else v
        lo, hi = asint(ov.get("min_format", 0)), asint(ov.get("max_format", 999))
        if lo <= 81 <= hi:
            for n in names:
                if n.startswith(d + "/"):
                    view[n[len(d)+1:]] = z.read(n)
    return view

views = [pack_view(p) for p in ZIPS]
final = {}
for v in views: final.update(v)

def load(key):
    b = final.get(key)
    return json.loads(b) if b else None

# world preset -> biome source list
wp = load("data/minecraft/worldgen/world_preset/normal.json")
src = None
if wp and "overworld" in wp.get("level_stem", {}):
    gen = wp["level_stem"]["overworld"]["generator"]
    src = gen.get("biome_source")
biomes_list = src.get("biomes", []) if src else []
print("biome_source:", src.get("type") if src else None, " entries:", len(biomes_list))

biome_ids = [b["biome"] for b in biomes_list]
feat_counter = Counter()          # configured feature type histogram
feat_by_type = defaultdict(set)   # type -> set of configured feature ids
placed_count = 0
missing_placed, missing_conf = [], []

for bid in biome_ids:
    key = f"data/{bid}/worldgen/biome.json"
    bj = load(key)
    if bj is None:
        continue
    for step in bj.get("features", []):
        for ref in step:
            pf = ref[1:] if ref.startswith("#") else ref
            pkey = f"data/{pf}/worldgen/placed_feature.json"
            pv = load(pkey)
            if pv is None:
                missing_placed.append(pf); continue
            placed_count += 1
            cfid = pv.get("feature")
            ckey = f"data/{cfid}/worldgen/configured_feature.json"
            cv = load(ckey)
            if cv is None:
                missing_conf.append(cfid); continue
            t = cv.get("type", "?").replace("minecraft:", "")
            feat_counter[t] += 1
            feat_by_type[t].add(cfid)

print("unique placed refs:", placed_count, " (unique: ", len(set(ref for b in biome_ids for s in (load(f'data/{b}/worldgen/biome.json') or {}).get('features', []) for ref in s)), ")")
print("\n=== configured feature type histogram (refs incl. repeats) ===")
for t, c in feat_counter.most_common():
    print(f"  {t:28s} {c:5d}   unique={len(feat_by_type[t])}")
print("\nmissing placed:", len(missing_placed), " missing configured:", len(missing_conf))
for t, ids in sorted(feat_by_type.items(), key=lambda kv: -len(kv[1]))[:12]:
    ex = sorted(ids)[:3]
    print(f"  {t:24s} unique={len(ids):4d} e.g. {[i.replace('minecraft:','m:').replace('terralith:','t:').replace('tectonic:','tc:') for i in ex]}")
