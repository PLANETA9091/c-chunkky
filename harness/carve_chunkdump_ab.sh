#!/usr/bin/env bash
# chunkdump A/B for two divergent chunks (cont-151 diagnostics).
set -uo pipefail
ROOT=/home/z/my-project/c-faster
SRV=/home/z/my-project/server263
JAVA=/home/z/tools/jdk-25.0.5+8/bin/java
RCON="python3 $ROOT/harness/rcon.py localhost 25576 crussty"
CHUNKS="${CHUNKS:-1026,1026 1026,1024}"

boot_leg() { # $1 = tag, $2 = env
  cd "$SRV"
  rm -rf world world_nether world_the_end
  cp "$ROOT/plugin/CrusstyGen.jar" plugins/CrusstyGen.jar
  cp "$ROOT/probe263/CrusstyProbe263.jar" plugins/CrusstyProbe263.jar
  cat > bukkit.yml <<'EOF'
worlds:
  world:
    generator: CrusstyGen
EOF
  grep -q "eula=true" eula.txt 2>/dev/null || echo "eula=true" > eula.txt
  rm -f run.in
  mkfifo run.in
  ( sleep 1200 > run.in ) >/dev/null 2>&1 &
  (
    cd "$SRV"
    env $2 CRUS_NATIVE_DECO=1 \
      setsid $JAVA -Xms2G -Xmx3G \
      -Djava.library.path="$ROOT/native/target/release" \
      -jar paper.jar nogui < run.in > run_ab_$1.out 2>&1 &
  )
  for i in $(seq 1 60); do
    grep -q "Done (" "$SRV/run_ab_$1.out" 2>/dev/null && break
    sleep 3
  done
  grep -q "Done (" "$SRV/run_ab_$1.out" || { echo "BOOT FAILED ($1)"; tail -5 "$SRV/run_ab_$1.out"; exit 1; }
  echo "booted ($1)"
  $RCON "forceload add 16384 16384 16511 16511" >/dev/null
  sleep 40
  for c in $CHUNKS; do
    cx=${c%,*}; cz=${c#*,}
    $RCON "chunkdump $cx $cz $ROOT/ab263_out_carvers7/chunk_${1}_${cx}_${cz}.txt" >/dev/null
  done
  $RCON "forceload remove all" >/dev/null
  sleep 2
  echo stop > run.in
  sleep 3
  pkill -9 -f 'paper-26\.3|paper\.jar' 2>/dev/null
  sleep 1
}

boot_leg on "CRUS_NATIVE_CARVERS=1"
boot_leg off "CRUS_NATIVE_CARVERS=0"
echo "DUMPS COMPLETE"
ls -la $ROOT/ab263_out_carvers7/chunk_*.txt 2>/dev/null
