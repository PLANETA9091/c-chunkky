# c-faster

Vanilla-exact chunk generation for Paper 1.21.8, targeting **1 ms/chunk for the native decoration stage end-to-end** (p50, incl. all JNI bridges), even with custom-generation datapacks.

Measurement precision (what the 1 ms numbers are and are NOT):
- **Reached** (2026-10-21): deco-stage p50 0.96-1.01 ms on the f1 probe pack; 1.77 ms on the heavy Terralith+Tectonic track (cont-58: 2.9 -> 1.77 ms via fused snapshot queries, native env-scan build-height arithmetic, water-matching parity fix, native glow_lichen; still shrinking). These are DecoStats `populate` wall numbers for the FEATURES pipeline only.
- **NOT covered by the 1 ms claim**: the native noise/fill stage (~10 ms/chunk p50) and the full-chunk generation wall (~40-85 ms/chunk incl. layout, lighting and server plumbing). "1 ms / full chunk" was the original aspirational wording; the honest current split is documented above and in `worklog-session.md`.

Judge: block-for-block equality against reference dumps captured from a REAL vanilla server.
An optimization is accepted only if the block match percentage does NOT drop.

## Distribution: a CRUSSTY module (not standalone)

The project ships as a [CRUSSTY](https://github.com/PLANETA9091/CRUSSTY) module — one
artifact instead of a plugin jar + a manually deployed `.so`:

```bash
bash build.sh                 # -> dist/faster/{module.json, libcrussty_chunkgen.so, CrusstyGen.jar.x}
cp -r dist/faster <server>/modules/   # or: crussty install PLANETA9091/c-faster (release bundle)
```

At boot, before the kernel starts, the module's `cplugin_init`:
1. resolves its own `.so` path (dladdr) and the server root (cwd walk-up),
2. installs the bundled `CrusstyGen.jar.x` into `<server>/plugins/CrusstyGen.jar`
   (byte-diff guarded; the `.x` suffix is the runtime's park-convention —
   the module scanner probes every `*.jar` under `modules/` otherwise),
3. writes `plugins/CrusstyGen.crussty-native` — the absolute `.so` path.
   `ChunkGenNative` reads it in static init and `System.load`s the module
   library (which carries BOTH `cplugin_init` and every JNI native), binding
   natives to the plugin's ClassLoader — no `-Djava.library.path`.

Verified end-to-end (2026-10-21, runtime v2.7.0 + Paper 1.21.8-60): module loads
(`init rc=0`), jar+marker installed, `CrusstyGen enabled`, natives bound via the
marker, world generated with the native ctx pool. Options: `plugin=off` skips the
install (natives-only mode); `plugin_dir=<path>` overrides the target plugins dir.

## Standalone mode (harness/bench/CI — unchanged)

Harness scripts still deploy `plugin/CrusstyGen.jar` + `native/target/release/libcrussty_chunkgen.so`
with `-Djava.library.path`: without the marker file `ChunkGenNative` falls back to
`System.loadLibrary("crussty_chunkgen")`. Both form factors share one codebase.

Layout:
- `native/`   — Rust cdylib `crussty_chunkgen` (C-ABI v2 + JNI natives) + `cplugin.rs` (CRUSSTY module entry, vendored `cplug-abi`/`cplug-sdk`)
- `probe/`    — Paper probe plugin: dumps fully-generated chunks as `.crsf` reference files
- `plugin/`   — production ChunkGenerator plugin (JNI bridge)
- `module.json` + `build.sh` — CRUSSTY module manifest and module build (dist/faster/)
- `harness/`  — benchmark scripts (RCON, single-invocation protocol, md5 deploy check)
- `reference/` — vanilla reference dumps + block registry
- `.github/workflows/` — CI

Rules (frozen):
- Same block in the same place; vanilla math may be skipped ONLY when it cannot affect final blocks.
- No fudging a "similar" world. Reference files are the single judge.
