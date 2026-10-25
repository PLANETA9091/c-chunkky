#!/usr/bin/env bash
# Build CrusstyProbe263.jar against the 26.3 paper-api (bukkit API only).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
API_JAR="${API_JAR:-server263/libraries/io/papermc/paper/paper-api/26.3.build.222-stable/paper-api-26.3.build.222-stable.jar}"
JAVAC="${JAVAC:-$(command -v javac || true)}"
[ -z "$JAVAC" ] && JAVAC="$(ls /home/z/tools/jdk-*/bin/javac 2>/dev/null | sort | tail -1 || true)"
[ -z "$JAVAC" ] && JAVAC="java -m jdk.compiler/com.sun.tools.javac.Main"
read -ra JAVAC_CMD <<< "$JAVAC"
if [ "${JAVAC_CMD[0]}" = "java" ]; then
  JAR_CMD=(java -m jdk.jartool/sun.tools.jar.Main)
else
  JAR_CMD=("$(dirname "${JAVAC_CMD[0]}")/jar")
fi
[ -f "$API_JAR" ] || { echo "missing $API_JAR"; exit 1; }
ADV="$(find server263/libraries -name 'adventure-api*.jar' -o -name 'adventure-key*.jar' -o -name 'examination-api*.jar' -o -name 'bungeecord-chat*.jar' 2>/dev/null | tr '\n' ':')"
rm -rf probe263/classes && mkdir -p probe263/classes
"${JAVAC_CMD[@]}" -encoding UTF-8 -cp "$API_JAR:$ADV" -d probe263/classes \
  probe263/src/com/planeta/crussty/probe/ProbePlugin.java
cp probe263/resources/plugin.yml probe263/classes/
"${JAR_CMD[@]}" --create --file probe263/CrusstyProbe263.jar -C probe263/classes .
echo "built probe263/CrusstyProbe263.jar"
