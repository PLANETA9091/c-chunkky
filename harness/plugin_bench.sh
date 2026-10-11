#!/usr/bin/env bash
# Integration benchmark: boot Paper with the CrusstyGen plugin (native noise
# stage), pregen the frozen region, collect stats, dump the plugin world, diff
# against the vanilla reference dumps (parity judge).
#
# Usage: harness/plugin_bench.sh <world> [diff_against_dir]
#   world            : vanilla | datapack (server.properties level-name is set)
#   diff_against_dir : default reference/dumps/<world>
#
# Env:
#   CRUS_FRESH=1  wipe the world dirs first (deterministic parity runs)
#   CRUS_SKIP_DIFF=1  skip the parity diff step
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
WORLD="${1:?usage: plugin_bench.sh <world> [diff_against_dir]}"
REF_DIR="${2:-reference/dumps/$WORLD}"
OUT_DIR="$ROOT/reference/dumps_plugin/$WORLD"
SRV="server"
HOST=127.0.0.1
PORT=25575
PASS=crussty
SEED=3257840388504953787

SO="native/target/release/libcrussty_chunkgen.so"
JAR="plugin/CrusstyGen.jar"
PROBE_JAR="probe/CrusstyProbe.jar"
[ -f "$SRV/paper.jar" ] || { echo "no server/paper.jar" >&2; exit 1; }
[ -f "$SO" ] || { echo "no $SO — build native first (cargo build --release in native/)" >&2; exit 1; }
[ -f "$JAR" ] || { echo "no $JAR — run plugin/build.sh" >&2; exit 1; }

rcon() {
  local tries=0
  while [ "$tries" -lt 30 ]; do
    if python3 harness/rcon.py "$HOST" "$PORT" "$PASS" "$1"; then return 0; fi
    tries=$((tries + 1))
    sleep 2
  done
  echo "rcon: giving up after retries: $1" >&2
  return 3
}

# --- deploy -----------------------------------------------------------------
mkdir -p "$SRV/plugins"
cp "$JAR" "$SRV/plugins/CrusstyGen.jar"
[ -f "$PROBE_JAR" ] && cp "$PROBE_JAR" "$SRV/plugins/CrusstyProbe.jar"
{
  echo "# deployed.md5 $(date -u +%Y-%m-%dT%H:%M:%SZ) (plugin_bench)"
  md5sum "$SRV/paper.jar" "$SRV/plugins/CrusstyGen.jar" "$SO"
} > "$SRV/deployed.md5"
echo "--- deployed.md5 ---"; cat "$SRV/deployed.md5"

# --- config -----------------------------------------------------------------
# generator wiring (bukkit.yml applies at world creation)
python3 - "$WORLD" <<'PY'
import sys
world = sys.argv[1]
content = f"worlds:\n  {world}:\n    generator: CrusstyGen\n"
open("server/bukkit.yml", "w").write(content)
print("server/bukkit.yml written:")
print(content)
PY
# world + seed (+ nether off: saves boot/pregen time, judge is overworld-only)
sed -i "s/^level-name=.*/level-name=$WORLD/; s/^level-seed=.*/level-seed=$SEED/; s/^allow-nether=.*/allow-nether=false/" "$SRV/server.properties"
grep -qE "^allow-nether=false" "$SRV/server.properties" || echo "allow-nether=false" >> "$SRV/server.properties"
grep -E "^(level-name|level-seed)=" "$SRV/server.properties"

if [ "${CRUS_FRESH:-0}" = "1" ]; then
  echo "--- wiping world dirs ($WORLD) for a deterministic run ---"
  rm -rf "$SRV/$WORLD" "$SRV/${WORLD}_nether" "$SRV/${WORLD}_the_end"
fi

# --- datapack wiring (world datapacks; applies at world load, wiped with CRUS_FRESH)
if [ "$WORLD" = "datapack" ]; then
  mkdir -p "$SRV/$WORLD/datapacks"
  rm -rf "$SRV/$WORLD/datapacks/crussty-custom"
  cp -r "$ROOT/probe/datapacks/crussty-custom" "$SRV/$WORLD/datapacks/"
  echo "datapack wired: $SRV/$WORLD/datapacks/crussty-custom"
fi

# --- boot -------------------------------------------------------------------
bash harness/server_up.sh

# --- pregen + measure -------------------------------------------------------
T0=$(date +%s%N)
rcon "forceload add -64 -64 63 63" >/dev/null
for i in $(seq 1 900); do
  LOADED=$(rcon "crusstatus" | grep -m1 "vanilla:.*region\[-4\.\.3,-4\.\.3\]=[0-9]*/64" | grep -o "region\[-4\.\.3,-4\.\.3\]=[0-9]*/64" | head -1 || true)
  if [ "$LOADED" = "region[-4..3,-4..3]=64/64" ]; then break; fi
  sleep 2
done
T1=$(date +%s%N)
WALL_MS=$(( (T1 - T0) / 1000000 ))
echo "=== pregen wall: ${WALL_MS}ms (includes settle granularity) ==="

# settle: match bench.sh (reference capture) — in-world fluid ticks (bubble
# column growth, spring spreads) need equal wall time on both sides or the
# judge compares a mid-tick world against a settled reference.
sleep 5

echo "=== crusgenstats ==="
rcon "crusgenstats" | tee "reference/plugin_stats_$WORLD.txt" || true

# --- dump plugin world ------------------------------------------------------
mkdir -p "$OUT_DIR"
rcon "crusstydumpregion -4 -4 3 3 $OUT_DIR chunk" | tail -2
ls "$OUT_DIR" | wc -l

# --- shutdown ---------------------------------------------------------------
bash harness/server_down.sh || true

# --- parity judge -----------------------------------------------------------
if [ "${CRUS_SKIP_DIFF:-0}" != "1" ]; then
  echo "=== parity: plugin world vs $REF_DIR ==="
  python3 harness/diff_dumps.py "$REF_DIR" "$OUT_DIR"
fi
