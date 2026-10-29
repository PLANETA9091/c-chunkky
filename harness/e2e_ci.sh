#!/usr/bin/env bash
# E2E per-chunk timing on the 26.3 server (CI step, twin of ab263_ci.sh boot):
# boot in the production native mode (native surface + native carvers), warm
# up on a small far region, reset the GenStats window, forceload a fresh
# NxN region, poll until done, then report:
#   - wall window ms/chunk (honest e2e: includes Paper scheduling, lighting,
#     biome post, DecoDriver features — everything the server pays per chunk)
#   - the full GenStats breakdown over EXACTLY the timed chunks (reset first):
#     native fill mean/p50 (JNI), layout, beard, marks, biome, deco, jni_prof
#
# Env: SRV_DIR (default server), RCON_PORT (25576), SEED, WX/WZ (warmup
#      region origin, chunk coords), RX/RZ (timed region origin), N
#      (chunk-side; NxN region), JAVA_BIN.
#
# DEFAULT COORDS: near-origin (~chunk 1050/1100), verified gen-active by the
# A/B gates (block 16384 ≈ chunk 1024). CI 2026-10-28: far coords (chunk
# 61440/70656) NEVER complete a single native gen — zero GenStats chunks in
# 240s with the server healthy. Do not move the defaults far without a
# far-gen fix (see harness/farprobe_ci.sh).
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

SRV="${SRV_DIR:-$ROOT/server}"
PORT="${RCON_PORT:-25576}"
SEED="${SEED:-3257840388504953787}"
WX="${WX:-1050}"; WZ="${WZ:-1050}"        # warmup 4x4 (chunk coords)
WN="${WN:-3}"
RX="${RX:-1100}"; RZ="${RZ:-1100}"        # timed region (chunk coords)
N="${N:-11}"                              # 12x12 = 144 chunks
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
sync-chunk-writes=false
view-distance=10
simulation-distance=10
spawn-protection=0
max-players=2
enable-command-block=false
motd=crussty e2e263
EOF

stop_server
rm -rf "$SRV/world" "$SRV/world_nether" "$SRV/world_the_end"
# Kill the spawn backlog AT THE SOURCE: without a pre-set paper-world-defaults
# the first boot generates ~169 spawn chunks (13x13) in the background; they
# throttle under forceload tickets and resume INSIDE the timed window
# (fills=314/320 for 144-chunk windows; gamerule spawnChunkRadius 0 does not
# cancel already-scheduled tasks; force-drain fails the same way). With
# keep-spawn-loaded:false the spawn area is never generated at boot.
mkdir -p "$SRV/config/paper-world-defaults/worlds"
# global defaults + per-world override (both paths, whichever Paper honours)
cat > "$SRV/config/paper-world-defaults.yml" <<EOF2
chunks:
  keep-spawn-loaded: false
EOF2
cat > "$SRV/config/paper-world-defaults/worlds/world.yml" <<EOF2
chunks:
  keep-spawn-loaded: false
EOF2
cp "$PLUGIN_JAR" "$SRV/plugins/CrusstyGen.jar"
cp "$PROBE_JAR" "$SRV/plugins/CrusstyProbe263.jar"
# CRITICAL: wire the world generator or the noise stage stays VANILLA.
# getDefaultWorldGenerator resolves only via bukkit.yml — without this the
# boot logs surface/carver/biome handshakes but generateNoise is vanilla
# (GenStats COUNT stays 0 forever; observed CI 2026-10-28).
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
    CRUS_FILL263_PROF="${CRUS_FILL263_PROF:-0}" \
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
  echo "E2E BOOT FAILED — last log lines:" >&2
  tail -30 "$SRV/run.out" >&2 || true
  stop_server
  exit 1
fi
echo "boot ok (e2e full-native, deco=${CRUS_NATIVE_DECO:-0})"
# Kill the spawn region: its ~169-chunk background backlog THROTTLES while
# forceload tickets are pending and RESUMES inside the timed window
# (observed: fills=320/314 for a 144-chunk window). The gamerule alone does
# NOT cancel already-scheduled spawn tasks — force-drain the whole spawn
# area BEFORE the window instead (see the drain block below, after
# stats_chunks).
$RCON "gamerule spawnChunkRadius 0" >/dev/null 2>&1 || true

stats_chunks() {
  local c
  c=$($RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2)
  echo "${c:-0}"
}

# force-drain the spawn backlog BEFORE the warmup/window (gamerule does not
# cancel pending spawn tasks; ticketing the spawn area gives them priority
# NOW so the timed window starts with an idle gen thread).
$RCON "forceload add -112 -112 111 111" >/dev/null   # chunks -7..6 (spawn 13x13)
last=-1; stable=0
for i in $(seq 1 480); do
  c=$(stats_chunks)
  if [ "$c" = "$last" ]; then stable=$((stable + 1)); else stable=0; fi
  [ "$stable" -ge 16 ] && break
  last=$c; sleep 0.5
done
$RCON "forceload remove -112 -112 111 111" >/dev/null
echo "spawn backlog drained (fills so far: ${last:-?})"

# ---- warmup: small far region (JIT/lib warm, ring buffer primed) ------------
# NOTE: the boot's spawn region (~169 chunks) keeps generating in the
# background well after "Done" — wait for before+warmup, not a bare target,
# or the stats reset lands mid-spawn-gen and the timed window mixes in ~170
# foreign fills (observed: chunks=313 for a 144-chunk window).
before=$(stats_chunks)
$RCON "forceload add $((WX * 16)) $((WZ * 16)) $(((WX + WN) * 16 + 15)) $(((WZ + WN) * 16 + 15))" >/dev/null
wtarget=$((before + (WN + 1) * (WN + 1)))
for i in $(seq 1 240); do
  c=$(stats_chunks)
  [ "$c" -ge "$wtarget" ] && break
  sleep 0.5
done
# settle: wait until the count goes STABLE (spawn-region backlog drained) —
# the timed window must start with an idle gen thread.
last=-1; stable=0
for i in $(seq 1 480); do
  c=$(stats_chunks)
  if [ "$c" = "$last" ]; then stable=$((stable + 1)); else stable=0; fi
  [ "$stable" -ge 8 ] && break
  last=$c; sleep 0.5
done

# ---- timed window: arm the REGION-BOUNDED window, forceload fresh NxN, poll
# to completion via wincount (0.05s granularity). The honest e2e number is
# the plugin-side span (first..last in-window fill) — immune to rcon latency
# and poll granularity; harness wall is kept for the dispatch overhead view.
$RCON "crusgenstats window $RX $RZ $N" >/dev/null
target=$(( (N + 1) * (N + 1) ))
t0=$(date +%s.%N)
$RCON "forceload add $((RX * 16)) $((RZ * 16)) $(((RX + N) * 16 + 15)) $(((RZ + N) * 16 + 15))" >/dev/null
done=0
for i in $(seq 1 6000); do
  c=$($RCON "crusgenstats wincount" 2>/dev/null | grep -o "window_inside=[0-9]*" | cut -d= -f2)
  c=${c:-0}
  if [ "$c" -ge "$target" ]; then done=1; break; fi
  if (( i % 600 == 0 )); then echo "  [window progress] inside=$c / $target"; fi
  sleep 0.05
done
t1=$(date +%s.%N)
if [ "$done" != "1" ]; then
  echo "E2E WINDOW TIMEOUT (inside=$c / $target)"
fi

nchunks=$(( (N + 1) * (N + 1) ))
STATS="$($RCON crusgenstats 2>/dev/null)"
sleep 2
stop_server

python3 - "$t0" "$t1" "$nchunks" "${CRUS_NATIVE_DECO:-0}" <<'PYEOF'
import sys
t0, t1, n, deco = float(sys.argv[1]), float(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
w = t1 - t0
print(f"=== e2e window ({n} chunks, fresh region, full-native, deco={deco}) ===")
print(f"harness_wall_ms_per_chunk={w * 1000 / n:.1f}  wall={w:.2f}s  chunks={n}")
PYEOF

echo "=== GenStats window breakdown (exactly the timed chunks) ==="
echo "$STATS" | tr '|' '\n' | sed 's/\[CrusstyGen\]//g; s/^ *//' | grep -v '^$'
