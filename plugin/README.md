# CrusstyGen plugin

Takes over the chunk-gen stages of **Paper 26.3** (build 222, JDK 25) via JNI
calls into `libcrussty_chunkgen.so`: native fill (noise), native surface rules,
native carvers, native decoration — each stage behind a kill-switch env
(CRUS_NO_NATIVE_SURFACE_263 / CRUS_NATIVE_CARVERS / CRUS_NATIVE_DECO), gated by
the A/B parity protocol in `harness/ab263_ci.sh`.

## Wiring worlds

`server263/bukkit.yml` (gitignored, maintained by harness):

```yaml
worlds:
  vanilla:
    generator: CrusstyGen
  datapack:
    generator: CrusstyGen
```

Note: bukkit.yml generator binding applies at world creation; for benchmark
runs use fresh world dirs (same seed => deterministic world => comparable
dumps).

## Deploy

1. `plugin/build.sh` -> `plugin/CrusstyGen.jar`
2. copy jar to `server263/plugins/`
3. put `libcrussty_chunkgen.so` where the JVM finds it: `-Djava.library.path`
   in the server start command (harness/server263_up.sh) or copy into
   `$JAVA_HOME/lib` (not recommended). Harness passes
   `-Djava.library.path=/home/z/my-project/c-faster/native/target/release`.
4. record md5s in `server263/deployed263.md5` (deploy-hygiene rule)

## Commands

- `/crusgenstats` — native noise-stage stats: count, mean, p50, p99, max (us).
  This measures ONLY generateNoise (native fill + ChunkData layout), not the
  vanilla remainder (surface/carvers/features/lighting).

## Blocks

Native u16 ids follow `block_registry.txt` (repo root, frozen append-only).
`GenStats.MATERIALS` maps ids -> Material for the ChunkData layout step. If
the native side gains new ids, update both `block_registry.txt` (append) and
this table.
