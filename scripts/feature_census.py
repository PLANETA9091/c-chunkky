#!/usr/bin/env python3
"""Census of the effective dptt feature pipeline: biomes -> placed -> configured types.

The overworld dimension override (data/minecraft/dimension/overworld.json,
Tectonic) carries the generator: biome_source multi_noise (1713 entries) +
settings=minecraft:overworld. Biome JSONs live at data/<ns>/worldgen/biome/<name>.json."""
import json, zipfile
from collections import Counter, defaultdict

ROOT = "/home/z/my-project/c-faster"
ZIPS = [ROOT + "/server_datapacks/01_tectonic.zip",
        ROOT + "/server_datapacks/02_terralith.zip"]

def pack_view(zpath):
    z = zipfile.ZipFile(zpath)
    names = [n for n in z.namelist() if n.endswith(".json")]
    view = {n: z.read(n) for n in names}
    mc = json.loads(z.read("pack.mcmeta"))
    for ov in mc["pack"].get("overlays", {}).get("entries", []):
        d = ov.get("directory", "")
        def ai(v): return v[0] if isinstance(v, list) else v
        if ai(ov.get("min_format", 0)) <= 81 <= ai(ov.get("max_format", 999)):
            for n in names:
                if n.startswith(d + "/"):
                    view[n[len(d)+1:]] = z.read(n)
    return view

final = {}
for p in ZIPS: final.update(pack_view(p))

def load(k):
    b = final.get(k)
    return json.loads(b) if b else None

dim = load("data/minecraft/dimension/overworld.json")
src = dim["generator"]["biome_source"]
biomes_list = src.get("biomes", [])
feat_counter = Counter()
feat_by_type = defaultdict(set)
placed = set()
for b in biomes_list:
    bid = b["biome"] if isinstance(b["biome"], str) else b["biome"][0]
    ns, name = bid.split(":")
    bj = load(f"data/{ns}/worldgen/biome/{name}.json")
    if bj is None: continue
    for step in bj.get("features", []):
        for ref in step:
            pf = ref[1:] if ref.startswith("#") else ref
            if not isinstance(pf, str): continue
            placed.add(pf)
            ns2, name2 = pf.split(":")
            pv = load(f"data/{ns2}/worldgen/placed_feature/{name2}.json")
            if pv is None: continue
            cfid = pv.get("feature")
            if not isinstance(cfid, str): continue
            ns3, name3 = cfid.split(":")
            cv = load(f"data/{ns3}/worldgen/configured_feature/{name3}.json")
            if cv is None: continue
            t = cv.get("type", "?").replace("minecraft:", "")
            feat_counter[t] += 1
            feat_by_type[t].add(cfid)

print("biomes:", len(biomes_list), " unique placed refs:", len(placed))
print("=== configured feature type histogram (biome-step refs) ===")
for t, c in feat_counter.most_common():
    ex = sorted(feat_by_type[t])[:2]
    print(f"  {t:24s} refs={c:5d}  unique={len(feat_by_type[t]):4d}  e.g. {[i.split(':')[1][:24] for i in ex]}")
