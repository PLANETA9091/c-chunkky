#!/usr/bin/env bash
# sandbox_recover.sh — one-shot recovery after a sandbox rollback (cont-120).
#
# The dev sandbox periodically resets: the c-faster checkout reverts to an old
# commit, and $HOME-hosted tooling (rustup/cargo, JDK25) plus $SRV263 (paper
# 26.3 server layout) vanish. Every step of the manual recovery chain used in
# cont-117..cont-119 is encoded here, idempotent: each stage checks for its
# output artifact and is skipped when fresh. Run it repeatedly (a Bash-tool
# timeout mid-way is fine — the next run resumes at the first missing piece).
#
# Stage map (with the hard-won details):
#   1. git     — fetch + reset --hard origin/main (drop stale-checkout dirt)
#   2. cargo   — rustup stable/minimal (sandbox PATH loses ~/.cargo/bin)
#   3. JDK25   — Temurin 25.0.5+8 into /home/z/tools (fixed URL, tar -xzf)
#   4. paper   — fill v3 API ONLY (v2 is sunset: {"error":"sunset"}); the
#                download key is "server:default"; sha256-pinned; then
#                paperclip.patchonly extracts versions/26.3 + libraries
#   5. native  — cargo build --release (incremental, safe to re-run)
#   6. plugin  — build.sh with ABSOLUTE SRV_DIR; tracked jar is restored after
#                (git checkout -- plugin/) to keep the tree clean
#   7. probe   — jetbrains-annotations must pre-come from the aliyun mirror:
#                repo1.maven.org rate-limits (HTTP 429) and build.sh's
#                internal fallback dies silently under set -e
#   8. server  — versions+libraries copied with sentinels (no cp nesting),
#                then LEVEL_NAME=world bootstrap_server.sh <build>
#   9. sanity  — tool versions + artifact hashes printed for the worklog
#
# Usage: bash harness/sandbox_recover.sh [--skip-git]
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

JDK_DIR=/home/z/tools/jdk-25.0.5+8
JDK_URL="https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25.0.5%2B8/OpenJDK25U-jdk_x64_linux_hotspot_25.0.5_8.tar.gz"
JDK_TGZ=/home/z/dl/jdk25.tar.gz
SRV263=/home/z/my-project/server263
PAPER_BUILD=222
PAPER_SHA256=f649771a2190214306fb0cfba22c7b2cbac5df6308b1158309593603ed07d488
PAPER_MD5=f49e299618f021d813b0ac69ec11e03d
JB=/tmp/jb-annotations.jar
JB_ALIYUN="https://maven.aliyun.com/repository/public/org/jetbrains/annotations/24.1.0/annotations-24.1.0.jar"
JB_CENTRAL="https://repo1.maven.org/maven2/org/jetbrains/annotations/24.1.0/annotations-24.1.0.jar"

stage() { echo ""; echo "=== [$1] $(date -u +%H:%M:%S) ==="; }
die() { echo "RECOVER FAILED at [$1]: $2" >&2; exit 1; }

# ---- 1. git sync -----------------------------------------------------------
if [ "${1:-}" != "--skip-git" ]; then
  stage git
  git fetch origin 2>&1 | tail -1
  LOCAL=$(git rev-parse HEAD); REMOTE=$(git rev-parse origin/main)
  if [ "$LOCAL" != "$REMOTE" ]; then
    git reset --hard origin/main | tail -1
    echo "reset: $LOCAL -> $REMOTE"
  else
    echo "git in sync: $LOCAL"
  fi
fi

# ---- 2. cargo --------------------------------------------------------------
stage cargo
export PATH="$HOME/.cargo/bin:$PATH"
if command -v cargo >/dev/null 2>&1; then
  echo "cargo present: $(cargo --version)"
else
  curl -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh || die cargo "rustup-init download"
  sh /tmp/rustup-init.sh -y --default-toolchain stable --profile minimal >/tmp/rustup.log 2>&1 \
    || die cargo "rustup install (see /tmp/rustup.log)"
  echo "cargo installed: $(cargo --version)"
fi

# ---- 3. JDK25 --------------------------------------------------------------
stage jdk25
if [ -x "$JDK_DIR/bin/java" ]; then
  echo "JDK25 present: $($JDK_DIR/bin/java -version 2>&1 | head -1)"
else
  mkdir -p /home/z/tools /home/z/dl
  [ -s "$JDK_TGZ" ] || curl -sL -o "$JDK_TGZ" "$JDK_URL" || die jdk25 "tarball download"
  tar -xzf "$JDK_TGZ" -C /home/z/tools || die jdk25 "tar -xzf"
  [ -x "$JDK_DIR/bin/java" ] || die jdk25 "java not executable after extract"
  echo "JDK25 installed: $($JDK_DIR/bin/java -version 2>&1 | head -1)"
fi

# ---- 4. paper 26.3 (fill v3, sha256-pinned) --------------------------------
stage paper
mkdir -p "$SRV263"
NEED_JAR=1
if [ -s "$SRV263/paper.jar" ]; then
  GOT=$(sha256sum < "$SRV263/paper.jar" | cut -d' ' -f1)
  if [ "$GOT" = "$PAPER_SHA256" ]; then NEED_JAR=0; echo "paper.jar sha256 OK ($GOT)"; fi
fi
if [ "$NEED_JAR" = "1" ]; then
  URL=$(curl -sL "https://fill.papermc.io/v3/projects/paper/versions/26.3/builds/$PAPER_BUILD" \
        | python3 -c "import json,sys; print(json.load(sys.stdin)['downloads']['server:default']['url'])")
  [ -n "$URL" ] || die paper "fill v3 gave no url (v2 is sunset — do not fall back to it)"
  curl -sL -o "$SRV263/paper.jar" "$URL" || die paper "fill-data download"
  GOT=$(sha256sum < "$SRV263/paper.jar" | cut -d' ' -f1)
  [ "$GOT" = "$PAPER_SHA256" ] || die paper "sha256 mismatch: $GOT"
  echo "paper.jar downloaded (build $PAPER_BUILD), sha256 OK"
fi
if [ ! -f "$SRV263/versions/26.3/paper-26.3.jar" ]; then
  (cd "$SRV263" && "$JDK_DIR/bin/java" -Dpaperclip.patchonly=true -jar paper.jar >/tmp/patchonly.log 2>&1) \
    || die paper "patchonly extract (see /tmp/patchonly.log)"
  echo "patchonly extracted versions/26.3 + libraries"
else
  echo "versions/26.3 present"
fi

# ---- 5. native .so ---------------------------------------------------------
stage native
(cd native && cargo build --release 2>&1 | grep -E "Finished|error" | tail -1)
[ -f native/target/release/libcrussty_chunkgen.so ] || die native ".so missing after build"
echo ".so OK: $(ls -la native/target/release/libcrussty_chunkgen.so | awk '{print $5}') bytes"

# ---- 6. plugin -------------------------------------------------------------
stage plugin
if [ ! -f plugin/CrusstyGen.jar ]; then
  (cd plugin && SRV_DIR="$SRV263" bash build.sh >/tmp/plugin.log 2>&1) || die plugin "build.sh (see /tmp/plugin.log)"
  echo "plugin built: $(md5sum plugin/CrusstyGen.jar | cut -d' ' -f1)"
else
  echo "plugin jar present: $(md5sum plugin/CrusstyGen.jar | cut -d' ' -f1)"
fi
git checkout -- plugin/ 2>/dev/null || true   # tracked jar gets dirty from builds

# ---- 7. probe263 (aliyun first: repo1 429s) ---------------------------------
stage probe
if [ ! -s "$JB" ]; then
  curl -sfL -o "$JB" "$JB_ALIYUN" || curl -sfL -o "$JB" "$JB_CENTRAL" || die probe "jb-annotations download"
fi
file "$JB" | grep -q "Zip archive" || die probe "jb-annotations not a zip"
(cd probe263 && SRV_DIR="$SRV263" JAVAC="$JDK_DIR/bin/javac" bash build.sh >/tmp/probe.log 2>&1) \
  || die probe "build.sh (see /tmp/probe.log)"
[ -f probe263/CrusstyProbe263.jar ] || die probe "probe jar missing"
git checkout -- probe263/ 2>/dev/null || true
echo "probe built: $(md5sum probe263/CrusstyProbe263.jar | cut -d' ' -f1)"

# ---- 8. server/ bootstrap ---------------------------------------------------
stage server
NEED_BOOT=0
if [ ! -f server/versions/26.3/paper-26.3.jar ]; then
  rm -rf server/versions; cp -rT "$SRV263/versions" server/versions
  NEED_BOOT=1
fi
if [ ! -d server/libraries/io/papermc/paper/paper-api ]; then
  rm -rf server/libraries; cp -rT "$SRV263/libraries" server/libraries
  NEED_BOOT=1
fi
if [ ! -f server/paper.jar ] || [ "$(md5sum < server/paper.jar | cut -d' ' -f1)" != "$PAPER_MD5" ]; then
  cp "$SRV263/paper.jar" server/paper.jar; NEED_BOOT=1
fi
# bootstrap_server.sh pins the build ONLY when it downloads the jar itself;
# with a pre-placed jar it prints the existing marker — so force the pin here
if [ ! -f server/paper.build ] || [ "$(cat server/paper.build)" != "$PAPER_BUILD" ]; then
  echo "$PAPER_BUILD" > server/paper.build; NEED_BOOT=1
fi
if [ "$NEED_BOOT" = "1" ] || [ ! -f server/crussty-boot-ok ]; then
  LEVEL_NAME=world bash harness/bootstrap_server.sh "$PAPER_BUILD" || die server "bootstrap"
  touch server/crussty-boot-ok
  echo "server/ bootstrapped (build $PAPER_BUILD)"
else
  echo "server/ already bootstrapped"
fi

# ---- 9. sanity ---------------------------------------------------------------
stage sanity
echo "git:     $(git rev-parse --short HEAD) $(git status --short | wc -l) dirty"
echo "cargo:   $(cargo --version)"
echo "jdk25:   $($JDK_DIR/bin/java -version 2>&1 | head -1)"
echo "paper:   build $PAPER_BUILD sha256 ${PAPER_SHA256:0:12}..."
echo "plugin:  $(md5sum plugin/CrusstyGen.jar | cut -d' ' -f1)"
echo "probe:   $(md5sum probe263/CrusstyProbe263.jar | cut -d' ' -f1)"
echo ".so:     $(stat -c%s native/target/release/libcrussty_chunkgen.so) bytes"
echo ""
echo "RECOVER OK — environment ready (ab263_ci.sh ore is the health check)"
