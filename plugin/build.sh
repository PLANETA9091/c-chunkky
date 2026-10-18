#!/usr/bin/env bash
# Build CrusstyGen plugin jar. Requires JDK 21 (PATH javac or /home/z/tools/jdk-*) and probe/paper-api.jar.
set -euo pipefail
cd "$(dirname "$0")"
API_JAR="../probe/paper-api.jar"
LIBS="$(ls ../probe/libs/*.jar 2>/dev/null | tr '\n' ':')"
[ -f "$API_JAR" ] || { echo "missing $API_JAR"; exit 1; }
JAVAC="${JAVAC:-$(command -v javac || true)}"
if [ -z "$JAVAC" ]; then
  # `|| true` guards against set -e -o pipefail killing the script when the
  # glob matches nothing (fresh sandbox without /home/z/tools/jdk-*).
  JAVAC="$(ls /home/z/tools/jdk-*/bin/javac 2>/dev/null | sort | tail -1 || true)"
fi
if [ -z "$JAVAC" ]; then
  # No launcher binaries (JRE-only sandbox): the jdk.compiler module exposes
  # the same entrypoints via `java -m module/mainclass`.
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
rm -rf classes
mkdir -p classes
# Main sources (paper-api only)
SRCS_MAIN=$(find src -name '*.java' ! -name 'NativeBiomeSource.java' ! -name 'BiomeSwap.java' ! -name 'SurfaceSetup.java' ! -name 'DecoDriver.java')
"${JAVAC_CMD[@]}" -encoding UTF-8 -cp "$API_JAR:$LIBS" -d classes $SRCS_MAIN

# NMS companions (NativeBiomeSource/BiomeSwap) need the mojmap paper jar —
# present only after harness/bootstrap_server.sh (CI integration job). When
# absent the plugin still builds and runs with vanilla Java biomes.
SERVER_JAR="../server/versions/1.21.8/paper-1.21.8.jar"
if [ -f "$SERVER_JAR" ]; then
  echo "compiling NMS biome companions against $SERVER_JAR"
  MOJANG_LIBS="$(find ../server/libraries/com/mojang ../server/libraries/it -name '*.jar' 2>/dev/null | tr '\n' ':')"
  "${JAVAC_CMD[@]}" -encoding UTF-8 -cp "classes:$SERVER_JAR:$API_JAR:$LIBS:$MOJANG_LIBS" -d classes \
    src/com/planeta/crussty/NativeBiomeSource.java src/com/planeta/crussty/BiomeSwap.java src/com/planeta/crussty/SurfaceSetup.java src/com/planeta/crussty/DecoDriver.java
else
  echo "no server jar — NMS biome companions skipped (vanilla Java biomes)"
fi
cp resources/plugin.yml classes/
# mojmap-mapped NMS references (NativeBiomeSource/BiomeSwap) — without this
# manifest marker Paper's plugin remapper treats the bytecode as spigot-mapped
# and garbles the NMS interface calls (NoSuchMethodError at runtime).
cat > classes/MANIFEST.MF << 'MF'
Manifest-Version: 1.0
paperweight-mappings-namespace: mojang

MF
"${JAR_CMD[@]}" --create --file CrusstyGen.jar --manifest classes/MANIFEST.MF -C classes .
echo "--- md5 (deploy hygiene: record in server/deployed.md5) ---"
md5sum CrusstyGen.jar
