# c-faster

Vanilla-exact chunk generation for **Paper 26.3** (build 222, JDK 25), native Rust
takeover of the chunk-gen stages — fill, surface, carvers, decoration — targeting
the minimum honest end-to-end ms/chunk (12x12 forceload window, incl. layout,
lighting and server plumbing). Paper 1.21.8 is RETIRED: all harness, CI and
servers are 26.3-only (`harness/server263_up.sh`, `server263/`).

Measurement precision (what the numbers are):
- **Anchor (cont-143)**: HONEST BEST 43.34 ms/ch (12x12 @14600, w2/F2,
  no-watchdog, carver fuse ON, vanilla-parity world); pack-world lever A/B
  (cont-144): native surface ON 75.70 vs OFF 68.62 ms/ch @14600.
- Per-stage splits (fill p50, deco phases, carver wall) live in
  `PROGRESS.json` and `STATUS.md`; harness `run263_baseline.sh` reproduces
  the number on a fresh region.

Parity: NOT block-diff against 1.21.8 reference dumps (that judge was removed
with the 26.3 retarget) — the honest 26.3 protocol is A/B kill-switch gates
(`harness/ab263_ci.sh`): ore signature max(4%,500) + orebase strict bit-equal
+ blockdump fingerprint for the deco-noise class, native stage ON vs OFF.

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

Verified end-to-end: module loads
(`init rc=0`), jar+marker installed, `CrusstyGen enabled`, natives bound via the
marker, world generated with the native ctx pool. Options: `plugin=off` skips the
install (natives-only mode); `plugin_dir=<path>` overrides the target plugins dir.

## Standalone mode (harness/bench/CI — unchanged)

Harness scripts still deploy `plugin/CrusstyGen.jar` + `native/target/release/libcrussty_chunkgen.so`
with `-Djava.library.path`: without the marker file `ChunkGenNative` falls back to
`System.loadLibrary("crussty_chunkgen")`. Both form factors share one codebase.

Layout:
- `native/`   — Rust cdylib `crussty_chunkgen` (C-ABI v2 + JNI natives) + `cplugin.rs` (CRUSSTY module entry, vendored `cplug-abi`/`cplug-sdk`)
- `plugin/`   — production ChunkGenerator plugin (JNI bridge)
- `probe263/` — Paper 26.3 probe plugin (crusfull/crusgenstats/statdump RCON commands)
- `oracle263/` — 26.3 fill/oracle verification classes
- `module.json` + `build.sh` — CRUSSTY module manifest and module build (dist/faster/)
- `harness/`  — 26.3 benchmark/CI scripts (RCON, single-invocation protocol, md5 deploy check)
- `reference/` — historical analysis docs + datapack/oracle fixtures
- `.github/workflows/` — CI (26.3 only)

Rules (frozen):
- Same block in the same place; vanilla math may be skipped ONLY when it cannot affect final blocks.
- No fudging a "similar" world. Reference files are the single judge.
