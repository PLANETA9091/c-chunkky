#!/usr/bin/env bash
# judge_dptt.sh <world> <tag> — datapack parity judge (Terralith+Tectonic).
#   fresh world + packs + CrusstyGen, forceload the 8x8 region (cx/cz -4..3),
#   settle 15s, crusstydumpregion -> reference/dumps/<tag>/,
#   diff vs reference/dumps_plugin/datapack (vanilla Paper + same packs).
# Env: DP_DIR (default server_datapacks), REF (default dumps_plugin/datapack).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
WORLD="${1:?usage: judge_dptt.sh <world> <tag>}"
TAG="${2:?usage: judge_dptt.sh <world> <tag>}"
DP_DIR="${DP_DIR:-server_datapacks}"
REF="${REF:-reference/dumps_plugin/dptt}"
DUMPDIR="reference/dumps/$TAG"
HOST=127.0.0.1 PORT=25575 PASS=crussty
export CRUS_FILL_THREADS="${CRUS_FILL_THREADS:-2}"
export CRUS_ALLOW_PARALLEL=1
export CRUS_HEAP="${CRUS_HEAP:--Xms1G -Xmx2G}"

rcon() {
  local tries=0
  while [ "$tries" -lt 20 ]; do
    if python3 harness/rcon.py "$HOST" "$PORT" "$PASS" "$1" 2>/dev/null; then return 0; fi
    tries=$((tries + 1)); sleep 2
  done
  echo "rcon FAILED: $1" >&2; return 3
}

bash harness/server_down.sh >/dev/null 2>&1 || true
pkill -9 -f 'paper\.jar' 2>/dev/null || true
sleep 3
rm -rf "server/$WORLD" "server/${WORLD}_nether" "server/${WORLD}_the_end"
mkdir -p "server/$WORLD/datapacks"
for z in "$DP_DIR"/*.zip; do [ -e "$z" ] && cp "$z" "server/$WORLD/datapacks/"; done
printf 'worlds:\n  %s:\n    generator: CrusstyGen\n' "$WORLD" > server/bukkit.yml
sed -i "s/^level-name=.*/level-name=$WORLD/; s/^level-seed=.*/level-seed=3257840388504953787/; s/^allow-nether=.*/allow-nether=false/" server/server.properties
grep -qE "^allow-nether=false" server/server.properties || echo "allow-nether=false" >> server/server.properties

mkdir -p server/plugins
find server/plugins -maxdepth 1 -name '*.jar' ! -name 'CrusstyProbe.jar' ! -name 'spark*.jar' -delete
cp plugin/CrusstyGen.jar server/plugins/CrusstyGen.jar

bash harness/server_up.sh
rcon "gamerule spawnChunkRadius 0" >/dev/null 2>&1 || true

# --- wait for the 8x8 region (cx/cz -4..3 => blocks -64..63) ------------------
wait_full() {
  local expect="$1" t0 n el
  t0=$(date +%s)
  while :; do
    n=$(rcon "crusfull -64 -64 63 63" 2>/dev/null | grep -m1 -oE 'FULL=[0-9]+' | sed 's/^FULL=//' | cut -d/ -f1 | tail -1 || true)
    case "${n:-}" in ''|*[!0-9]*) n=0;; esac
    [ "${n}" -ge "$expect" ] && return 0
    el=$(( $(date +%s) - t0 ))
    if [ "$el" -ge 90 ]; then
      echo "[judge] FORCE reissue after ${el}s (full=${n:-?}/$expect)"; rcon "forceload add -64 -64 63 63" >/dev/null || true
    elif [ "$el" -ge 25 ] && [ $((el % 25)) -lt 2 ]; then
      rcon "forceload add -64 -64 63 63" >/dev/null || true
    fi
    sleep 1
  done
}
rcon "forceload add -64 -64 63 63" >/dev/null
wait_full 64

sleep 15  # fluid-sim settle (deterministic steady state, same as reference)

mkdir -p "$DUMPDIR"
rcon "crusstydumpregion -4 -4 3 3 $ROOT/$DUMPDIR chunk $WORLD" || true
sleep 2
bash harness/server_down.sh >/dev/null 2>&1 || true

python3 harness/verify_dumps.py "$DUMPDIR" 64
echo "=== diff vs $REF ==="
python3 harness/diff_dumps.py "$DUMPDIR" "$REF"
