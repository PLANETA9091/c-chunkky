#!/usr/bin/env bash
# Build CrusstyOracle.jar against the mojmap server jar (NOT paper-api).
set -euo pipefail
cd "$(dirname "$0")"
SRV_JAR="../server/versions/1.21.8/paper-1.21.8.jar:../probe/paper-api.jar"
# Paper's bundled Mojang libraries (datafixerupper etc.) live in
# server/libraries after paperclip extraction — needed for BuiltInRegistries.
# NOTE: only com/mojang jars — net/kyori jars from libraries shadow the
# probe/libs adventure jars and break javac resolution.
LIBS="$(ls ../probe/libs/*.jar 2>/dev/null | tr '\n' ':')$(find ../server/libraries/com/mojang ../server/libraries/it -name '*.jar' 2>/dev/null | tr '\n' ':')"
JAVAC="${JAVAC:-$(command -v javac || true)}"
if [ -z "$JAVAC" ]; then
  # JRE-only sandbox: the jdk.compiler module exposes the same entrypoints
  JAVAC="java -m jdk.compiler/com.sun.tools.javac.Main"
fi
if [ "${JAVAC%% *}" = "java" ]; then
  JARBIN_CMD=(java -m jdk.jartool/sun.tools.jar.Main)
else
  JARBIN_CMD=("$(dirname "${JAVAC%% *}")/jar")
fi
rm -rf classes && mkdir -p classes
read -ra JAVAC_CMD <<< "$JAVAC"
"${JAVAC_CMD[@]}" -encoding UTF-8 -cp "$SRV_JAR:$LIBS" -d classes $(find src -name '*.java')
cat > classes/MANIFEST.MF << 'MF'
Manifest-Version: 1.0
paperweight-mappings-namespace: mojang

MF
cp resources/plugin.yml classes/
"${JARBIN_CMD[@]}" --create --file CrusstyOracle.jar --manifest classes/MANIFEST.MF -C classes plugin.yml -C classes com
echo "built CrusstyOracle.jar"
