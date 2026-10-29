#!/usr/bin/env bash
# cont-54: A/B for the BiomeManager fiddle fix (sulfur-band Y-boundary gap).
# Region 16320..16575 = chunks 1020..1036 (17x17) — includes the cont-53
# divergent chunks (1022,1022) and (1034,1033). Fresh world per run,
# blockdump (per-chunk FNV) + statdump (per-chunk material counts).
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
RCON="python3 harness/rcon.py localhost 25576 crussty"
SRV="$ROOT/server263"

wipe_world() { rm -rf "$SRV/world" "$SRV/world_nether" "$SRV/world_the_end"; }

run_one() {  # $1 = dump prefix, $2 = extra env ("" or CRUS_NO_NATIVE_SURFACE_263=1)
  bash harness/server263_down.sh >/dev/null 2>&1
  wipe_world
  env $2 bash harness/server263_up.sh >/dev/null 2>&1
  for i in $(seq 1 90); do
    rg -q "Done \(" "$SRV/run.out" 2>/dev/null && break
    sleep 5
  done
  $RCON "forceload add 16320 16320 16575 16575" >/dev/null 2>&1
  sleep 150
  $RCON "blockdump $1.txt" >/dev/null 2>&1
  sleep 3
  $RCON "statdump ${1}_stat.txt" >/dev/null 2>&1
  sleep 3
  bash harness/server263_down.sh >/dev/null 2>&1
  echo "$1: $(wc -l < "$SRV/$1.txt" 2>/dev/null || echo MISSING) chunk lines, $(wc -l < "$SRV/${1}_stat.txt" 2>/dev/null || echo MISSING) stat lines"
}

run_one "ab54_native" ""
run_one "ab54_vsurf" "CRUS_NO_NATIVE_SURFACE_263=1"

echo "=== blockdump diff ==="
diff "$SRV/ab54_native.txt" "$SRV/ab54_vsurf.txt" > /tmp/ab54.diff && echo "IDENTICAL" || {
  echo "divergent chunks: $(rg -c '^[<>]' /tmp/ab54.diff || true)"
  head -8 /tmp/ab54.diff
}
echo "=== sulfur/cinnabar material deltas (native vs vanilla-surface) ==="
python3 - << 'PYEOF'
import re, collections
def load(path):
    totals = collections.Counter()
    per_chunk = {}
    for line in open(path):
        parts = line.split()
        if len(parts) < 2: continue
        cx, cz = parts[0], parts[1]
        counts = {}
        for tok in parts[2:]:
            m = re.match(r'([A-Za-z_]+):(\d+)', tok)
            if m:
                counts[m.group(1)] = int(m.group(2))
                totals[m.group(1)] += int(m.group(2))
        per_chunk[(cx, cz)] = counts
    return totals, per_chunk
t_n, p_n = load('server263/ab54_native_stat.txt')
t_v, p_v = load('server263/ab54_vsurf_stat.txt')
for mat in ['SULFUR', 'CINNABAR', 'STONE', 'DEEPSLATE', 'GRASS_BLOCK', 'DIRT']:
    print(f"{mat:12s} native={t_n.get(mat,0):8d} vanilla={t_v.get(mat,0):8d} delta={t_n.get(mat,0)-t_v.get(mat,0):+7d}")
# per-chunk deltas on sulfur-family mats
sf = ['SULFUR', 'CINNABAR']
worst = []
for k in p_n:
    d = sum(p_n[k].get(m, 0) - p_v.get(k, {}).get(m, 0) for m in sf)
    if d != 0: worst.append((abs(d), d, k))
worst.sort(reverse=True)
print("worst sulfur/cinnabar per-chunk deltas:", worst[:6])
PYEOF
