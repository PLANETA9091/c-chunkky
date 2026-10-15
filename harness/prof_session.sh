#!/usr/bin/env bash
# prof_session.sh <world> <tag> [datapacks_dir]
#
# Profiling session (steady-state, fresh region, spark around the window):
#   1. wipe <world> (fresh), copy *.zip from [datapacks_dir] into world
#      datapacks; wire bukkit.yml generator ONLY when datapacks_dir given
#      (vanilla control runs without the entry -> Paper default generator)
#   2. boot (CRUS_ALLOW_PARALLEL=1, CRUS_FILL_THREADS=2 — bench5k conditions)
#   3. JIT warmup: 8x8 fresh region cx 20..27 / cz 20..27, unload, reset stats
#   4. spark profiler start
#   5. measured window: 12x12 fresh region cx 32..43 / cz 32..43, 4 wave
#      batches of 3 chunk-rows, per-batch /crusfull completion (reissue/force)
#   6. spark profiler stop; final crusgenstats; spark link from console
#
# Outputs: reference/prof_session_<tag>.txt (batch rates + stats + spark link)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

WORLD="${1:?usage: prof_session.sh <world> <tag> [datapacks_dir]}"
TAG="${2:?usage: prof_session.sh <world> <tag> [datapacks_dir]}"
DPDIR="${3:-}"
SRV=server
HOST=127.0.0.1 PORT=25575 PASS=crussty
SEED=3257840388504953787
REPORT="reference/prof_session_${TAG}.txt"

export CRUS_FILL_THREADS="${CRUS_FILL_THREADS:-2}"
export CRUS_ALLOW_PARALLEL=1
export CRUS_HEAP="${CRUS_HEAP:--Xms1G -Xmx2G}"

WARM_CXMIN=20 WARM_CXMAX=27 WARM_CZMIN=20 WARM_CZMAX=27
MEAS_CXMIN=32 MEAS_CXMAX=43 MEAS_CZMIN=32 MEAS_CZMAX=43
ROWS_BATCH=3

rcon() {
  local tries=0
  while [ "$tries" -lt 20 ]; do
    if python3 harness/rcon.py "$HOST" "$PORT" "$PASS" "$1" 2>/dev/null; then return 0; fi
    tries=$((tries + 1)); sleep 2
  done
  echo "rcon FAILED: $1" >&2; return 3
}

blockof() { echo $(( $1 * 16 )); }

# --- fresh world ------------------------------------------------------------
bash harness/server_down.sh >/dev/null 2>&1 || true
pkill -9 -f 'paper\.jar' 2>/dev/null || true
sleep 3
rm -rf "$SRV/$WORLD" "$SRV/${WORLD}_nether" "$SRV/${WORLD}_the_end"
if [ -n "$DPDIR" ]; then
  mkdir -p "$SRV/$WORLD/datapacks"
  for z in "$DPDIR"/*.zip; do [ -e "$z" ] && cp "$z" "$SRV/$WORLD/datapacks/"; done
  ls "$SRV/$WORLD/datapacks/"
  printf 'worlds:\n  %s:\n    generator: CrusstyGen\n' "$WORLD" > "$SRV/bukkit.yml"
else
  printf 'worlds: {}\n' > "$SRV/bukkit.yml"
fi
sed -i "s/^level-name=.*/level-name=$WORLD/; s/^level-seed=.*/level-seed=$SEED/; s/^allow-nether=.*/allow-nether=false/" "$SRV/server.properties"
grep -qE "^allow-nether=false" "$SRV/server.properties" || echo "allow-nether=false" >> "$SRV/server.properties"

# --- deploy: only Gen + Probe + spark (oracle hygiene lesson) ----------------
mkdir -p "$SRV/plugins"
find "$SRV/plugins" -maxdepth 1 -name '*.jar' ! -name 'CrusstyProbe.jar' ! -name 'spark*.jar' -delete
cp plugin/CrusstyGen.jar "$SRV/plugins/CrusstyGen.jar"
SO="native/target/release/libcrussty_chunkgen.so"
[ -f "$SO" ] || { echo "no $SO" >&2; exit 1; }

{
  echo "# prof session $TAG $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "world=$WORLD datapacks_dir=${DPDIR:-none} fill_threads=$CRUS_FILL_THREADS parallel=1 heap=$CRUS_HEAP"
  [ -n "$DPDIR" ] && (cd "$DPDIR" && md5sum *.zip)
  md5sum "$SRV/paper.jar" "$SRV/plugins/CrusstyGen.jar" "$SO"
} > "$REPORT"
cat "$REPORT"

# --- boot --------------------------------------------------------------------
bash harness/server_up.sh
rcon "gamerule spawnChunkRadius 0" >/dev/null 2>&1 || true

wait_full() { # wait_full <x1> <z1> <x2> <z2> <expect> [reissue_sec force_sec]
  local x1="$1" z1="$2" x2="$3" z2="$4" expect="$5"
  local reissue="${6:-25}" force="${7:-70}" t0 n
  t0=$(date +%s)
  while :; do
    # rcon can print the response AND exit non-zero under load (pipefail would
    # then append the ||-fallback) — sanitize to the last pure-integer token.
    n=$(rcon "crusfull $x1 $z1 $x2 $z2" 2>/dev/null | grep -m1 -oE 'FULL=[0-9]+' | sed 's/^FULL=//' | cut -d/ -f1 | tail -1 || true)
    case "${n:-}" in ''|*[!0-9]*) n=0;; esac
    [ "${n}" -ge "$expect" ] && return 0
    local el=$(( $(date +%s) - t0 ))
    if [ "$el" -ge "$force" ]; then
      echo "[prof] FORCE after ${el}s (full=${n:-?}/$expect) — reissuing rect"; rcon "forceload add $x1 $z1 $x2 $z2" >/dev/null || true
    elif [ "$el" -ge "$reissue" ] && [ $((el % reissue)) -lt 2 ]; then
      rcon "forceload add $x1 $z1 $x2 $z2" >/dev/null || true
    fi
    sleep 1
  done
}

# --- JIT warmup (outside measured region) ------------------------------------
echo "=== warmup: 8x8 at cx $WARM_CXMIN..$WARM_CXMAX ==="
T0=$(date +%s%3N)
rcon "forceload add $(blockof $WARM_CXMIN) $(blockof $WARM_CZMIN) $(( $(blockof $WARM_CXMAX) + 15 )) $(( $(blockof $WARM_CZMAX) + 15 ))" >/dev/null
wait_full "$(blockof $WARM_CXMIN)" "$(blockof $WARM_CZMIN)" "$(( $(blockof $WARM_CXMAX) + 15 ))" "$(( $(blockof $WARM_CZMAX) + 15 ))" 64 25 70
T1=$(date +%s%3N)
echo "warmup wall: $((T1 - T0)) ms (64 chunks)"
rcon "forceload remove all" >/dev/null
sleep 3
rcon "crusgenstats reset" >/dev/null

# --- measured window: waves of ROWS_BATCH rows -------------------------------
{
  echo "--- measured window: 12x12 cx $MEAS_CXMIN..$MEAS_CXMAX cz $MEAS_CZMIN..$MEAS_CZMAX ---"
} >> "$REPORT"
spark_on=0
SPARK_OUT="$(rcon "spark profiler start" 2>/dev/null || true)"
echo "spark start: $SPARK_OUT"
case "$SPARK_OUT" in *profiler*|*start*|*Start*) spark_on=1;; esac
if [ "$spark_on" = "0" ]; then
  sleep 2
  SPARK_OUT="$(rcon "spark profiler start" 2>/dev/null || true)"
  echo "spark retry: $SPARK_OUT"
  case "$SPARK_OUT" in *profiler*|*start*|*Start*) spark_on=1;; esac
fi
echo "spark profiler: on=$spark_on"

WIN_T0=$(date +%s%3N)
czA=$MEAS_CZMIN
while [ "$czA" -le "$MEAS_CZMAX" ]; do
  czB=$(( czA + ROWS_BATCH - 1 )); [ "$czB" -gt "$MEAS_CZMAX" ] && czB=$MEAS_CZMAX
  BT0=$(date +%s%3N)
  rcon "forceload add $(blockof $MEAS_CXMIN) $(blockof $czA) $(( $(blockof $MEAS_CXMAX) + 15 )) $(( $(blockof $czB) + 15 ))" >/dev/null
  CH=$(( (MEAS_CXMAX - MEAS_CXMIN + 1) * (czB - czA + 1) ))
  wait_full "$(blockof $MEAS_CXMIN)" "$(blockof $czA)" "$(( $(blockof $MEAS_CXMAX) + 15 ))" "$(( $(blockof $czB) + 15 ))" "$CH" 25 70
  BT1=$(date +%s%3N)
  LINE="batch cz $czA..$czB: $CH chunks in $((BT1 - BT0)) ms -> $(( (BT1 - BT0) / CH )).$(( ((BT1 - BT0) * 10 / CH) % 10 )) ms/chunk"
  echo "$LINE"; echo "$LINE" >> "$REPORT"
  czA=$(( czB + 1 ))
done
WIN_T1=$(date +%s%3N)
WIN_MS=$(( WIN_T1 - WIN_T0 ))
NCH=$(( (MEAS_CXMAX - MEAS_CXMIN + 1) * (MEAS_CZMAX - MEAS_CZMIN + 1) ))
echo "=== window: ${WIN_MS} ms / $NCH chunks = $(( WIN_MS / NCH )).$(( WIN_MS * 10 / NCH % 10 )) ms/chunk end-to-end ==="

# --- spark stop + stats ------------------------------------------------------
if [ "$spark_on" = "1" ]; then
  rcon "spark profiler stop" 2>/dev/null | tail -3 || true
  sleep 3
fi
{
  echo "window_ms=$WIN_MS chunks=$NCH ms_per_chunk=$(python3 -c "print(f'{$WIN_MS/$NCH:.2f}')")"
  echo "--- crusgenstats (final) ---"
  rcon "crusgenstats" || true
  echo "--- spark links in console ---"
  grep -o 'https://spark.lucko.me/[A-Za-z0-9]*' "$SRV/run.out" | tail -3 || echo "(no spark link)"
} >> "$REPORT"
echo "=== final stats ==="; tail -8 "$REPORT"

bash harness/server_down.sh >/dev/null 2>&1 || true
echo "=== report: $REPORT ==="
