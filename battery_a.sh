#!/usr/bin/env bash
# Q6 idle-decomposition battery A: boot + honest-window experiments + shutdown.
# All results append to /home/z/my-project/audit_idle263/results.txt (crash-safe).
set -u
RES=/home/z/my-project/audit_idle263/results.txt
HRN=/home/z/my-project/c-faster/harness
R() { python3 $HRN/rcon.py localhost 25576 crussty "$1" >> "$RES" 2>&1; }
RP() { python3 $HRN/rcon.py localhost 25576 crussty "$1" 2>/dev/null; }
log() { echo "[$(date +%H:%M:%S)] $*" >> "$RES"; }

# --- sanity: no server already running
if pgrep -f 'paper-26\.3' >/dev/null 2>&1; then
  log "KILL leftover server first"
  bash $HRN/server263_down.sh >> "$RES" 2>&1
  sleep 2
fi

log "=== BOOT begin (JFR profile, worker=2, fill=2, deco=1, carvers=1, watchdog off) ==="
cd /home/z/my-project/c-faster
JVM_FLAGS_ADD="-XX:StartFlightRecording=filename=/home/z/my-project/audit_idle263/rec.jfr,settings=profile,dumponexit=true -Ddisable.watchdog=true" \
CRUS_NATIVE_DECO=1 CRUS_NATIVE_CARVERS=1 CRUS_FILL_THREADS=2 CRUS_WORKER_THREADS=2 \
  bash harness/server263_up.sh >> "$RES" 2>&1
sleep 3
ok=0
for i in $(seq 1 90); do
  if grep -q 'Done (' /home/z/my-project/c-faster/server263/run.out 2>/dev/null; then ok=1; break; end=1; fi
  sleep 2
done
if [ "$ok" != 1 ]; then log "BOOT FAILED — tail:"; tail -20 /home/z/my-project/c-faster/server263/run.out >> "$RES" 2>&1; bash $HRN/server263_down.sh >> "$RES" 2>&1; exit 1; fi
log "server up"

# --- warmup on ALREADY-GENERATED region (1000,1000) so fresh terrain untouched
log "--- WARMUP 13x13 @1000,1000 (regenerated region, fills expected ~0) ---"
R "crusgenstats reset"
R "forceload add 16000 16000 16032 16032"
sleep 4
R "crusgenstats"
sleep 2
R "crusgenstats"

# window experiment driver: $1 tag, $2 rx, $3 rz, $4 n, then forceload cmd via $5..$8 (block coords)
run_window() {
  local tag=$1 rx=$2 rz=$3 n=$4 x1=$5 z1=$6 x2=$7 z2=$8
  log "--- EXP $tag: window($rx,$rz,n=$n) forceload [$x1 $z1]->[$x2 $z2] ---"
  R "crusgenstats window $rx $rz $n"
  R "crusgenstats wincount"
  local t0; t0=$(date +%s.%N)
  R "forceload add $x1 $z1 $x2 $z2"
  local prev=0 prevd=0 stable=0
  for i in $(seq 1 80); do
    sleep 0.4
    local out c d
    out=$(RP "crusgenstats wincount" 2>/dev/null)
    c=$(echo "$out" | grep -oE "window_inside=[0-9]+" | head -1 | cut -d= -f2)
    d=$(echo "$out" | grep -oE "window_deco_inside=[0-9]+" | head -1 | cut -d= -f2)
    c=${c:-0}; d=${d:-0}
    local now; now=$(date +%s.%N)
    echo "  [$tag] t=$(awk -v a="$now" -v b="$t0" "BEGIN{printf \"%.1f\", a-b}")s inside=$c deco=$d" >> "$RES"
    if [ "$c" = "$prev" ] && [ "$d" = "$prevd" ] && [ "$c" -ge 1 ]; then
      stable=$((stable+1))
    else
      stable=0
    fi
    prev=$c; prevd=$d
    if [ $stable -ge 5 ]; then break; fi
  done
  R "crusgenstats"
  log "--- EXP $tag done ---"
}

# chunk->block helpers: chunk c -> block 16c .. 16c+15 (to-corner = 16*(cx+n+1)-1)
cb() { echo $(( $1 * 16 )); }

# E1: control 12x12 square @6200,6200 (fresh)
run_window E1-square-6200 6200 6200 11 $(cb 6200) $(cb 6200) $((16*6212-1)) $((16*6212-1))

# E2: 1x12 ROW x=6500, z 6400..6411 (window square armed, only row forceloaded)
run_window E2-row 6500 6400 11 $(cb 6500) $(cb 6400) $((16*6500+15)) $((16*6412-1))

# E3: 12x1 COLUMN z=6700, x 6400..6411 (window armed 6400,6700 n=11, only col forceloaded)
run_window E3-col 6400 6700 11 $(cb 6400) $(cb 6700) $((16*6412-1)) $((16*6700+15))

# E4: second 12x12 square @6900,6400 (fresh, later in boot -> less cold)
run_window E4-square-6900 6900 6400 11 $(cb 6900) $(cb 6400) $((16*6912-1)) $((16*6412-1))

# E5b: TWO DISJOINT 12x12 rects concurrent; window counts rectA @7200,7200;
# rectB @7400,7200 forceloaded FIRST then rectA immediately (~concurrent)
log "--- EXP E5b-concurrent: rectB(7400) submitted, then rectA(7200) ~0.15s later ---"
R "crusgenstats window 7200 7200 11"
t0=$(date +%s.%N)
R "forceload add $(cb 7400) $(cb 7200) $((16*7412-1)) $((16*7212-1))"
sleep 0.15
R "forceload add $(cb 7200) $(cb 7200) $((16*7212-1)) $((16*7212-1))"
prev=0; prevd=0; stable=0
for i in $(seq 1 120); do
  sleep 0.4
  out=$(RP "crusgenstats wincount" 2>/dev/null)
  c=$(echo "$out" | grep -oE "window_inside=[0-9]+" | head -1 | cut -d= -f2); c=${c:-0}
  d=$(echo "$out" | grep -oE "window_deco_inside=[0-9]+" | head -1 | cut -d= -f2); d=${d:-0}
  now=$(date +%s.%N)
  echo "  [E5b] t=$(awk -v a="$now" -v b="$t0" "BEGIN{printf \"%.1f\", a-b}")s inside=$c deco=$d" >> "$RES"
  if [ "$c" = "$prev" ] && [ "$d" = "$prevd" ] && [ "$c" -ge 1 ]; then stable=$((stable+1)); else stable=0; fi
  prev=$c; prevd=$d
  if [ $stable -ge 6 ]; then break; fi
done
R "crusgenstats"
R "forceload remove all"
sleep 1

# E5c: solo 12x12 @7600,7200 (paired control for E5b, same boot)
run_window E5c-solo-7600 7600 7200 11 $(cb 7600) $(cb 7200) $((16*7612-1)) $((16*7212-1))
R "forceload remove all"

log "=== BATTERY A done; stopping server (JFR dumps on exit) ==="
bash $HRN/server263_down.sh >> "$RES" 2>&1
sleep 3
ls -la /home/z/my-project/audit_idle263/rec.jfr >> "$RES" 2>&1
log "=== BATTERY A complete ==="
