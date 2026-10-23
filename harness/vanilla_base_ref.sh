#!/usr/bin/env bash
# Vanilla ground-truth for base-height parity: boot a PURE VANILLA server
# (no CrusstyGen) with the dptt datapacks, forceload a few chunks, dump them.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

bash harness/server_down.sh >/dev/null 2>&1 || true
pkill -9 -f 'paper\.jar' 2>/dev/null || true
sleep 2
LEVEL=vbase
rm -rf "server/$LEVEL"
mkdir -p "server/$LEVEL/datapacks" server/plugins
cp server_datapacks/01_tectonic.zip server_datapacks/02_terralith.zip "server/$LEVEL/datapacks/"
rm -f server/plugins/CrusstyGen.jar
cp probe/CrusstyProbe.jar server/plugins/
: > server/bukkit.yml
sed -i "s/^level-name=.*/level-name=$LEVEL/; s/^level-seed=.*/level-seed=3257840388504953787/; s/^allow-nether=.*/allow-nether=false/" server/server.properties

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

for i in $(seq 1 120); do
  if grep -q 'Done (' server/run.out 2>/dev/null; then break; fi
  sleep 1
done
grep -q 'Done (' server/run.out || { echo "SERVER BOOT FAILED"; tail -5 server/run.out; exit 1; }
sleep 2
python3 harness/rcon.py 127.0.0.1 25575 crussty "forceload add 512 512 527 527" >/dev/null
python3 harness/rcon.py 127.0.0.1 25575 crussty "forceload add 0 0 15 15" >/dev/null
sleep 6
python3 harness/rcon.py 127.0.0.1 25575 crussty "crusfull 512 512 527 527 $LEVEL"
python3 harness/rcon.py 127.0.0.1 25575 crussty "crusfull 0 0 15 15 $LEVEL"
python3 harness/rcon.py 127.0.0.1 25575 crussty "crusbase 512 512 1023 1023 16 reference/base_dump_vanilla.txt $LEVEL" >/dev/null
python3 harness/rcon.py 127.0.0.1 25575 crussty "crusbase 0 0 255 255 16 reference/base_dump_vanilla_spawn.txt $LEVEL" >/dev/null
sleep 1
for i in $(seq 1 60); do
  [ -f server/reference/base_dump_vanilla_spawn.txt ] && tail -1 server/reference/base_dump_vanilla_spawn.txt | grep -q DONE && break
  sleep 2
done
python3 harness/rcon.py 127.0.0.1 25575 crussty "crusstydump 32 32 reference/van_32_32.crsf $LEVEL"
python3 harness/rcon.py 127.0.0.1 25575 crussty "crusstydump 0 0 reference/van_0_0.crsf $LEVEL"
sleep 2
echo "--- dump tails ---"
tail -1 server/reference/base_dump_vanilla.txt 2>/dev/null
tail -1 server/reference/base_dump_vanilla_spawn.txt 2>/dev/null
bash harness/server_down.sh >/dev/null 2>&1 || true
echo DONE
