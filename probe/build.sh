#!/usr/bin/env bash
# Build CrusstyProbe.jar (Bukkit-only probe plugin).
# Compiles against probe/paper-api.jar (Paper 1.21.8 API), packs plugin.yml inside.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# locate javac: PATH first, then local Temurin JDKs in /home/z/tools, then
# the JDK's compiler module (JRE-only sandboxes: java -m jdk.compiler/...)
JAVAC="${JAVAC:-$(command -v javac || true)}"
if [ -z "$JAVAC" ]; then
  JAVAC="$(ls /home/z/tools/jdk-*/bin/javac 2>/dev/null | sort | tail -1 || true)"
fi
if [ -z "$JAVAC" ]; then
  JAVAC="java -m jdk.compiler/com.sun.tools.javac.Main"
fi
# Multi-word launchers ("java -m ...") must word-split into a command array.
read -ra JAVAC_CMD <<< "$JAVAC"
if [ "${JAVAC_CMD[0]}" = "java" ]; then
  JAR_CMD=(java -m jdk.jartool/sun.tools.jar.Main)
else
  JAR_CMD=("$(dirname "${JAVAC_CMD[0]}")/jar")
fi
echo "using javac: ${JAVAC_CMD[*]}"
LIBS="$(ls probe/libs/*.jar 2>/dev/null | tr '\n' ':')"
CP="probe/paper-api.jar:$LIBS"
mkdir -p probe/classes
"${JAVAC_CMD[@]}" -encoding UTF-8 -cp "$CP" \
  -d probe/classes probe/src/com/planeta/crussty/probe/*.java
"${JAR_CMD[@]}" cf probe/CrusstyProbe.jar -C probe/classes . -C probe/resources plugin.yml
echo "built probe/CrusstyProbe.jar"
sha256sum probe/CrusstyProbe.jar
