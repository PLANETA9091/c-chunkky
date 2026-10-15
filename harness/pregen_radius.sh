#!/usr/bin/env bash
# pregen_radius.sh <world> <radius_blocks>
#
# Square-radius pregen benchmark with the CrusstyGen plugin (end-to-end):
# waves of row-rect forceloads, per-batch completion via the probe plugin's
# exact /crusfull counter (FULLY loaded chunks inside the rect), per-batch
# rate log, final ms/chunk report. No parity judge — datapack worlds
# intentionally differ from the vanilla reference dumps.
#
# Env:
#   CRUS_FRESH=1          wipe the world dir first (deterministic runs)
#   DATAPACKS_DIR=DIR     copy *.zip into <world>/datapacks (priority: name order)
#   CRUS_HEAP             JVM heap override (forwarded to server_up.sh)
#   CRUS_FILL_THREADS     native fill threads (default 2)
#   CRUS_WAVE_ROWS        chunk-rows per wave batch (default 8)
#   CRUS_POLL_SEC         progress poll interval (default 3)
#   CRUS_SETTLE_SEC       post-pregen flush before final stats (default 20)
#   CRUS_PREGEN_MAX_MIN   global wall budget in minutes (default 300)
#
# Why /crusfull and not /crusgenstats chunks= for completion: the global
# counter ticks for PARTIAL neighbour chunks too (the noise stage runs on the
# ring around every marked chunk — measured: marking a 15x15 square ticked the
# counter by ~484, not 225), so counter arithmetic cannot express "rect done".
# isChunkLoaded()==FULL is the exact, per-chunk completion signal.
#
# Wave accounting:
#   - boot pregenerates the spawn area (Paper 1.21.8 spawn prep measured at
#     121 noise runs = 11x11, cx,cz in [-5..5]); those chunks are on disk when
#     the waves start and are EXCLUDED from the window via C_BOOT.
#   - main waves cover disjoint rows cz=CXMIN..CXMAX; each batch completes
#     when /crusfull over the batch rect == rect size (boot chunks re-load
#     from disk and count toward the rect). Exact => no holes, no stalls.
#   - NO separate spawn batch: marking a square around spawn right after boot
#     deterministically wedges ~29 edge holders (boot-prep holders stuck
#     mid-pipeline; reissues do not help) — the same chunks generate fine
#     inside waves a minute later, so the waves simply cover spawn too.
#   - Paper can LOSE a generation task for a ticket (known: fast native fill —
#     the judge saw the same; a repeated forceload add re-issues it,
#     idempotent). Poll loop REISSUES the batch rects when the count stalls,
#     and only FORCE-advances after ~90s.
#   - unique chunks generated in the window = TOTAL - boot_full (boot square),
#     ms/chunk = wall / unique.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

WORLD="${1:?usage: pregen_radius.sh <world> <radius_blocks>}"
RB="${2:?usage: pregen_radius.sh <world> <radius_blocks>}"
SEED=3257840388504953787
SRV=server
HOST=127.0.0.1 PORT=25575 PASS=crussty
SEG=128                                   # chunks per forceload cmd (vanilla cap 256)
ROWS_BATCH="${CRUS_WAVE_ROWS:-8}"
POLL="${CRUS_POLL_SEC:-3}"
SETTLE="${CRUS_SETTLE_SEC:-20}"
MAX_MS=$(( ${CRUS_PREGEN_MAX_MIN:-300} * 60000 ))
REPORT="reference/pregen_${WORLD}_r${RB}.txt"

export CRUS_FILL_THREADS="${CRUS_FILL_THREADS:-2}"

rcon() { python3 harness/rcon.py "$HOST" "$PORT" "$PASS" "$1"; }

# exact FULL-chunk count inside a block rect (probe plugin)
probe_full() {
  local r
  r=$(python3 harness/rcon.py "$HOST" "$PORT" "$PASS" "crusfull $1 $2 $3 $4" 2>/dev/null \
      | sed -n 's/^FULL=\([0-9]*\).*/\1/p' | head -1)
  echo "${r:-0}"
}

# global noise-stage counter (diagnostics only — counts partial neighbours too)
count_chunks() {
  python3 harness/rcon.py "$HOST" "$PORT" "$PASS" "crusgenstats" 2>/dev/null | python3 -c '
import sys, re
m = re.search(r"(?m)(?:^|\s)chunks=(\d+)", sys.stdin.read())
print(m.group(1) if m else 0)'
}

cleanup() {
  if [ -n "${RSSPID:-}" ] && [ "$RSSPID" != "0" ]; then kill "$RSSPID" 2>/dev/null || true; fi
  bash harness/server_down.sh >/dev/null 2>&1 || true
}
trap cleanup EXIT

# --- chunk square for the radius --------------------------------------------
read -r CXMIN CXMAX NSIDE TOTAL <<< "$(python3 -c "
r = $RB
cmin = -((r + 15) // 16)      # floor(-r/16)
cmax = (r - 1) // 16          # floor((r-1)/16)
print(cmin, cmax, cmax - cmin + 1, (cmax - cmin + 1) ** 2)")"
echo "[pregen] world=$WORLD radius=${RB} blocks => chunk square cx,cz in [$CXMIN..$CXMAX], total chunks=$TOTAL"

# --- deploy (same hygiene as plugin_bench: judge must be Gen+Probe only) ----
[ -f native/target/release/libcrussty_chunkgen.so ] || { echo "no native .so" >&2; exit 1; }
[ -f plugin/CrusstyGen.jar ] || { echo "no plugin jar" >&2; exit 1; }
mkdir -p "$SRV/plugins"
find "$SRV/plugins" -maxdepth 1 -name '*.jar' ! -name 'CrusstyProbe.jar' -delete
cp plugin/CrusstyGen.jar "$SRV/plugins/CrusstyGen.jar"
[ -f probe/CrusstyProbe.jar ] && cp probe/CrusstyProbe.jar "$SRV/plugins/CrusstyProbe.jar"
{
  echo "# deployed.md5 $(date -u +%Y-%m-%dT%H:%M:%SZ) (pregen_radius)"
  md5sum "$SRV/paper.jar" "$SRV/plugins/CrusstyGen.jar" "$SRV/plugins/CrusstyProbe.jar" native/target/release/libcrussty_chunkgen.so
} > "$SRV/deployed.md5"

# --- config -----------------------------------------------------------------
python3 - "$WORLD" <<'PY'
import sys
world = sys.argv[1]
open("server/bukkit.yml", "w").write(f"worlds:\n  {world}:\n    generator: CrusstyGen\n")
PY
sed -i "s/^level-name=.*/level-name=$WORLD/; s/^level-seed=.*/level-seed=$SEED/; s/^allow-nether=.*/allow-nether=false/; s/^sync-chunk-writes=.*/sync-chunk-writes=false/" "$SRV/server.properties"
grep -qE "^sync-chunk-writes=false" "$SRV/server.properties" || echo "sync-chunk-writes=false" >> "$SRV/server.properties"
grep -E "^(level-name|level-seed|sync-chunk-writes)=" "$SRV/server.properties"

if [ "${CRUS_FRESH:-0}" = "1" ]; then
  echo "--- wiping world dirs ($WORLD) ---"
  rm -rf "$SRV/$WORLD" "$SRV/${WORLD}_nether" "$SRV/${WORLD}_the_end"
fi
if [ -n "${DATAPACKS_DIR:-}" ]; then
  mkdir -p "$SRV/$WORLD/datapacks"
  cp -f "$DATAPACKS_DIR"/*.zip "$SRV/$WORLD/datapacks/" 2>/dev/null || { echo "no zips in $DATAPACKS_DIR" >&2; exit 1; }
  echo "datapacks wired:"; ls -la "$SRV/$WORLD/datapacks/"
fi
[ -f "${DATAPACKS_DIR:-/nonexistent}/versions.txt" ] && cat "$DATAPACKS_DIR/versions.txt" || true

# --- boot -------------------------------------------------------------------
bash harness/server_down.sh >/dev/null 2>&1 || true
pkill -9 -f 'paper\.jar' 2>/dev/null || true
sleep 3
bash harness/server_up.sh
SRVPID="$(cat "$SRV/server.pid")"
rm -f "$SRV/rss.log"
(
  while kill -0 "$SRVPID" 2>/dev/null; do
    RSS=$(ps -o rss= -p "$SRVPID" 2>/dev/null | tr -d ' ' || echo 0)
    echo "$(date -u +%T) $(( ${RSS:-0} / 1024 ))MB" >> "$SRV/rss.log"
    sleep 15
  done
) & RSSPID=$!

rcon "gamerule spawnChunkRadius 0" || true
rcon "forceload remove all" >/dev/null || true

# probe sanity: the completion signal depends on the crusfull command
PROBE_CHK=$(rcon "crusfull -16 -16 15 15" 2>/dev/null || true)
case "$PROBE_CHK" in
  FULL=*) echo "[pregen] probe sanity: $PROBE_CHK" ;;
  *) echo "[pregen][FATAL] probe has no crusfull — rebuild probe/CrusstyProbe.jar (got: '$PROBE_CHK')" >&2; exit 7 ;;
esac

B0_X1=-112; B0_Z1=-112; B0_X2=111; B0_Z2=111; B0_EXPECT=225   # 15x15 chunk square
B0="forceload add -112 -112 111 111"

REISSUE_CMDS=()
replay_cmds() {
  # ticket cycle: remove ALL forced tickets, then re-add the pending rects —
  # a fresh ticket unwedges holders stuck mid-generation (fresh task).
  rcon "forceload remove all" >/dev/null 2>&1 || true
  for CMD in "${REISSUE_CMDS[@]}"; do rcon "$CMD" >/dev/null 2>&1 || true; done
}

# wait_rect <x1> <z1> <x2> <z2> <expect> <label> — poll /crusfull until the
# rect is fully generated; REISSUE the pending forceloads on stalls, FORCE
# after 30 stable polls (~90s). Prints the final count; rc=1 on FORCE.
TIMEOUT_HIT=0
wait_rect() {
  local x1="$1" z1="$2" x2="$3" z2="$4" expect="$5" label="$6"
  local stable=0 prev=-1 cur=0 reissues=0
  while :; do
    cur=$(probe_full "$x1" "$z1" "$x2" "$z2")
    if [ "${cur:-0}" -ge "$expect" ] 2>/dev/null; then echo "$cur"; return 0; fi
    if [ "$cur" = "$prev" ]; then stable=$((stable + 1)); else stable=0; fi
    prev="$cur"
    # Paper can lose the gen task for a ticket; re-marking re-issues it.
    if [ "$stable" -gt 0 ] && [ $(( stable % 5 )) -eq 0 ]; then
      reissues=$((reissues + 1))
      echo "[pregen] wait_rect($label): REISSUE #$reissues (stable=$stable, $cur/$expect)" >&2
      replay_cmds
    fi
    if [ "$stable" -ge 30 ]; then
      echo "[pregen][FORCE] wait_rect($label): $cur/$expect after $reissues reissues — forcing" >&2
      tail -n 6 "$SRV/run.out" 2>/dev/null >&2 || true
      echo "$cur"; return 1
    fi
    echo "[pregen] wait_rect($label): full=$cur/$expect stable=$stable" >&2
    if [ "$(( $(date +%s%3N) - T0 ))" -gt "$MAX_MS" ]; then
      echo "[pregen][TIMEOUT] global wall budget exceeded — aborting waves" >&2
      TIMEOUT_HIT=1
      echo "$cur"; return 1
    fi
    sleep "$POLL"
  done
}

# --- boot measurement ---------------------------------------------------------
# Wait for the boot spawn prep to settle, then take the global counter as
# C_BOOT: the number of chunks whose noise stage already ran at boot (the
# 11x11 spawn prep). The probe CANNOT be used here — boot chunks are already
# UNLOADED at this point (spawn tickets removed), only on-disk data remains;
# they re-load (not re-generate) when batch0 marks them.
PREV=-1; STABLE=0
for i in $(seq 1 200); do
  CUR=$(count_chunks)
  if [ "$CUR" = "$PREV" ]; then STABLE=$((STABLE + 1)); else STABLE=0; fi
  PREV="$CUR"
  [ "$STABLE" -ge 3 ] && break
  sleep "$POLL"
done
C_BOOT="$PREV"
FULL_AT_BOOT=$(probe_full "$B0_X1" "$B0_Z1" "$B0_X2" "$B0_Z2")
echo "[pregen] boot pregen done: noise_runs=$C_BOOT (spawn prep ~11x11), still_loaded=$FULL_AT_BOOT/$B0_EXPECT"

# --- measurement window starts (no spawn batch: see header) -----------------
T0=$(date +%s%3N)
rcon "crusgenstats reset" >/dev/null || true
echo "[pregen] stats reset — measurement window starts (spawn area is covered by the waves)"

# --- main waves -------------------------------------------------------------
BATCH=0
NBATCHES=$(( (NSIDE + ROWS_BATCH - 1) / ROWS_BATCH ))
BATCHLOG=()
for (( CZ0 = CXMIN; CZ0 <= CXMAX; CZ0 += ROWS_BATCH )); do
  [ "$TIMEOUT_HIT" = "1" ] && break
  BATCH=$((BATCH + 1))
  CZ1=$(( CZ0 + ROWS_BATCH - 1 )); [ "$CZ1" -gt "$CXMAX" ] && CZ1=$CXMAX
  T_B0=$(date +%s%3N)
  RECT_X1=$(( CXMIN * 16 )); RECT_Z1=$(( CZ0 * 16 ))
  RECT_X2=$(( CXMAX * 16 + 15 )); RECT_Z2=$(( CZ1 * 16 + 15 ))
  EXPECT=$(( (CZ1 - CZ0 + 1) * NSIDE ))

  REISSUE_CMDS=()
  for (( CZ = CZ0; CZ <= CZ1; CZ++ )); do
    for (( SX = CXMIN; SX <= CXMAX; SX += SEG )); do
      SX1=$(( SX + SEG - 1 )); [ "$SX1" -gt "$CXMAX" ] && SX1=$CXMAX
      REISSUE_CMDS+=("forceload add $(( SX * 16 )) $(( CZ * 16 )) $(( SX1 * 16 + 15 )) $(( CZ * 16 + 15 ))")
    done
  done
  for CMD in "${REISSUE_CMDS[@]}"; do rcon "$CMD" >/dev/null || true; done

  CUR=$(wait_rect "$RECT_X1" "$RECT_Z1" "$RECT_X2" "$RECT_Z2" "$EXPECT" "batch$BATCH") || true

  T_B1=$(date +%s%3N)
  BATCH_MS=$(( T_B1 - T_B0 )); BATCH_N=$(( CUR - 0 )); [ "$BATCH_N" -le 0 ] && BATCH_N=1
  LINE=$(printf 'batch %3d/%d rows cz=%4d..%4d: %6d/%6d chunks in %7.1fs -> %6.2f ms/chunk (%7.1f chunks/s)' \
    "$BATCH" "$NBATCHES" "$CZ0" "$CZ1" "$CUR" "$EXPECT" \
    "$(python3 -c "print($BATCH_MS/1000)")" \
    "$(python3 -c "print($BATCH_MS/$BATCH_N)")" \
    "$(python3 -c "print($BATCH_N/($BATCH_MS/1000))")")
  echo "[pregen] $LINE"
  BATCHLOG+=("$LINE")

  for (( CZ = CZ0; CZ <= CZ1; CZ++ )); do
    for (( SX = CXMIN; SX <= CXMAX; SX += SEG )); do
      SX1=$(( SX + SEG - 1 )); [ "$SX1" -gt "$CXMAX" ] && SX1=$CXMAX
      rcon "forceload remove $(( SX * 16 )) $(( CZ * 16 )) $(( SX1 * 16 + 15 )) $(( CZ * 16 + 15 ))" >/dev/null || true
    done
  done
done
T1="${T1:-$(date +%s%3N)}"

# --- finalize ---------------------------------------------------------------
sleep "$SETTLE"
STATS=$(rcon "crusgenstats" 2>/dev/null || echo "stats unavailable")
DU=$(du -sh "$SRV/$WORLD" 2>/dev/null | cut -f1 || echo '?')
DF_LINE=$(df -h "$ROOT" | tail -1 | awk '{print $4" free"}')
PACKS_LINE=$(grep -i "datapack" "$SRV/run.out" 2>/dev/null | grep -iE "enabled|loading" | head -3 || true)
WALL=$(( T1 - T0 ))
GEN_UNIQUE=$(( TOTAL - C_BOOT ))
MPC=$(python3 -c "print($WALL / $GEN_UNIQUE)")
RATED=$(python3 -c "print($GEN_UNIQUE / ($WALL / 1000))")

kill "$RSSPID" 2>/dev/null || true
rcon "forceload remove all" >/dev/null 2>&1 || true
bash harness/server_down.sh || true

{
  echo "pregen report — CrusstyGen end-to-end square-radius pregen"
  echo "world=$WORLD radius=${RB} blocks  chunk_square=[$CXMIN..$CXMAX]  total_chunks=$TOTAL"
  echo "seed=$SEED  datapacks=${DATAPACKS_DIR:-none}"
  [ -n "$PACKS_LINE" ] && echo "server datapack lines: $PACKS_LINE" || true
  [ -f "${DATAPACKS_DIR:-/nonexistent}/versions.txt" ] && echo "pack versions: $(tr '\n' ' ' < "$DATAPACKS_DIR/versions.txt")" || true
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "boot_full_chunks_in_spawn_square=$C_BOOT (excluded from unique count)"
  echo "wall_ms=$WALL  chunks_generated=$GEN_UNIQUE  ms_per_chunk=$MPC  chunks_per_sec=$RATED"
  echo "timeout_hit=$TIMEOUT_HIT"
  echo ""
  echo "--- final crusgenstats (last 4096-chunk ring + [cpu]) ---"
  echo "$STATS"
  echo ""
  echo "--- per-batch rates ---"
  printf '%s\n' "${BATCHLOG[@]}"
  echo ""
  echo "world_size_on_disk=$DU  disk: $DF_LINE"
} | tee "$REPORT"

echo ""
echo "PREGEN RESULT: chunks=$GEN_UNIQUE wall_ms=$WALL ms_per_chunk=$MPC chunks_per_sec=$RATED timeout=$TIMEOUT_HIT"
