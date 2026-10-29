# crussty-f1 — native features-engine probe (stage F1)

Purpose: bit-parity probe for the native decoration master loop (features
engine). Two server runs of the SAME world and pack must produce IDENTICAL
blocks:

1. reference run: vanilla generator (`generator:` absent from bukkit.yml) —
   Paper's vanilla addVanillaDecorations runs the pack features in Java.
2. native run: CrusstyGen + `CRUS_NATIVE_DECO=1` — vanilla decorations are
   skipped (shouldGenerateDecorations=false) and the DecoDriver populator
   drives the native engine (features_master + features_jni bridge).

Pack contents (23 files):
- `data/minecraft/dimension/overworld.json` — explicit multi_noise biome
  source with 2 probe biomes (test_a temp [-1.005..-0.445], test_b
  [-0.445..2.005]; all other params full range) over vanilla
  `minecraft:overworld` noise settings. Explicit (non-preset) source is
  required: the native ctx parses data-driven biome sources only.
- `data/crussty_f1/worldgen/biome/test_{a,b}.json` — full-range biome defs,
  carvers null, features only at step 6 (underground_ores) and step 9
  (vegetal_decoration). Surface = vanilla default_block (stone/deepslate):
  no vanilla biome-conditioned surface rules match the custom biomes —
  deterministic in BOTH runs.
- 9 configured + 12 placed features covering the F1 surface:
  simple_block (simple/weighted/rotated state providers, schedule_tick not
  used), replace_single_block (stone→andesite, deepslate→deepslate_andesite),
  random_selector (chance 0.3 → sel_a, default sel_b),
  simple_random_selector, random_boolean_selector, count, in_square,
  heightmap (MOTION_BLOCKING / WORLD_SURFACE_WG), height_range
  (uniform/trapezoid absolute), random_offset (xz U[-3..3] double-draw,
  y U[0..2]), surface_water_depth_filter 0, block_predicate_filter
  (would_survive poppy), rarity_filter 2, biome_filter.

Regeneration flow (two boots, same seed 3257840388504953787):
  harness/plugin_bench.sh-equivalent with CRUS_NATIVE_DECO toggled, then
  scripts/diff via reference/dump tooling. Judge criterion: 0 divergent
  blocks outside the aquifer/fluid-update noise band.
