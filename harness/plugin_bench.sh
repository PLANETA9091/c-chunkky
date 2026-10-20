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
# Judge region: MUST match the reference capture region (bench.sh, 8x8 = -4..3)
# EXACTLY. A larger pregen (the old -6..5 12x12) decorates the ring chunks
# whose features spill into the judged edge columns — overwrites the reference
# never had (the CI-only 0.556%: chunk_3_* = 1.4-1.9%). The spawn pregen sits
# around chunk (-13,0) — the 7x7 spawn prep (-16..-10) does not overlap -4..3.
X0="${CRUS_REGION_MIN:--4}"
X1=$((X0 + 7))
REGION_TAG="region\\[$X0\\.\\.$X1,$X0\\.\\.$X1\\]"

# generator wiring (bukkit.yml applies at world creation)
python3 - "$WORLD" <<'PY'
import sys
world = sys.argv[1]
content = f"worlds:\n  {world}:\n    generator: CrusstyGen\n"
open("server/bukkit.yml", "w").write(content)
print("server/bukkit.yml written:")
print(content)
PY

# --- boot -------------------------------------------------------------------
# world + seed (+ nether off: saves boot/pregen time, judge is overworld-only)
sed -i "s/^level-name=.*/level-name=$WORLD/; s/^level-seed=.*/level-seed=$SEED/; s/^allow-nether=.*/allow-nether=false/" "$SRV/server.properties"
grep -qE "^allow-nether=false" "$SRV/server.properties" || echo "allow-nether=false" >> "$SRV/server.properties"
grep -E "^(level-name|level-seed)=" "$SRV/server.properties"

if [ "${CRUS_FRESH:-0}" = "1" ]; then
  echo "--- wiping world dirs ($WORLD) for a deterministic run ---"
  rm -rf "$SRV/$WORLD" "$SRV/${WORLD}_nether" "$SRV/${WORLD}_the_end"
fi

# --- datapack wiring (world datapacks; applies at world load, wiped with CRUS_FRESH)
if [ "${DATAPACKS_DIR:-}" != "" ]; then
  mkdir -p "$SRV/$WORLD/datapacks"
  rm -rf "$SRV/$WORLD/datapacks"
  mkdir -p "$SRV/$WORLD/datapacks"
  cp -r "$DATAPACKS_DIR"/"." "$SRV/$WORLD/datapacks/"
  echo "datapacks wired from DATAPACKS_DIR=$DATAPACKS_DIR:"
  ls "$SRV/$WORLD/datapacks"
elif [ "$WORLD" = "datapack" ]; then
  mkdir -p "$SRV/$WORLD/datapacks"
  rm -rf "$SRV/$WORLD/datapacks/crussty-custom"
  cp -r "$ROOT/probe/datapacks/crussty-custom" "$SRV/$WORLD/datapacks/"
  echo "datapack wired: $SRV/$WORLD/datapacks/crussty-custom"
fi

# --- boot -------------------------------------------------------------------
# Judge determinism: the native noise stage finishes in ~3ms/chunk, which OPENS
# Paper's concurrency window — neighbour carvers (Java) run on other workers
# while this chunk decorates. The reference capture (bench.sh, vanilla-speed
# Java noise) never had overlapping carver/deco, so the contested 3x3 margin
# writes and the per-chunk feature RNG streams (a carver touching the same
# proto chunk flips canSurvive -> whole-chunk RNG cascade) resolve in forceload
# order there. Symptom: CI 4-core runner 0.556% (exploding +X columns, ore
# veins shifted) vs 2-core local 0.009% on the SAME code — commits e746d13+
# (native surface engaged -> fill went fast). gen-parallelism=false restores
# the reference semantics for the judged lane. CRUS_GEN_PARALLEL=1 opts out.
if [ "${CRUS_GEN_PARALLEL:-0}" != "1" ]; then
  sed -i "s/^  gen-parallelism: .*/  gen-parallelism: 'false'/" "$SRV/config/paper-global.yml"
  echo "gen-parallelism=false (judge serial semantics):"
  grep -A3 "^chunk-system:" "$SRV/config/paper-global.yml" | head -4
fi
# Parallel fill threads for the native .so (slice corners + cell strips):
# corners/strips are pure functions of position; results bit-identical.
export CRUS_FILL_THREADS="${CRUS_FILL_THREADS:-2}"
export CRUS_PROFILE="${CRUS_PROFILE:-1}"
# hard cleanup: a stale server (e.g. from the oracle step) holding RCON port
# 25575 silently steals judge rcon traffic — kill everything first.
bash harness/server_down.sh >/dev/null 2>&1 || true
pkill -9 -f 'java .*-jar paper\.jar' 2>/dev/null || true
sleep 3
bash harness/server_up.sh
SRVPID="$(cat "$SRV/server.pid")"
echo "=== server pid check: crusstatus must report srvpid=$SRVPID ==="
rcon "crusstatus" | head -4 || true

# --- JIT warmup pass (noise control) ----------------------------------------
# The shared 2-core runner makes a COLD measurement window unreproducible:
# the first ~16 of 64 chunks include C1/C2 compilation of the Java hot paths
# (bulk layout packer, JNI glue), shifting p50 by up to 2x run-to-run and
# inflating the mean with 100ms+ compile pauses. Pregen an 8x8 region OUTSIDE
# the measured one first (identical code paths — same world/seed/datapack),
# unload it, then reset the stats ring — the measured window below is
# steady-state, which is the honest per-chunk cost.
# CRUS_WARMUP=0 skips it entirely (parity probe runs: warmup is dead weight
# that pushes the phase past the sandbox's 10-min tool timeout).
if [ "${CRUS_WARMUP:-1}" != "0" ]; then
echo "=== JIT warmup region (chunks 8..15, 8..15) — discarded ==="
rcon "forceload add 128 128 255 255" >/dev/null || true
NWARM=0
for i in $(seq 1 90); do
  NWARM=$(rcon "crusgenstats" | grep -m1 -o "chunks=[0-9]*" | grep -oE "[0-9]+" || true)
  if [ "${NWARM:-0}" -ge 64 ]; then break; fi
  sleep 1
done
echo "=== warmup done: chunks=${NWARM:-?}/64 ==="
rcon "forceload remove all" >/dev/null || true
sleep 2
rcon "crusgenstats reset" >/dev/null || true
fi

# --- pregen + measure -------------------------------------------------------
# Judge determinism (post-gen simulation): kelp growth, amethyst buds and
# crop/cave growth are RANDOM ticks — which chunks get ticked depends on the
# world random draw sequence, which depends on WHEN chunks loaded — the fast
# native lane loads them in a different pattern than the slow reference
# capture. CI showed 0.556% (kelp +1-2 blocks, fresh small_amethyst_buds)
# where the reference had none. randomTickSpeed=0 freezes ALL random ticks on
# BOTH lanes (the reference captures run the same gamerule — bench.sh).
# The remaining scheduled ticks (fluid spreads, bubble columns) are
# tick-deterministic: the settle waits TICKS (300 = the capture's 15s at
# TPS 20), not wall seconds — the CI wall-clock drift made the lanes compare
# different simulation depths.
rcon "gamerule randomTickSpeed 0" >/dev/null || true
echo "gamerule randomTickSpeed=0 (judge freeze of random ticks)"
# Sequential per-chunk forceload (cx asc, cz asc): ore/blob features write into
# the 3x3 WorldGenRegion, so the final block at contested boundary positions
# depends on the chunk DECORATION ORDER. The reference capture (bench.sh) uses
# the identical loop — both worlds decorate in the same order, making the
# spill zones deterministic for the judge.
T0=$(date +%s%N)
EXPECT=0
STALL=0
RETRY_MAX=4
for cz in $(seq $X0 $X1); do
  for cx in $(seq $X0 $X1); do
    EXPECT=$((EXPECT + 1))
    loaded_ok=0
    for attempt in $(seq 1 $RETRY_MAX); do
      rcon "forceload add $((cx * 16)) $((cz * 16)) $((cx * 16 + 15)) $((cz * 16 + 15))" >/dev/null || true
      for i in $(seq 1 30); do
        # parse the counter AFTER '=': the naive [0-9]* match returned "4"
        # (the first number inside "[-4..3,-4..3]") — the loop then waited
        # forever on chunk #5. [0-9]+$ grabs the trailing counter only.
        LOADED=$(rcon "crusstatus $X0 $X0 $X1 $X1" | grep -m1 -o "${REGION_TAG}=[0-9]*" | grep -oE "[0-9]+$" || true)
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

# settle: TICK-matched (300 game ticks = the capture's 15s at TPS 20) — the
# scheduled fluid/growth ticks fire in tick order, so equal tick counts mean
# equal simulation depth regardless of the runner's wall-clock speed.
SETTLE_T0=$(rcon "crustick" | grep -m1 -oE "gametime=[0-9]+" | cut -d= -f2)
echo "settle: tick-matched 300 from gametime=${SETTLE_T0:-?}"
if [ -n "${SETTLE_T0:-}" ]; then
  for i in $(seq 1 90); do
    NOW=$(rcon "crustick" | grep -m1 -oE "gametime=[0-9]+" | cut -d= -f2)
    if [ -n "$NOW" ] && [ $((NOW - SETTLE_T0)) -ge 300 ]; then
      echo "settle: reached gametime=$NOW (+$((NOW - SETTLE_T0)) ticks)"
      break
    fi
    sleep 1
  done
fi
sleep 2

echo "=== crusgenstats ==="
rcon "crusgenstats" | tee "reference/plugin_stats_$WORLD.txt" || true

# --- dump plugin world ------------------------------------------------------
mkdir -p "$OUT_DIR"
rcon "crusstydumpregion $X0 $X0 $X1 $X1 $OUT_DIR chunk" | tail -2
ls "$OUT_DIR" | wc -l

# --- shutdown ---------------------------------------------------------------
bash harness/server_down.sh || true

# --- parity judge -----------------------------------------------------------
if [ "${CRUS_SKIP_DIFF:-0}" != "1" ]; then
  echo "=== parity: plugin world vs $REF_DIR ==="
  python3 harness/diff_dumps.py "$REF_DIR" "$OUT_DIR"
fi
