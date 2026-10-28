#!/usr/bin/env bash
# Build CrusstyProbe263.jar against the 26.3 paper-api (bukkit API only).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
SRV_DIR="${SRV_DIR:-server263}"
# paper-api jar name is build-pinned (26.3.build.<N>-stable) — resolve by glob,
# explicit env wins
API_JAR="${API_JAR:-$(find "$SRV_DIR/libraries/io/papermc/paper/paper-api" -name 'paper-api-*.jar' 2>/dev/null | sort | tail -1)}"
JAVAC="${JAVAC:-$(command -v javac || true)}"
[ -z "$JAVAC" ] && JAVAC="$(ls /home/z/tools/jdk-*/bin/javac 2>/dev/null | sort | tail -1 || true)"
[ -z "$JAVAC" ] && JAVAC="java -m jdk.compiler/com.sun.tools.javac.Main"
read -ra JAVAC_CMD <<< "$JAVAC"
if [ "${JAVAC_CMD[0]}" = "java" ]; then
  JAR_CMD=(java -m jdk.jartool/sun.tools.jar.Main)
else
  JAR_CMD=("$(dirname "${JAVAC_CMD[0]}")/jar")
fi
[ -n "$API_JAR" ] && [ -f "$API_JAR" ] || { echo "missing paper-api under $SRV_DIR/libraries"; exit 1; }
ADV="$(find "$SRV_DIR/libraries" -name 'adventure-api*.jar' -o -name 'adventure-key*.jar' -o -name 'examination-api*.jar' -o -name 'bungeecord-chat*.jar' 2>/dev/null | tr '\n' ':')"
# 26.3 paper-api signatures carry jetbrains/jspecify annotations — javac
# needs them on the classpath or it dies with "cannot access NotNull"
ANNOT="$(find "$SRV_DIR/libraries" -name 'jetbrains-annotations*.jar' -o -name 'annotations-*.jar' -o -name 'jspecify*.jar' 2>/dev/null | tr '\n' ':')"
# org.jetbrains.annotations (NotNull etc.) is ALWAYS pulled from Maven —
# the extraction's single annot jar is jspecify, which lacks NotNull
JB="/tmp/jb-annotations.jar"
[ -f "$JB" ] || curl -sfL -o "$JB" "https://repo1.maven.org/maven2/org/jetbrains/annotations/26.0.2-1/annotations-26.0.2-1.jar" \
  || curl -sfL -o "$JB" "https://repo1.maven.org/maven2/org/jetbrains/annotations/24.1.0/annotations-24.1.0.jar"
[ -f "$JB" ] && ANNOT="$JB:$ANNOT"
echo "probe263 classpath: api=$(basename "$API_JAR") adv=$(echo "$ADV" | tr ':' '\n' | grep -c .) annot=$(echo "$ANNOT" | tr ':' '\n' | grep -c .)"
rm -rf probe263/classes && mkdir -p probe263/classes
"${JAVAC_CMD[@]}" -encoding UTF-8 -cp "$API_JAR:$ADV:$ANNOT" -d probe263/classes \
  probe263/src/com/planeta/crussty/probe/ProbePlugin.java
cp probe263/resources/plugin.yml probe263/classes/
"${JAR_CMD[@]}" --create --file probe263/CrusstyProbe263.jar -C probe263/classes .
echo "built probe263/CrusstyProbe263.jar"
