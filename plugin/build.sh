#!/usr/bin/env bash
# Build CrusstyGen plugin jar. Requires JDK 21 (PATH javac or /home/z/tools/jdk-*) and probe/paper-api.jar.
set -euo pipefail
cd "$(dirname "$0")"
API_JAR="../probe/paper-api.jar"
LIBS="$(ls ../probe/libs/*.jar 2>/dev/null | tr '\n' ':')"
[ -f "$API_JAR" ] || { echo "missing $API_JAR"; exit 1; }
JAVAC="${JAVAC:-$(command -v javac || true)}"
if [ -z "$JAVAC" ]; then
  JAVAC="$(ls /home/z/tools/jdk-*/bin/javac 2>/dev/null | sort | tail -1)"
fi
[ -n "$JAVAC" ] || { echo "ERROR: no javac found" >&2; exit 1; }
JARBIN="$(dirname "$JAVAC")/jar"
echo "using javac: $JAVAC"
rm -rf classes
mkdir -p classes
"$JAVAC" -encoding UTF-8 -cp "$API_JAR:$LIBS" -d classes $(find src -name '*.java')
cp resources/plugin.yml classes/
"$JARBIN" cf CrusstyGen.jar -C classes .
echo "--- md5 (deploy hygiene: record in server/deployed.md5) ---"
md5sum CrusstyGen.jar
