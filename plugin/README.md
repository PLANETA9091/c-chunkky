# CrusstyGen plugin

Takes over ONLY the noise stage of chunk generation on Paper 1.21.8 via a
single JNI call into `libcrussty_chunkgen.so` (ABI v2). Vanilla continues on
top of our terrain: surface rules, carvers, features, lighting — that is what
preserves block-for-block parity with the vanilla reference dumps
(`reference/dumps/*`).

## Wiring worlds

`server/bukkit.yml` (gitignored, maintained by harness):

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
2. copy jar to `server/plugins/`
3. put `libcrussty_chunkgen.so` where the JVM finds it: `-Djava.library.path`
   in the server start command (harness/server_up.sh) or copy into
   `$JAVA_HOME/lib` (not recommended). Harness passes
   `-Djava.library.path=/home/z/my-project/c-faster/native/target/release`.
4. record md5s in `server/deployed.md5` (deploy-hygiene rule)

## Commands

- `/crusgenstats` — native noise-stage stats: count, mean, p50, p99, max (us).
  This measures ONLY generateNoise (native fill + ChunkData layout), not the
  vanilla remainder (surface/carvers/features/lighting).

## Blocks

Native u16 ids follow `block_registry.txt` (repo root, frozen append-only).
`GenStats.MATERIALS` maps ids -> Material for the ChunkData layout step. If
the native side gains new ids, update both `block_registry.txt` (append) and
this table.
