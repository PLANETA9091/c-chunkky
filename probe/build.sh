#!/usr/bin/env bash
# Build CrusstyProbe.jar (Bukkit-only probe plugin).
# Compiles against probe/paper-api.jar (Paper 1.21.8 API), packs plugin.yml inside.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# locate javac: PATH first, then local Temurin JDKs in /home/z/tools
JAVAC="${JAVAC:-$(command -v javac || true)}"
if [ -z "$JAVAC" ]; then
  JAVAC="$(ls /home/z/tools/jdk-*/bin/javac 2>/dev/null | sort | tail -1)"
fi
[ -n "$JAVAC" ] || { echo "ERROR: no javac found" >&2; exit 1; }
JARBIN="$(dirname "$JAVAC")/jar"
echo "using javac: $JAVAC"
LIBS="$(ls probe/libs/*.jar 2>/dev/null | tr '\n' ':' )"
CP="probe/paper-api.jar:$LIBS"
mkdir -p probe/classes
"$JAVAC" -encoding UTF-8 -cp "$CP" \
  -d probe/classes probe/src/com/planeta/crussty/probe/*.java
"$JARBIN" cf probe/CrusstyProbe.jar -C probe/classes . -C probe/resources plugin.yml
echo "built probe/CrusstyProbe.jar"
sha256sum probe/CrusstyProbe.jar
