#!/usr/bin/env python3
"""Rebuild the effective datapack view (base+overlay per pack, packs in order)
and report the winning noise_settings.overworld: carvers, biome_source type,
surface_rule top-level nodes, sea_level. Prints which pack won each file."""
import json, zipfile, os, sys

ZIPS = ["/home/z/my-project/c-faster/server_datapacks/01_tectonic.zip",
        "/home/z/my-project/c-faster/server_datapacks/02_terralith.zip"]

def pack_view(zpath):
    """base + overlays: last matching overlay wins inside one pack; base first."""
    z = zipfile.ZipFile(zpath)
    names = [n for n in z.namelist() if n.endswith(".json") or n.endswith(".mcmeta")]
    mcmeta = json.loads(z.read("pack.mcmeta"))
    pf = mcmeta["pack"].get("pack_format") or mcmeta["pack"].get("pack_description", 0)
    overlays = mcmeta["pack"].get("overlays", {}).get("entries", [])
    view = {}
    # base layer
    for n in names:
        if n.startswith("overlay") or n.startswith("data") is False:
            pass
        if n == "pack.mcmeta":
            continue
        view[n] = z.read(n)
    # overlay layers in declared order
    for ov in overlays:
        d = ov.get("directory", "")
        fmt = ov.get("min_format", ov.get("min_format", 0))
        mx = ov.get("max_format", 999)
        # formats may be ints or [major,minor]
        def asint(v):
            if isinstance(v, list): return v[0]
            return v
        lo, hi = asint(fmt), asint(mx)
        if lo <= 81 <= hi:  # server pack format on paper 1.21.8
            for n in names:
                if n.startswith(d + "/"):
                    rel = n[len(d) + 1:]
                    view[rel] = z.read(n)
    return view

views = [pack_view(p) for p in ZIPS]
final = {}
for v in views:
    final.update(v)

ns = final.get("data/minecraft/worldgen/noise_settings/overworld.json")
if ns is None:
    print("NO overworld noise_settings in merged view!"); sys.exit(1)
d = json.loads(ns)

# provenance: which pack supplied the winning file
for name, vi in zip(["tectonic", "terralith"], views):
    print(f"{name}: has overworld.json = {'data/minecraft/worldgen/noise_settings/overworld.json' in vi}")

print("\n=== effective noise_settings.overworld ===")
print("sea_level:", d.get("sea_level"), " noise height:", d.get("noise", {}).get("min_y"), d.get("noise", {}).get("height"))
carvers = d.get("carvers")
print("carvers type:", type(carvers).__name__)
if isinstance(carvers, dict):
    for k, v in carvers.items():
        print("  ", k, "->", json.dumps(v)[:300])
bs = d.get("biome_source", {})
print("biome_source type:", bs.get("type"))
bl = bs.get("biomes", [])
print("biome_source biomes:", len(bl))
sr = d.get("surface_rule", {})
def count(node):
    if isinstance(node, dict):
        if "type" in node: total = 1
        else: total = 0
        for v in node.values(): total += count(v)
        return total
    if isinstance(node, list): return sum(count(x) for x in node)
    return 0
print("surface_rule root type:", sr.get("type"), " total nodes:", count(sr))
print("\ndefault_block:", d.get("default_block"), " default_fluid:", d.get("default_fluid"))
print("spawn_target count:", len(d.get("spawn_target", [])))
