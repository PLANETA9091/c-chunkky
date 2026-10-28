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
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

SRV="${SRV_DIR:-$ROOT/server}"
PORT="${RCON_PORT:-25576}"
SEED="${SEED:-3257840388504953787}"
WX="${WX:-61440}"; WZ="${WZ:-61440}"      # warmup 4x4 (chunk coords)
WN="${WN:-3}"
RX="${RX:-70656}"; RZ="${RZ:-70656}"      # timed region (chunk coords)
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
sync-chunk-writes=true
view-distance=10
simulation-distance=10
spawn-protection=0
max-players=2
enable-command-block=false
motd=crussty e2e263
EOF

stop_server
rm -rf "$SRV/world" "$SRV/world_nether" "$SRV/world_the_end"
cp "$PLUGIN_JAR" "$SRV/plugins/CrusstyGen.jar"
cp "$PROBE_JAR" "$SRV/plugins/CrusstyProbe263.jar"
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
  echo "E2E BOOT FAILED — last log lines:" >&2
  tail -30 "$SRV/run.out" >&2 || true
  stop_server
  exit 1
fi
echo "boot ok (e2e full-native, deco=${CRUS_NATIVE_DECO:-0})"

stats_chunks() {
  $RCON crusgenstats 2>/dev/null | grep -o "chunks=[0-9]*" | head -1 | cut -d= -f2
}

# ---- warmup: small far region (JIT/lib warm, ring buffer primed) ------------
$RCON "forceload add $((WX * 16)) $((WZ * 16)) $(((WX + WN) * 16 + 15)) $(((WZ + WN) * 16 + 15))" >/dev/null
wtarget=$(( (WN + 1) * (WN + 1) ))
for i in $(seq 1 240); do
  c=$(stats_chunks)
  [ -n "$c" ] && [ "$c" -ge "$wtarget" ] && break
  sleep 0.5
done
sleep 8   # settle: flush async writes so the timed window starts idle

# ---- timed window: reset stats, forceload fresh NxN, poll to completion -----
$RCON "crusgenstats reset" >/dev/null
target=$(( (N + 1) * (N + 1) ))
t0=$(date +%s.%N)
$RCON "forceload add $((RX * 16)) $((RZ * 16)) $(((RX + N) * 16 + 15)) $(((RZ + N) * 16 + 15))" >/dev/null
done=0
for i in $(seq 1 480); do
  c=$(stats_chunks)
  if [ -n "$c" ] && [ "$c" -ge "$target" ]; then done=1; break; fi
  sleep 0.5
done
t1=$(date +%s.%N)
if [ "$done" != "1" ]; then
  echo "E2E WINDOW TIMEOUT (chunks=$c / $target)"
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
print(f"e2e_ms_per_chunk={w * 1000 / n:.1f}  wall={w:.2f}s  chunks={n}")
PYEOF

echo "=== GenStats window breakdown (exactly the timed chunks) ==="
echo "$STATS" | tr '|' '\n' | sed 's/\[CrusstyGen\]//g; s/^ *//' | grep -v '^$'
