#!/usr/bin/env bash
# Build the vanilla-lane trace agent (probe/agent/agent.jar, fat with ASM).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
CP="$HERE/../paper-api.jar"
# paper-api has the NMS types? No: agent only needs asm + stdlib (weaving is
# string-based); compile against asm + a minimal stub set via the paper jar.
PAPER="$HERE/../../server/versions/1.21.8/paper-1.21.8.jar"
[ -f "$PAPER" ] || PAPER=/tmp/my-project/paper/versions/1.21.8/paper-1.21.8.jar

OUT="$HERE/classes"
rm -rf "$OUT"
mkdir -p "$OUT"
java -m jdk.compiler/com.sun.tools.javac.Main \
  -cp "$HERE/asm.jar:$HERE/asm-tree.jar:$HERE/asm-commons.jar" \
  -d "$OUT" "$HERE"/src/com/planeta/crussty/agent/*.java

# fat jar: our classes + asm packages
JAR="$HERE/agent.jar"
rm -f "$JAR"
mkdir -p "$OUT/META-INF"
cat > "$OUT/META-INF/MANIFEST.MF" <<'EOF'
Manifest-Version: 1.0
Premain-Class: com.planeta.crussty.agent.VanTraceAgent
Boot-Class-Path: agent.jar
EOF
cd "$OUT"
java -m jdk.jartool/sun.tools.jar.Main cfm "$JAR" META-INF/MANIFEST.MF .
cd "$HERE"
mkdir -p /tmp/asm_unzip && cd /tmp/asm_unzip && rm -rf org
unzip -o -q "$HERE/asm.jar" -d /tmp/asm_unzip
cd /tmp/asm_unzip && java -m jdk.jartool/sun.tools.jar.Main uf "$JAR" org
echo "[agent] built $JAR"
