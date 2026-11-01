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

# cont-93: both CRUS_NATIVE_DECO lanes of run 41324500487 died silently
# (boot ok -> spawn backlog drained -> "CMD_FAIL: connection closed" ->
# forceload FAILED -> 8min of dead polls -> timeout) and the log hid the
# cause: the post-window run.out dump greps ONLY run-prof|ore-prof lines,
# so the Rust panic / JVM fatal error / OOM evidence never reached CI.
dump_server_diag() {
  echo "=== SERVER DEATH DIAG (unfiltered run.out tail + hs_err + memory) ==="
  if [ -f "$SRV/run.out" ]; then
    echo "--- run.out tail (last 250 lines, unfiltered) ---"
    tail -n 250 "$SRV/run.out" 2>/dev/null
  else
    echo "(no $SRV/run.out)"
  fi
  local hs
  hs=$(ls -t "$SRV"/hs_err_pid*.log 2>/dev/null | head -1)
  if [ -n "$hs" ]; then
    echo "--- hs_err: $hs (first 120 lines) ---"
    head -n 120 "$hs" 2>/dev/null
  else
    echo "(no hs_err_pid*.log in $SRV)"
  fi
  echo "--- memory ---"
  free -m 2>/dev/null || true
  (dmesg 2>/dev/null || sudo -n dmesg 2>/dev/null) \
    | grep -i "killed process\|out of memory" | tail -5 \
    || echo "(no OOM evidence / dmesg unavailable)"
  echo "=== END SERVER DEATH DIAG ==="
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
# CRUS_GEN_PARALLEL=1 boots with Paper's default parallel chunk generation
# (the timing lane only — judge lanes keep serial semantics for the RNG
# streams; the plugin pool is per-ctx ReentrantLock + RR, thread-safe).
PGEN_FLAG="-Dpaper.chunk-system.gen-parallelism=false"
if [ "${CRUS_GEN_PARALLEL:-0}" = "1" ]; then PGEN_FLAG=""; fi
# CRUS_WORKER_THREADS=N pins chunk-system.worker-threads: the auto value is
# ONE thread for <=3 physical cores (paper docs), and the 4-vCPU CI runner
# detects <=3 physical — Paper's gen pool is a single worker, which is why
# CRUS_GEN_PARALLEL alone changed nothing (verified by the JFR window:
# only Paper Common Worker #0 ever fills). POOL_SIZE in the plugin is 4.
if [ -n "${CRUS_WORKER_THREADS:-}" ] && [ -f "$SRV/config/paper-global.yml" ]; then
  sed -i -E "s/^([[:space:]]*)worker-threads:.*$/\\1worker-threads: ${CRUS_WORKER_THREADS}/" "$SRV/config/paper-global.yml"
  echo "chunk-system.worker-threads=${CRUS_WORKER_THREADS} (patched $SRV/config/paper-global.yml)"
elif [ -f "$SRV/config/paper-global.yml" ]; then
  # No explicit pin: undo any pin a previous lane left in the shared
  # workspace. cont-72: the par4 lane pinned worker-threads=4 and every
  # later lane (big 48x48, farprobe) booted with it — Paper 26.3 then
  # destabilized (MissingPaletteEntryException at save; big lane's server
  # died right after boot, 0/2304 chunks). -1 = Paper auto heuristic.
  if ! grep -qE '^[[:space:]]*worker-threads:[[:space:]]*-1[[:space:]]*$' "$SRV/config/paper-global.yml"; then
    sed -i -E "s/^([[:space:]]*)worker-threads:.*$/\\1worker-threads: -1/" "$SRV/config/paper-global.yml"
    echo "chunk-system.worker-threads=-1 (reset to auto — previous lane may have pinned)"
  fi
fi
(
  cd "$SRV"
  env CRUS_NATIVE_CARVERS=1 CRUS_NATIVE_DECO="${CRUS_NATIVE_DECO:-0}" \
    CRUS_FILL263_PROF="${CRUS_FILL263_PROF:-0}" \
    CRUS_BIOME_GRID="${CRUS_BIOME_GRID:-1}" \
    setsid "$JAVA_BIN" -Xms2G -Xmx3G $PGEN_FLAG \
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
if ! $RCON "list" >/dev/null 2>&1; then
  echo "  [SERVER DEAD] right after spawn backlog drain (the deco-lane death point) — lane ends early"
  dump_server_diag
  exit 0
fi
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
# Optional JFR around EXACTLY the window (CRUS_JFR=1): ExecutionSample 10ms
# profile — hot-methods of the window = the Paper-side split (scheduling,
# light, serialization, JNI) the honest e2e number lumps together.
JC=""; JPID=""; JFR_FILE=""
if [ "${CRUS_JFR:-0}" = "1" ]; then
  JC="$(dirname "$JAVA_BIN")/jcmd"; [ -x "$JC" ] || JC=jcmd
  JPID="$(cat "$SRV/server.pid")"
  # distinct recording name per lane so parallel-run artifacts don't overwrite
  if [ "${CRUS_GEN_PARALLEL:-0}" = "1" ]; then
    JFR_FILE="$ROOT/e2e263_window_par.jfr"
  else
    JFR_FILE="$ROOT/e2e263_window.jfr"
  fi
  rm -f "$JFR_FILE"
  "$JC" "$JPID" JFR.configure stackdepth=128 >/dev/null 2>&1 || true
  "$JC" "$JPID" JFR.start name=win settings=profile filename="$JFR_FILE" >/dev/null 2>&1 \
    && echo "JFR started (profile, window slice)" || echo "JFR.start FAILED"
fi
t0=$(date +%s.%N)
# vanilla /forceload add rejects areas over 256 chunks — a 48x48 window
# (2304 chunks) failed SILENTLY (stderr to /dev/null) and the lane burned
# its whole timeout at inside=0/2304 (runs 7fe1e722 / c0e33eb9). Split into
# <=256-chunk tiles (256x256 blocks each), clamped to the window rect.
fminx=$((RX * 16)); fmaxx=$(((RX + N) * 16 + 15))
fminz=$((RZ * 16)); fmaxz=$(((RZ + N) * 16 + 15))
for bx in $(seq "$fminx" 256 "$fmaxx"); do
  for bz in $(seq "$fminz" 256 "$fmaxz"); do
    ex=$((bx + 255)); [ "$ex" -gt "$fmaxx" ] && ex=$fmaxx
    ez=$((bz + 255)); [ "$ez" -gt "$fmaxz" ] && ez=$fmaxz
    $RCON "forceload add $bx $bz $ex $ez" >/dev/null \
      || echo "  [forceload tile FAILED] $bx $bz -> $ex $ez"
  done
done
# cont-93 fail-fast: a tile failure is usually the first VISIBLE symptom of
# a dead server (the death itself happened minutes earlier, e.g. the deco
# lanes' JVM abort right after the spawn backlog). Probe once; on a dead
# server dump the diag and skip the window entirely (the old path burned
# ~8 min of dead polls before the timeout line).
server_dead=0
if ! $RCON "list" >/dev/null 2>&1; then
  server_dead=1
  echo "  [SERVER DEAD] rcon unreachable right after forceload tiles — lane ends early"
  dump_server_diag
  exit 0
fi
done=0
stall=0; lastc=-1; dumped=0; dead_polls=0
for i in $(seq 1 6000); do
  [ "$server_dead" = "1" ] && break
  c=$($RCON "crusgenstats wincount" 2>/dev/null | grep -o "window_inside=[0-9]*" | cut -d= -f2)
  if [ -z "$c" ]; then
    # transient empty (rcon hiccup) vs dead server: 40 consecutive dead
    # polls (~20-40s) = dead — dump diag once and stop polling
    dead_polls=$((dead_polls + 1))
    if [ "$dead_polls" -ge 40 ]; then
      server_dead=1
      echo "  [SERVER DEAD] 40 consecutive dead wincount polls (inside was $lastc / $target) — lane ends early"
      dump_server_diag
      exit 0
    fi
    sleep 0.5
    continue
  fi
  dead_polls=0
  c=${c:-0}
  if [ "$c" -ge "$target" ]; then done=1; break; fi
  # wedge probe: if the inside count stops moving for ~30s of polls, take
  # ONE jcmd thread dump (par4deco run 41214674243 wedged silently — the
  # log showed nothing between lane start and server death).
  if [ "$c" = "$lastc" ]; then
    stall=$((stall + 1))
  else
    stall=0; lastc=$c
  fi
  if [ "$stall" -ge 600 ] && [ "$dumped" = "0" ]; then
    dumped=1
    echo "  [WEDGE] inside=$c stalled ~30s — thread dump:"
    if [ -n "${JC:-}" ] && [ -n "${JPID:-}" ]; then
      "$JC" "$JPID" Thread.print 2>/dev/null | head -120 || echo "  thread dump failed"
    else
      echo "  (no jcmd/jpid in this lane)"
    fi
  fi
  if (( i % 600 == 0 )); then echo "  [window progress] inside=$c / $target"; fi
  sleep 0.05
done
t1=$(date +%s.%N)
# ---- cont-92 deco drain: the fill span closes at the last noise fill, but
# Paper's decorate phase (structures/features/populators) for the later
# chunks lands after it — every window so far printed "deco: no chunks
# decorated yet". The DecoMarker populator (always-on, both deco modes)
# timestamps decorate events; wait up to 30s beyond fill completion for
# window_deco_inside to reach the fill target, so the final GenStats
# snapshot carries deco_tail/e2e_deco and (JFR lanes) the profile covers
# the decorate tail. Best-effort: on grace timeout the snapshot still
# shows the partial deco_inside (diagnostic, not a lane failure).
dtarget=$target
dgrace=0
deco_last=-1
for i in $(seq 1 600); do
  d=$($RCON "crusgenstats wincount" 2>/dev/null | grep -o "window_deco_inside=[0-9]*" | cut -d= -f2)
  d=${d:-}
  # plugin without the cont-92 field: skip the drain entirely
  [ -z "$d" ] && break
  if [ "$d" -ge "$dtarget" ]; then dgrace=-1; break; fi
  if [ "$d" = "$deco_last" ]; then dgrace=$((dgrace + 1)); else dgrace=0; deco_last=$d; fi
  if (( dgrace >= 300 )); then echo "  [deco drain stalled 15s] deco_inside=$d / $dtarget — continuing"; break; fi
  if (( i % 100 == 0 )); then echo "  [deco drain] deco_inside=$d / $dtarget"; fi
  sleep 0.05
done
if [ -n "${d:-}" ] && [ "$d" -ge "$dtarget" ]; then
  echo "  [deco drain] complete: deco_inside=$d / $dtarget"
elif [ -n "${d:-}" ]; then
  echo "  [deco drain] partial at snapshot: deco_inside=${d:-0} / $dtarget (deco_tail may be understated)"
fi
echo "=== mspt (tick pacing right after window; last-100-ticks digest) ==="
$RCON "mspt" 2>/dev/null | head -6 || echo "mspt unavailable"
if [ "${CRUS_JFR:-0}" = "1" ] && [ -n "$JC" ] && [ -n "$JPID" ]; then
  "$JC" "$JPID" JFR.stop name=win >/dev/null 2>&1 || true
  JFR_BIN="$(dirname "$JAVA_BIN")/jfr"; [ -x "$JFR_BIN" ] || JFR_BIN=jfr
  echo "=== JFR hot methods (exactly the timed window) ==="
  "$JFR_BIN" view hot-methods "$JFR_FILE" 2>/dev/null | head -40 || echo "jfr view failed"
  echo "=== JFR per-thread attribution (infra spinners excluded) ==="
  "$JFR_BIN" print --events jdk.ExecutionSample --stack-depth 64 "$JFR_FILE" 2>/dev/null \
    | python3 "$ROOT/harness/jfr_top.py" | head -80 || echo "jfr print failed"
  echo "=== JFR native + park attribution (the non-Java wall: JNI fills + waiting) ==="
  "$JFR_BIN" print --events jdk.NativeMethodSample,jdk.ThreadPark,jdk.JavaMonitorWait --stack-depth 64 "$JFR_FILE" 2>/dev/null \
    | python3 "$ROOT/harness/jfr_wait.py" | head -60 || echo "jfr wait print failed"
fi
if [ "$done" != "1" ]; then
  echo "E2E WINDOW TIMEOUT (inside=$c / $target)"
  # alive-but-wedged or late death: the unfiltered run.out tail still names
  # the cause (exceptions, plugin errors, native warnings) — dump it here too
  [ "$server_dead" != "1" ] && dump_server_diag
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

# native run-phase profiler lives in run.out (log_warn = stderr): surface the
# last 16-chunk windows — decoRunFeature wall split + ore internals + top-10.
if [ -f "$SRV/run.out" ]; then
  echo "=== native run-prof (run.out tail; deco lanes only) ==="
  grep -h "run-prof\|ore-prof" "$SRV/run.out" 2>/dev/null | tail -14 || true
fi
