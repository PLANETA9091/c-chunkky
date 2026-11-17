#!/usr/bin/env bash
# Boot diagnosis: start Paper with the plugin, capture a jstack thread dump
# ~50s after spawn-prep completes (or at Done), probe rcon, then stop.
# All inside ONE call (no background survives between calls).
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

bash harness/server_down.sh >/dev/null 2>&1 || true
pkill -9 -f 'paper\.jar' 2>/dev/null || true
sleep 2

JAR="plugin/CrusstyGen.jar"
SO="native/target/release/libcrussty_chunkgen.so"
find server/plugins -maxdepth 1 -name '*.jar' ! -name 'CrusstyProbe.jar' -delete
cp "$JAR" server/plugins/CrusstyGen.jar
[ -f probe/CrusstyProbe.jar ] && cp probe/CrusstyProbe.jar server/plugins/

python3 - dptt <<'PY'
import sys
open("server/bukkit.yml", "w").write(f"worlds:\n  {sys.argv[1]}:\n    generator: CrusstyGen\n")
PY
sed -i "s/^level-name=.*/level-name=dptt/; s/^level-seed=.*/level-seed=3257840388504953787/; s/^allow-nether=.*/allow-nether=false/" server/server.properties
rm -rf server/dptt server/dptt_nether server/dptt_the_end
mkdir -p server/dptt/datapacks
cp -r server_datapacks/. server/dptt/datapacks/
rm -f server/run.out

cd server
rm -f run.in
mkfifo run.in
( sleep 28800 > run.in ) >/dev/null 2>&1 &
echo $! > fifo.pid
export CRUS_FILL_THREADS=2 CRUS_PROFILE=1
JSTACK=/home/z/tools/jdk-21.0.5+11/bin/jstack
java -Xms512M -Xmx1536M \
  -Djava.library.path="$ROOT/native/target/release" \
  -Dpaper.chunk-system.gen-parallelism=false \
  -jar paper.jar nogui < run.in > run.out 2>&1 &
PID=$!
echo "paper pid=$PID"
cd "$ROOT"

# wait for spawn-prep done or Done (max 240s)
SEEN_DONE=0
for i in $(seq 1 120); do
  if grep -q '^Done (' server/run.out 2>/dev/null || grep -q "Time elapsed" server/run.out 2>/dev/null; then
    break
  fi
  sleep 2
done
grep -q '^Done (' server/run.out && SEEN_DONE=1
echo "=== after wait: SEEN_DONE=$SEEN_DONE; last 6 lines ==="
tail -6 server/run.out

# give the stall 50s to develop, then dump threads via SIGQUIT (goes to run.out)
sleep 15
grep -q '^Done (' server/run.out && SEEN_DONE=1
echo "=== pre-dump SEEN_DONE=$SEEN_DONE ==="
kill -3 "$PID"
sleep 6
awk '/Full thread dump/,0' server/run.out > boot_thread_dump.txt
echo "=== thread dump saved ($(wc -l < boot_thread_dump.txt) lines) ==="

# rcon probe
python3 harness/rcon.py 127.0.0.1 25575 crussty crusstatus 2>&1 | head -6 || echo "RCON FAILED"

grep -q '^Done (' server/run.out && SEEN_DONE=1
echo "=== final SEEN_DONE=$SEEN_DONE ==="
tail -4 server/run.out

kill "$PID" 2>/dev/null; sleep 3; kill -9 "$PID" 2>/dev/null
echo "=== server stopped; diag complete ==="
