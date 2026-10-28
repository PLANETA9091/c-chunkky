#!/usr/bin/env python3
"""Generate native/src/features_vanilla_data263.rs from reference/vanilla263.

26.3 feature JSONs are FLAT: {"type": "<t>", <config fields inline>}.
The legacy features_json.rs parser expects the 1.21.8 shape
{"type": "<t>", "config": {...}}. This script wraps every non-"type"
top-level field back into "config" so the existing parser consumes the
26.3 vanilla set unchanged (schema drift inside configs is measured by
the unsupported counters, not silently dropped).

placed_feature JSONs are unchanged between versions -> passed through.

Regenerate after re-extracting reference/vanilla263:
    python3 scripts/extract_features263.py
"""
import json
import glob
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "reference", "vanilla263", "data", "minecraft", "worldgen")
OUT = os.path.join(ROOT, "native", "src", "features_vanilla_data263.rs")


def rs_str(s: str) -> str:
    """Escape a JSON string for a Rust string literal (like the 1.21.8 file)."""
    return s.replace("\\", "\\\\").replace('"', '\\"')


def load_dir(sub: str):
    entries = []
    for path in sorted(glob.glob(os.path.join(SRC, sub, "*.json"))):
        name = "minecraft:" + os.path.basename(path)[: -len(".json")]
        with open(path, "r", encoding="utf-8") as f:
            d = json.load(f)
        entries.append((name, d))
    return entries


def normalize_configured(name: str, d: dict) -> str:
    """Flat 26.3 feature JSON -> legacy {"type","config"} shape."""
    if "type" not in d:
        raise ValueError(f"{name}: no 'type' field (not a flat 26.3 feature?)")
    config = {k: v for k, v in d.items() if k != "type"}
    return json.dumps({"type": d["type"], "config": config},
                      separators=(",", ":"), sort_keys=False)


def normalize_placed(name: str, d: dict) -> str:
    # reject inline feature objects loudly (audit says vanilla has none)
    if not isinstance(d.get("feature"), str):
        raise ValueError(f"{name}: inline/missing feature ref — extend script")
    return json.dumps(d, separators=(",", ":"), sort_keys=False)


def emit_const(out, cname: str, entries, comment: str):
    out.append(f"// {comment}\n")
    out.append(f"pub const {cname}: &[(&str, &str)] = &[\n")
    for name, text in entries:
        out.append(f'    ("{rs_str(name)}", "{rs_str(text)}"),\n')
    out.append("];\n\n")


def main() -> None:
    configured = load_dir("feature")
    placed = load_dir("placed_feature")
    biomes = load_dir("biome")

    cfg_norm = [(n, normalize_configured(n, d)) for n, d in configured]
    pl_norm = [(n, normalize_placed(n, d)) for n, d in placed]
    # biome JSONs pass through unchanged: build_deco_ctx reads the top-level
    # "features" array (steps of placed-feature refs), shape is stable 26.3.
    bio_norm = [(n, json.dumps(d, separators=(",", ":"), sort_keys=False))
                for n, d in biomes]

    lines = []
    lines.append("//! VANILLA 26.3 worldgen feature data (GENERATED — do not edit).\n")
    lines.append("//! Source: reference/vanilla263/data/minecraft/worldgen/{feature,placed_feature}.\n")
    lines.append("//! Flat 26.3 feature JSONs normalized to the legacy {type,config} shape\n")
    lines.append("//! via scripts/extract_features263.py (see header there).\n\n")

    emit_const(lines, "VANILLA_CONFIGURED_263", cfg_norm,
               f"vanilla 26.3 features ({len(cfg_norm)}), flat->config normalized")
    emit_const(lines, "VANILLA_PLACED_263", pl_norm,
               f"vanilla 26.3 placed features ({len(pl_norm)}), pass-through")
    emit_const(lines, "VANILLA_BIOMES_263", bio_norm,
               f"vanilla 26.3 biome JSONs ({len(bio_norm)}), features arrays intact")

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", encoding="utf-8") as f:
        f.write("".join(lines))
    print(f"wrote {OUT}: {len(cfg_norm)} configured, {len(pl_norm)} placed, "
          f"{len(bio_norm)} biomes")


if __name__ == "__main__":
    main()
