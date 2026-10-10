# c-faster

Vanilla-exact chunk generation for Paper 1.21.8 at target **1 ms / full chunk end-to-end** (even with custom-generation datapacks).

Judge: block-for-block equality against reference dumps captured from a REAL vanilla server.
An optimization is accepted only if the block match percentage does NOT drop.

Layout:
- `native/`   — Rust cdylib `crussty_chunkgen` (C-ABI v2 + JNI natives)
- `probe/`    — Paper probe plugin: dumps fully-generated chunks as `.crsf` reference files
- `plugin/`   — production ChunkGenerator plugin (JNI bridge)
- `harness/`  — benchmark scripts (RCON, single-invocation protocol, md5 deploy check)
- `reference/` — vanilla reference dumps + block registry
- `.github/workflows/` — CI

Rules (frozen):
- Same block in the same place; vanilla math may be skipped ONLY when it cannot affect final blocks.
- No fudging a "similar" world. Reference files are the single judge.
