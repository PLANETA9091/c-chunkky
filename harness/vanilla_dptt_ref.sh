#!/usr/bin/env bash
# cont-129: regenerate reference/dumps_plugin/dptt (vanilla Paper + packs
# ground truth for the dptt judge). The old local reference was POLLUTED:
# chunk_-1_-1/-1_-2 are byte-identical to native dumps (a misconfigured dump
# run at 09:34 overwrote the reference with native-side output).
#
# Protocol parity with judge_dptt.sh: same seed, same packs, same 8x8 region
# (forceload -64 -64 63 63 => chunks -4..3), same 15s settle, same dumper
# (crusstydumpregion via CrusstyProbe), NO CrusstyGen (pure vanilla gen).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

bash harness/server_down.sh >/dev/null 2>&1 || true
pkill -9 -f 'paper\.jar' 2>/dev/null || true
sleep 3
LEVEL=dpttref
rm -rf "server/$LEVEL" "server/${LEVEL}_nether" "server/${LEVEL}_the_end"
mkdir -p "server/$LEVEL/datapacks" server/plugins
cp server_datapacks/01_tectonic.zip server_datapacks/02_terralith.zip "server/$LEVEL/datapacks/"
rm -f server/plugins/CrusstyGen.jar
cp probe/CrusstyProbe.jar server/plugins/
: > server/bukkit.yml
sed -i "s/^level-name=.*/level-name=$LEVEL/; s/^level-seed=.*/level-seed=3257840388504953787/; s/^allow-nether=.*/allow-nether=false/" server/server.properties
grep -qE "^allow-nether=false" server/server.properties || echo "allow-nether=false" >> server/server.properties

cd server
if pgrep -f 'java .*-jar .*paper\.jar' >/dev/null 2>&1; then
  echo "ERROR: paper already running" >&2; exit 1
fi
rm -f run.in
mkfifo run.in
( sleep 28800 > run.in ) &
echo $! > fifo.pid
java -Xms512M -Xmx1536M -jar paper.jar nogui < run.in > run.out 2>&1 &
echo $! > server.pid
cd "$ROOT"

ok=0
for i in $(seq 1 150); do
  if grep -q 'Done (' server/run.out 2>/dev/null; then ok=1; break; fi
  sleep 1
done
[ "$ok" = "1" ] || { echo "SERVER BOOT FAILED"; tail -5 server/run.out; exit 1; }
sleep 2
python3 harness/rcon.py 127.0.0.1 25575 crussty "forceload add -64 -64 63 63" >/dev/null
# wait until the full 8x8 region is generated
for i in $(seq 1 40); do
  n=$(python3 harness/rcon.py 127.0.0.1 25575 crussty "crusfull -64 -64 63 63" 2>/dev/null \
      | grep -m1 -oE 'FULL=[0-9]+' | sed 's/^FULL=//' | cut -d/ -f1 | tail -1 || true)
  case "${n:-}" in ''|*[!0-9]*) n=0;; esac
  [ "${n}" -ge 64 ] && break
  [ $((i % 10)) = 0 ] && python3 harness/rcon.py 127.0.0.1 25575 crussty "forceload add -64 -64 63 63" >/dev/null || true
  sleep 2
done
echo "region full=$n/64"
sleep 15  # fluid-sim settle (same as judge)
mkdir -p reference/dumps_plugin/dptt
rm -f reference/dumps_plugin/dptt/chunk_*.crsf
python3 harness/rcon.py 127.0.0.1 25575 crussty "crusstydumpregion -4 -4 3 3 $ROOT/reference/dumps_plugin/dptt chunk $LEVEL" || true
sleep 3
ls reference/dumps_plugin/dptt/ | wc -l
bash harness/server_down.sh >/dev/null 2>&1 || true
echo DONE
