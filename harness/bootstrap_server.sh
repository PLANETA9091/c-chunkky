#!/usr/bin/env bash
# Bootstrap a headless Paper server dir for the parity judge (CI or fresh local env).
# Creates ./server with paper.jar (paperclip), eula, server.properties, bukkit.yml.
# Idempotent: skips download if paper.jar exists.
#
# Usage: harness/bootstrap_server.sh [paper_build]
#   paper_build: pin a Paper 1.21.8 build number (default: pin used for the
#                reference dumps, fallback latest if that build disappears).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRV="$ROOT/server"
BUILD="${1:-60}"
VERSION="1.21.8"
SEED=3257840388504953787

mkdir -p "$SRV/plugins"

# --- paper.jar (paperclip launcher; extracts mojmap jar on first boot) -------
if [ ! -f "$SRV/paper.jar" ]; then
  API="https://api.papermc.io/v2/projects/paper/versions/$VERSION/builds"
  fetch() { # $1 = build id
    curl -sfL -o "$SRV/paper.jar" "$API/$1/downloads/paper-$VERSION-$1.jar"
  }
  echo "downloading Paper $VERSION build $BUILD ..."
  if ! fetch "$BUILD"; then
    echo "build $BUILD unavailable, falling back to latest"
    BUILD=$(curl -sf "$API" | python3 -c "import sys,json;print(json.load(sys.stdin)['builds'][-1]['build'])")
    fetch "$BUILD"
  fi
  echo "paper build: $BUILD"
  echo "$BUILD" > "$SRV/paper.build"
else
  echo "paper.jar present ($(md5sum "$SRV/paper.jar" | cut -d' ' -f1)), build=$(cat "$SRV/paper.build" 2>/dev/null || echo '?')"
fi

# --- eula -------------------------------------------------------------------
grep -q "eula=true" "$SRV/eula.txt" 2>/dev/null || echo "eula=true" > "$SRV/eula.txt"

# --- server.properties (deterministic judge config) -------------------------
PROPS="$SRV/server.properties"
cat > "$PROPS" <<EOF
level-name=vanilla
level-seed=$SEED
level-type=minecraft\:normal
allow-nether=false
allow-end=false
generate-structures=true
online-mode=false
enable-rcon=true
rcon.port=25575
rcon.password=crussty
sync-chunk-writes=true
view-distance=10
simulation-distance=10
spawn-protection=0
max-players=2
enable-command-block=false
motd=crussty judge
EOF

# --- bukkit.yml (generator wiring is rewritten by plugin_bench.sh per world) --
cat > "$SRV/bukkit.yml" <<EOF
settings:
  allow-end: false
EOF

echo "bootstrap done: $SRV"
