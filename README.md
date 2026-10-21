# c-faster

Vanilla-exact chunk generation for Paper 1.21.8, targeting **1 ms/chunk for the native decoration stage end-to-end** (p50, incl. all JNI bridges), even with custom-generation datapacks.

Measurement precision (what the 1 ms numbers are and are NOT):
- **Reached** (2026-10-21): deco-stage p50 0.96-1.01 ms on the f1 probe pack; 2.9 ms on the heavy Terralith+Tectonic track (still shrinking). These are DecoStats `populate` wall numbers for the FEATURES pipeline only.
- **NOT covered by the 1 ms claim**: the native noise/fill stage (~10 ms/chunk p50) and the full-chunk generation wall (~40-85 ms/chunk incl. layout, lighting and server plumbing). "1 ms / full chunk" was the original aspirational wording; the honest current split is documented above and in `worklog-session.md`.

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
