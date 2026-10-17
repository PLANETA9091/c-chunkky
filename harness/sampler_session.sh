#!/usr/bin/env bash
# sampler_session.sh <world> <tag> <crussty|vanilla>
#
# jstack sampling profile of a steady-state generation window:
#   - boots the server on an EXISTING world (no wipe; region must be fresh at
#     cx 60..71 / cz 60..71), warmup 8x8 at cx 48..55, then a 12x12 window at
#     cx 60..71 sampled with jstack back-to-back for the whole window
#   - final crusgenstats via rcon (rcon.py now drains multi-packet replies)
# Outputs: reference/sampler_<tag>.txt + reference/jstack_<tag>/NNN.txt
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

WORLD="${1:?usage: sampler_session.sh <world> <tag> <crussty|vanilla>}"
TAG="${2:?usage: sampler_session.sh <world> <tag> <crussty|vanilla>}"
GEN="${3:-crussty}"
SRV=server
HOST=127.0.0.1 PORT=25575 PASS=crussty
SEED=3257840388504953787
REPORT="reference/sampler_${TAG}.txt"
DUMPDIR="reference/jstack_${TAG}"
export CRUS_FILL_THREADS="${CRUS_FILL_THREADS:-2}"
export CRUS_ALLOW_PARALLEL=1
export CRUS_HEAP="${CRUS_HEAP:--Xms1G -Xmx2G}"

WARM_CMIN="${WARM_CMIN:-48}" WARM_CMAX="${WARM_CMAX:-55}"
MEAS_CMIN="${MEAS_CMIN:-80}" MEAS_CMAX="${MEAS_CMAX:-91}"

rcon() {
  local tries=0
  while [ "$tries" -lt 20 ]; do
    if python3 harness/rcon.py "$HOST" "$PORT" "$PASS" "$1" 2>/dev/null; then return 0; fi
    tries=$((tries + 1)); sleep 2
  done
  echo "rcon FAILED: $1" >&2; return 3
}
blockof() { echo $(( $1 * 16 )); }

# --- wiring (no wipe; datapacks must already be in the world dir) ------------
bash harness/server_down.sh >/dev/null 2>&1 || true
pkill -9 -f 'paper\.jar' 2>/dev/null || true
sleep 3
if [ "$GEN" = "crussty" ]; then
  printf 'worlds:\n  %s:\n    generator: CrusstyGen\n' "$WORLD" > "$SRV/bukkit.yml"
else
  printf 'worlds: {}\n' > "$SRV/bukkit.yml"
fi
sed -i "s/^level-name=.*/level-name=$WORLD/; s/^level-seed=.*/level-seed=$SEED/; s/^allow-nether=.*/allow-nether=false/" "$SRV/server.properties"

mkdir -p "$SRV/plugins" "$DUMPDIR"
find "$SRV/plugins" -maxdepth 1 -name '*.jar' ! -name 'CrusstyProbe.jar' ! -name 'spark*.jar' -delete
cp plugin/CrusstyGen.jar "$SRV/plugins/CrusstyGen.jar"
echo "# sampler session $TAG $(date -u +%Y-%m-%dT%H:%M:%SZ) world=$WORLD gen=$GEN" > "$REPORT"
ls "$SRV/$WORLD/datapacks/" 2>/dev/null >> "$REPORT" || echo "(no datapacks)" >> "$REPORT"

bash harness/server_up.sh
rcon "gamerule spawnChunkRadius 0" >/dev/null 2>&1 || true

wait_full() { # wait_full <x1> <z1> <x2> <z2> <expect>
  local x1="$1" z1="$2" x2="$3" z2="$4" expect="$5" t0 n
  t0=$(date +%s)
  while :; do
    n=$(rcon "crusfull $x1 $z1 $x2 $z2" 2>/dev/null | grep -m1 -oE 'FULL=[0-9]+' | sed 's/^FULL=//' | cut -d/ -f1 | tail -1 || true)
    case "${n:-}" in ''|*[!0-9]*) n=0;; esac
    [ "${n}" -ge "$expect" ] && return 0
    local el=$(( $(date +%s) - t0 ))
    if [ "$el" -ge 60 ]; then
      echo "[sampler] FORCE reissue after ${el}s"; rcon "forceload add $x1 $z1 $x2 $z2" >/dev/null || true
    elif [ "$el" -ge 25 ] && [ $((el % 25)) -lt 2 ]; then
      rcon "forceload add $x1 $z1 $x2 $z2" >/dev/null || true
    fi
    sleep 1
  done
}

# --- warmup ------------------------------------------------------------------
rcon "forceload add $(blockof $WARM_CMIN) $(blockof $WARM_CMIN) $(( $(blockof $WARM_CMAX) + 15 )) $(( $(blockof $WARM_CMAX) + 15 ))" >/dev/null
wait_full "$(blockof $WARM_CMIN)" "$(blockof $WARM_CMIN)" "$(( $(blockof $WARM_CMAX) + 15 ))" "$(( $(blockof $WARM_CMAX) + 15 ))" 64
rcon "forceload remove all" >/dev/null
sleep 3
rcon "crusgenstats reset" >/dev/null

# --- sampled window ----------------------------------------------------------
# JFR around the window via jcmd attach (started AFTER warmup => clean slice).
# profile settings: ExecutionSample 10ms, NativeMethodSample 20ms (JNI fills).
# Sandbox JRE has no launcher binaries; module classes work directly
# (java -m jdk.jcmd/sun.tools.jcmd.JCmd), and `jfr print` equivalent:
#   java -m jdk.jfr/jdk.jfr.internal.tool.Main print <file.jfr>
JC="java -m jdk.jcmd/sun.tools.jcmd.JCmd"
JPID="$(cat "$SRV/server.pid")"
WIN_T0=$(date +%s%3N)
$JC "$JPID" JFR.start name=prof settings=profile filename="$ROOT/reference/jfr_${TAG}.jfr" >/dev/null 2>&1 || echo "[sampler] JFR.start FAILED"
rcon "forceload add $(blockof $MEAS_CMIN) $(blockof $MEAS_CMIN) $(( $(blockof $MEAS_CMAX) + 15 )) $(( $(blockof $MEAS_CMAX) + 15 ))" >/dev/null
while :; do
  n=$(rcon "crusfull $(blockof $MEAS_CMIN) $(blockof $MEAS_CMIN) $(( $(blockof $MEAS_CMAX) + 15 )) $(( $(blockof $MEAS_CMAX) + 15 ))" 2>/dev/null | grep -m1 -oE 'FULL=[0-9]+' | sed 's/^FULL=//' | cut -d/ -f1 | tail -1 || true)
  case "${n:-}" in ''|*[!0-9]*) n=0;; esac
  [ "${n}" -ge 144 ] && break
  local_el=$(( $(date +%s) - WIN_T0 / 1000 ))
  if [ $(( local_el % 25 )) -eq 0 ]; then
    rcon "forceload add $(blockof $MEAS_CMIN) $(blockof $MEAS_CMIN) $(( $(blockof $MEAS_CMAX) + 15 )) $(( $(blockof $MEAS_CMAX) + 15 ))" >/dev/null || true
  fi
  sleep 1
done
WIN_T1=$(date +%s%3N)
WIN_MS=$(( WIN_T1 - WIN_T0 ))
$JC "$JPID" JFR.stop name=prof >/dev/null 2>&1 || echo "[sampler] JFR.stop FAILED"
echo "=== window: ${WIN_MS} ms / 144 chunks = $(( WIN_MS / 144 )).$(( WIN_MS * 10 / 144 % 10 )) ms/chunk (jfr) ==="

{
  echo "window_ms=$WIN_MS"
  echo "--- crusgenstats (final) ---"
  rcon "crusgenstats" || true
} >> "$REPORT"
echo "--- final stats ---"; cat "$REPORT"

bash harness/server_down.sh >/dev/null 2>&1 || true
echo "=== report: $REPORT | jfr: reference/jfr_${TAG}.jfr ==="
