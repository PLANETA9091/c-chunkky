#!/usr/bin/env bash
# Far-coords native-gen probe (diagnostic, CI-only):
# boots the 26.3 server and forceloads a 2x2 region at increasing chunk
# distances from origin, polling GenStats for completion at each step.
# The FIRST distance that produces zero completed chunks in its budget marks
# the far-gen stall cliff (CI 2026-10-28: chunk 61440 never completed while
# the server stayed healthy; near-origin gen is verified by the A/B gates).
#
# NOTE: once the chunk system wedges, every later distance also reads zero —
# the first stall is the answer.
#
# Env: SRV_DIR, RCON_PORT, SEED, JAVA_BIN. Distances hardcoded below.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

SRV="${SRV_DIR:-$ROOT/server}"
PORT="${RCON_PORT:-25576}"
SEED="${SEED:-3257840388504953787}"
JAVA_BIN="${JAVA_BIN:-${JAVA_HOME:-}/bin/java}"
[ -x "$JAVA_BIN" ] || JAVA_BIN="$(command -v java)"
RCON="python3 harness/rcon.py localhost $PORT crussty"
PLUGIN_JAR="$ROOT/plugin/CrusstyGen.jar"
PROBE_JAR="$ROOT/probe263/CrusstyProbe263.jar"
SO="$ROOT/native/target/release/libcrussty_chunkgen.so"

[ -f "$PLUGIN_JAR" ] || { echo "missing $PLUGIN_JAR" >&2; exit 1; }
[ -f "$PROBE_JAR" ] || { echo "missing $PROBE_JAR" >&2; exit 1; }
[ -f "$SO" ] || { echo "missing $SO" >&2; exit 1; }
[ -f "$SRV/paper.jar" ] || { echo "missing $SRV/paper.jar (bootstrap first)" >&2; exit 1; }

stop_server() {
  pkill -9 -f 'java .*-jar paper\.jar' 2>/dev/null || true
  pkill -9 -f 'java .*paper-26\.3' 2>/dev/null || true
  sleep 2
}

cat > "$SRV/server.properties" <<EOF
level-name=world
level-seed=$SEED
level-type=minecraft\:normal
allow-nether=false
allow-end=false
generate-structures=false
online-mode=false
enable-rcon=true
rcon.port=$PORT
rcon.password=crussty
sync-chunk-writes=true
view-distance=10
simulation-distance=10
spawn-protection=0
max-players=2
enable-command-block=false
motd=crussty farprobe263
EOF

stop_server
rm -rf "$SRV/world" "$SRV/world_nether" "$SRV/world_the_end"
cp "$PLUGIN_JAR" "$SRV/plugins/CrusstyGen.jar"
cp "$PROBE_JAR" "$SRV/plugins/CrusstyProbe263.jar"
# Wire the native generator (see e2e_ci.sh: without bukkit.yml the noise
# stage stays vanilla and the probe measures nothing).
cat > "$SRV/bukkit.yml" <<EOF
worlds:
  world:
    generator: CrusstyGen
EOF
rm -f "$SRV/run.in"
mkfifo "$SRV/run.in"
( sleep 1800 > "$SRV/run.in" ) >/dev/null 2>&1 &
(
  cd "$SRV"
  env CRUS_NATIVE_CARVERS=1 CRUS_NATIVE_DECO="${CRUS_NATIVE_DECO:-0}" \
    setsid "$JAVA_BIN" -Xms2G -Xmx3G -Dpaper.chunk-system.gen-parallelism=false \
    -Djava.library.path="$ROOT/native/target/release" \
    -jar paper.jar nogui < run.in > run.out 2>&1 &
  echo $! > server.pid
)
up=0
for i in $(seq 1 60); do
  if grep -q "Done (" "$SRV/run.out" 2>/dev/null; then up=1; break; fi
  sleep 5
done
if [ "$up" != "1" ]; then
  echo "FARPROBE BOOT FAILED" >&2
  tail -30 "$SRV/run.out" >&2 || true
  stop_server
  exit 1
fi
echo "boot ok (farprobe)"

stats_chunks() {
  local c
  c=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2)
  echo "${c:-0}"
}

echo "=== far-gen probe (2x2 per distance, 45s budget each) ==="
stall=0
for D in 64 1024 8192 16384 32768 61440; do
  before=$(stats_chunks)
  $RCON "forceload add $((D * 16)) $((D * 16)) $((D * 16 + 31)) $((D * 16 + 31))" >/dev/null
  ok=0
  for i in $(seq 1 90); do
    c=$(stats_chunks)
    if [ "$c" -ge $((before + 4)) ]; then ok=1; break; fi
    sleep 0.5
  done
  after=$(stats_chunks)
  if [ "$ok" = "1" ]; then
    echo "D=$D (chunk)  chunks $before -> $after  OK"
  else
    echo "D=$D (chunk)  chunks $before -> $after  STALL"
    if [ "$stall" = "0" ]; then stall=$D; fi
  fi
  $RCON "forceload remove $((D * 16)) $((D * 16)) $((D * 16 + 31)) $((D * 16 + 31))" >/dev/null
  [ "$stall" != "0" ] && break   # wedged — further distances are meaningless
done
sleep 1
stop_server
if [ "$stall" != "0" ]; then
  echo "far-gen stall cliff: chunk $stall (block $((stall * 16)))"
else
  echo "far-gen probe: all distances OK"
fi
