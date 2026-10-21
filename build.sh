#!/usr/bin/env bash
# CRUSSTY module build (repo root): native lib + plugin jar -> dist/faster/
# module layout. The dist/faster directory is THE module: drop it into a
# server's modules/ (or `crussty install PLANETA9091/c-faster` from a
# release bundle) — at boot cplugin_init installs CrusstyGen.jar into
# plugins/ and writes the native-path marker; no java.library.path, no
# manual .so copying.
#
# Standalone harness/bench builds are unchanged:
#   native/  -> cargo build --release (libcrussty_chunkgen.so)
#   plugin/  -> bash build.sh        (CrusstyGen.jar)
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

( cd native && cargo build --release )
( cd plugin && bash build.sh )

mkdir -p dist/faster
cp native/target/release/libcrussty_chunkgen.so dist/faster/
# .x = the runtime's park-suffix: the module scanner probes every *.jar under
# modules/ as an archive module; the parked name keeps the boot log clean.
cp plugin/CrusstyGen.jar dist/faster/CrusstyGen.jar.x
cp module.json dist/faster/

echo "--- module -> dist/faster/ (module.json + libcrussty_chunkgen.so + CrusstyGen.jar) ---"
ls -la dist/faster/
