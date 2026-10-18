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

rcon() { python3 "$ROOT/harness/rcon.py" localhost 25575 crussty "$1"; }

clean_world() {
  rm -rf "$SRV/$WORLD" "$SRV/${WORLD}_nether" "$SRV/${WORLD}_the_end"
  rm -f "$SRV/plugins/CrusstyGen.jar"
}

set_props() {
  sed -i "s/^level-name=.*/level-name=$WORLD/; s/^level-seed=.*/level-seed=$SEED/; s/^allow-nether=.*/allow-nether=false/" \
    "$SRV/server.properties"
  grep -qE "^allow-nether=false" "$SRV/server.properties" || echo "allow-nether=false" >> "$SRV/server.properties"
}

wire_datapacks() {
  mkdir -p "$SRV/$WORLD/datapacks"
  rm -rf "$SRV/$WORLD/datapacks"
  mkdir -p "$SRV/$WORLD/datapacks"
  cp -r "$PACKS"/"." "$SRV/$WORLD/datapacks/"
  echo "[f1] datapacks wired:"; ls "$SRV/$WORLD/datapacks"
}

# ---------------------------------------------------------------------------
# Run A — vanilla reference (no generator wiring, no CrusstyGen.jar)
# ---------------------------------------------------------------------------
echo "=== [A] vanilla reference run ==="
clean_world
cat > "$SRV/bukkit.yml" << 'EOF'
worlds: {}
EOF
set_props
wire_datapacks

# bench.sh boots the server itself (and sets spawnChunkRadius 0 after boot)
pkill -9 -f 'paper\.jar' 2>/dev/null || true
sleep 3
CRUS_FRESH=0 bash harness/bench.sh "$WORLD" f1probe_vanilla
bash harness/server_down.sh >/dev/null 2>&1 || true

# ---------------------------------------------------------------------------
# Run B — native deco (CrusstyGen + CRUS_NATIVE_DECO=1)
# ---------------------------------------------------------------------------
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
export CRUS_FRESH=0 CRUS_SKIP_DIFF=0 CRUS_NATIVE_DECO=1
export DATAPACKS_DIR="$PACKS"
bash harness/plugin_bench.sh "$WORLD" "$ROOT/reference/dumps/f1probe_vanilla"
