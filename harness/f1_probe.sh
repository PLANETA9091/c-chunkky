#!/usr/bin/env bash
# F1 probe: native features-engine bit-parity check.
#   Run A (reference): vanilla generator, probe pack, dump -> reference/dumps/f1probe_vanilla
#   Run B (native):    CrusstyGen + CRUS_NATIVE_DECO=1, same pack/seed,
#                      dump -> reference/dumps_plugin/f1probe, diff vs A.
# Criterion: 0 divergent blocks (probe pack has no carvers/structures/fluid
# re-ticks; both runs decorate in identical chunk order).
#
# Env: CRUS_KEEP=1 keep server worlds between phases (default wipes each run).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
WORLD=f1probe
SEED=3257840388504953787
PACKS="$ROOT/probe/datapacks/f1"
SRV="server"

# Measured region base chunk. The harness forceloads a 12x12 region (base..
# base+7) and compares ONLY the inner 8x8 (base+2..base+9): every chunk whose
# features can spill into the compared area is then REQUESTED IN ORDER with
# per-chunk waits, so the decoration order is strict forceload order on BOTH
# lanes. Margin chunks outside the 12x12 never run features (ring promotion
# stops at CARVERS). The old 8x8-forceload layout left ring-1 chunks to the
# scheduler, whose margin-vs-center order flips between the reference (vanilla
# deco) and native (JNI deco) lanes because the deco step durations differ.
export CRUS_REGION_MIN="${CRUS_REGION_MIN:-98}"
INNER_LO=$((CRUS_REGION_MIN + 2))
INNER_HI=$((CRUS_REGION_MIN + 9))

rcon() { python3 "$ROOT/harness/rcon.py" localhost 25575 crussty "$1"; }

clean_world() {
  rm -rf "$SRV/$WORLD" "$SRV/${WORLD}_nether" "$SRV/${WORLD}_the_end"
  rm -f "$SRV/plugins/CrusstyGen.jar"
}

set_props() {
  sed -i "s/^level-name=.*/level-name=$WORLD/; s/^level-seed=.*/level-seed=$SEED/; s/^allow-nether=.*/allow-nether=false/" \
    "$SRV/server.properties"
  grep -qE "^allow-nether=false" "$SRV/server.properties" || echo "allow-nether=false" >> "$SRV/server.properties"
  # spawn-chunk pregen at boot decorates chunks (-2..2) BEFORE the forceload
  # loop, and the deco duration differs between the reference (vanilla deco)
  # and native lanes -> the boot-time cascade order can flip contested
  # boundary blocks. Kill the boot pregen entirely: every measured chunk then
  # decorates in strict forceload order on BOTH lanes.
  sed -i "s/^spawn-chunk-radius=.*/spawn-chunk-radius=0/" "$SRV/server.properties"
  grep -qE "^spawn-chunk-radius=0" "$SRV/server.properties" || echo "spawn-chunk-radius=0" >> "$SRV/server.properties"
}

wire_datapacks() {
  mkdir -p "$SRV/$WORLD/datapacks"
  rm -rf "$SRV/$WORLD/datapacks"
  mkdir -p "$SRV/$WORLD/datapacks"
  cp -r "$PACKS"/"." "$SRV/$WORLD/datapacks/"
  echo "[f1] datapacks wired:"; ls "$SRV/$WORLD/datapacks"
}

# ---------------------------------------------------------------------------
# Run A — native-substrate reference: CrusstyGen noise/surface/biomes,
# VANILLA feature engine (CRUS_NATIVE_DECO unset). Same harness path as Run B
# so the chunk-scheduler timing is identical: the deco task dispatch order
# depends on the noise-stage speed, and a vanilla-noise reference lane
# reorders margin-vs-center features vs the fast native lane (observed as
# ore veins contesting across the forceload border). Substrate parity itself
# is covered by the dptt judge; this probe isolates the FEATURES engine.
# ---------------------------------------------------------------------------
# CRUS_PHASE=a|b|all: the sandbox reaps background processes between tool
# calls, so the harness splits into two foreground phases.
if [ "${CRUS_PHASE:-all}" != "b" ]; then
echo "=== [A] native-substrate + vanilla deco reference run ==="
clean_world
cat > "$SRV/bukkit.yml" << 'EOF'
worlds:
  f1probe:
    generator: CrusstyGen
EOF
set_props
wire_datapacks
cp "$ROOT/plugin/CrusstyGen.jar" "$SRV/plugins/CrusstyGen.jar"

# plugin_bench.sh does boot + pregen + dump (skip diff; dumps moved below).
# CRUS_NATIVE_DECO deliberately NOT set here.
pkill -9 -f 'java .*-jar paper\.jar' 2>/dev/null || true
sleep 3
CRUS_FRESH=0 CRUS_SKIP_DIFF=1 CRUS_WARMUP=0 bash harness/plugin_bench.sh "$WORLD" "$ROOT/reference/dumps_plugin/$WORLD"
bash harness/server_down.sh >/dev/null 2>&1 || true

# Run A dumps become the reference set for Run B's diff. Keep ONLY the inner
# 8x8 (the outer 2-ring exists to pin the decoration order; its own margins
# are scheduler-raced and NOT comparable).
rm -rf "$ROOT/reference/dumps/f1probe_vanilla"
mv "$ROOT/reference/dumps_plugin/$WORLD" "$ROOT/reference/dumps/f1probe_vanilla"
python3 - "$ROOT/reference/dumps/f1probe_vanilla" "$INNER_LO" "$INNER_HI" <<'PY'
import sys, os
from pathlib import Path
d, lo, hi = Path(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
kept = 0
for p in list(d.glob("chunk_*.crsf")):
    try:
        _, cx, cz = p.stem.split("_")
        cx, cz = int(cx), int(cz)
    except ValueError:
        continue
    if not (lo <= cx <= hi and lo <= cz <= hi):
        p.unlink()
    else:
        kept += 1
print(f"[f1] inner 8x8 reference: {kept} chunks ({lo}..{hi})")
PY
fi

# ---------------------------------------------------------------------------
# Run B — native deco (CrusstyGen + CRUS_NATIVE_DECO=1)
# ---------------------------------------------------------------------------
if [ "${CRUS_PHASE:-all}" != "a" ]; then
echo "=== [B] native deco run ==="
clean_world
cat > "$SRV/bukkit.yml" << 'EOF'
worlds:
  f1probe:
    generator: CrusstyGen
EOF
set_props
wire_datapacks
cp "$ROOT/plugin/CrusstyGen.jar" "$SRV/plugins/CrusstyGen.jar"

# plugin_bench.sh does boot + pregen + dump + diff (REF_DIR = run A dumps)
export CRUS_FRESH=0 CRUS_SKIP_DIFF=0 CRUS_NATIVE_DECO=1 CRUS_WARMUP=0
export DATAPACKS_DIR="$PACKS"
bash harness/plugin_bench.sh "$WORLD" "$ROOT/reference/dumps/f1probe_vanilla"
fi
