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
# Deploy hygiene: remove EVERY foreign plugin first. The oracle marks step
# leaves CrusstyOracle.jar behind; it boots in the judge server and its
# onEnable generates chunks out of order -> decoration streams shift ->
# ~5900 phantom diffs. The judge must run EXACTLY Gen + Probe.
find "$SRV/plugins" -maxdepth 1 -name '*.jar' ! -name 'CrusstyProbe.jar' -delete
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
# Parallel fill threads for the native .so (slice corners + cell strips):
# corners/strips are pure functions of position; results bit-identical.
export CRUS_FILL_THREADS="${CRUS_FILL_THREADS:-1}"
export CRUS_PROFILE="${CRUS_PROFILE:-1}"
# hard cleanup: a stale server (e.g. from the oracle step) holding RCON port
# 25575 silently steals judge rcon traffic — kill everything first.
bash harness/server_down.sh >/dev/null 2>&1 || true
pkill -9 -f 'paper\.jar' 2>/dev/null || true
sleep 3
bash harness/server_up.sh
SRVPID="$(cat "$SRV/server.pid")"
echo "=== server pid check: crusstatus must report srvpid=$SRVPID ==="
rcon "crusstatus" | head -4 || true

# --- pregen + measure -------------------------------------------------------
# Sequential per-chunk forceload (cx asc, cz asc): ore/blob features write into
# the 3x3 WorldGenRegion, so the final block at contested boundary positions
# depends on the chunk DECORATION ORDER. The reference capture (bench.sh) uses
# the identical loop — both worlds decorate in the same order, making the
# spill zones deterministic for the judge.
T0=$(date +%s%N)
EXPECT=0
STALL=0
RETRY_MAX=4
for cz in $(seq -4 3); do
  for cx in $(seq -4 3); do
    EXPECT=$((EXPECT + 1))
    loaded_ok=0
    for attempt in $(seq 1 $RETRY_MAX); do
      rcon "forceload add $((cx * 16)) $((cz * 16)) $((cx * 16 + 15)) $((cz * 16 + 15))" >/dev/null || true
      for i in $(seq 1 30); do
        # parse the counter AFTER '=': the naive [0-9]* match returned "4"
        # (the first number inside "[-4..3,-4..3]") — the loop then waited
        # forever on chunk #5. [0-9]+$ grabs the trailing counter only.
        LOADED=$(rcon "crusstatus" | grep -m1 -o "region\[-4\.\.3,-4\.\.3\]=[0-9]*" | grep -oE "[0-9]+$" || true)
        if [ "${LOADED:-0}" -ge "$EXPECT" ]; then loaded_ok=1; STALL=0; break; fi
        if [ "$((i % 15))" -eq 0 ]; then
          echo "[pregen stall] chunk $cx $cz (attempt $attempt): wait ${i}s, loaded=${LOADED:-?}/$EXPECT"
          tail -n 4 "$SRV/run.out" 2>/dev/null || true
        fi
        sleep 1
      done
      [ "$loaded_ok" -eq 1 ] && break
      # Paper's chunk scheduler can lose the generation task when generation
      # is extremely fast (native fill ~16ms): the ticket exists but no worker
      # is scheduled (jstack: all Worker-Main idle). A repeated forceload add
      # re-issues the ticket — idempotent, and the vanilla reference capture
      # (bench.sh, slow path) never hits this, so parity is unaffected.
      echo "[pregen RETRY] chunk $cx $cz did not load in 30s (attempt $attempt/$RETRY_MAX) — re-issuing forceload"
    done
    if [ "$loaded_ok" -ne 1 ]; then
      STALL=$((STALL + 1))
      echo "[pregen STALL] chunk $cx $cz did not load after $RETRY_MAX attempts (loaded=${LOADED:-?}/$EXPECT, stalls=$STALL)"
      if [ "$STALL" -ge 3 ]; then
        echo "=== FATAL: server not loading chunks — full jstack ==="
        JP=$(pgrep -f 'paper.jar' | head -1)
        [ -n "$JP" ] && (timeout 30 "${JAVA_HOME:-}/bin/jstack" "$JP" 2>/dev/null || jstack "$JP" 2>/dev/null || true) | head -400
        echo "=== FATAL: last 120 lines of run.out ==="
        tail -n 120 "$SRV/run.out" 2>/dev/null || true
        exit 9
      fi
    fi
  done
done
T1=$(date +%s%N)
WALL_MS=$(( (T1 - T0) / 1000000 ))
echo "=== pregen wall: ${WALL_MS}ms (sequential forceload, incl. poll granularity) ==="

# settle: match bench.sh (reference capture, 15s) — in-world fluid ticks (bubble
# column growth, spring spreads) need equal wall time on both sides or the
# judge compares a mid-tick world against a settled reference.
sleep 15

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
