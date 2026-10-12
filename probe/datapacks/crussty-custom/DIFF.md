# crussty-custom — exact diff vs vanilla

File overridden: `data/minecraft/worldgen/noise_settings/overworld.json`
(taken verbatim from the vanilla server jar shipped inside paper.jar:
`META-INF/versions/1.21.8/server-1.21.8.jar!data/minecraft/worldgen/noise_settings/overworld.json`)

Exact change:
- `"sea_level": 63` → `"sea_level": 40`

That is the ONLY change. All other keys (noise_router, surface_rule, default_block,
default_fluid, aquifers_enabled, ore_veins_enabled, spawn_target, noise, legacy_random_source,
disable_mob_generation) are byte-identical to vanilla.

Effect: oceans/lakes/rivers drain by 23 blocks — exposed ocean floors, dry-ish rivers,
coastline shifted far out. Visible at a glance and block-diff % vs vanilla dumps is large.
It exercises the custom-noise-settings code path end to end (sea level feeds fluid placement
and aquifer/barrier math in the noise router, all still deterministic for seed
3257840388504953787).

pack.mcmeta: pack_format 81 (1.21.8 data pack format; server accepts without warnings).

Regeneration: harness/bench.sh datapack datapack (datapack must be deployed to
server/datapack/datapacks/crussty-custom/ first).
