#!/usr/bin/env python3
"""Download the Terralith + Tectonic worldgen datapacks from Modrinth.

Usage: python3 scripts/fetch_datapacks.py [outdir] [mc_version]

Files land as 01_tectonic.zip / 02_terralith.zip: world datapacks are enabled
in alphabetical order and LATER packs override earlier ones on conflicting
paths, so Terralith (02) wins the dimension/noise_settings conflicts. Tectonic
still contributes its non-conflicting worldgen data. This is a SPEED benchmark
setup (CrusstyGen replaces the noise stage with the vanilla-exact native one;
the datapacks stress biomes/features/structures), NOT a parity setup.

Stdlib only, no dependencies. Writes outdir/versions.txt for the report.
"""
import json
import pathlib
import sys
import urllib.request

UA = {"User-Agent": "crussty-bench/0.1 (PLANETA9091/c-faster CI)"}
PACKS = [
    ("tectonic", "01_tectonic.zip"),
    ("terralith", "02_terralith.zip"),
]
MIN_BYTES = 100_000  # sane lower bound for a worldgen datapack zip


def http_get(url: str) -> bytes:
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=120) as r:
        return r.read()


def versions(slug: str) -> list:
    return json.loads(http_get(f"https://api.modrinth.com/v2/project/{slug}/version"))


def pick(vs: list, mc: str) -> dict:
    """Newest version compatible with mc (exact match preferred, 1.21.* fallback)."""
    for v in vs:  # Modrinth returns newest-first
        gv = v.get("game_versions") or []
        if mc in gv:
            return v
    for v in vs:
        gv = v.get("game_versions") or []
        if any(g.startswith("1.21") for g in gv):
            return v
    raise SystemExit(f"ERROR: no {mc}/1.21.x compatible build for {slug}")


def main() -> int:
    outdir = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "server_datapacks")
    mc = sys.argv[2] if len(sys.argv) > 2 else "1.21.8"
    outdir.mkdir(parents=True, exist_ok=True)
    lines = []
    for slug, fname in PACKS:
        v = pick(versions(slug), mc)
        url = v["files"][0]["url"]
        dest = outdir / fname
        print(f"{slug}: version {v['version_number']} -> {dest}")
        data = http_get(url)
        if len(data) < MIN_BYTES:
            raise SystemExit(f"ERROR: {slug} download suspiciously small ({len(data)}B)")
        dest.write_bytes(data)
        print(f"  saved {len(data)} bytes")
        lines.append(f"{slug}\t{v['version_number']}\tmc={mc}\t{url}")
    (outdir / "versions.txt").write_text("\n".join(lines) + "\n")
    print("all datapacks fetched")
    return 0


if __name__ == "__main__":
    sys.exit(main())
