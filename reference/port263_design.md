# 26.3 densityfunction layer — port design (cont-48 entry point)

Status after cont-47/47b: noise core (n263.rs) bit-exact vs Paper 26.3 b218, 61/61 tests.
This doc freezes the reading map + Rust architecture for the density-function layer.

## What changed vs 1.21.8 (from reference/dec263/)

* Package moved: `levelgen.DensityFunctions` -> `levelgen.densityfunction.DensityFunctions`
  (op classes split into `op/` and `generator/` subpackages).
* Evaluation is FLOAT end-to-end: `sampleValue(ctx, x, y, z) -> float`; volume evaluation
  through `DensityBuffer.values[float[]]` + `DensityVolume` (sizeX/Y/Z, minBlock*, step*).
* `NoiseChunk` is now a thin wrapper (73 lines): `RandomState.samplersWithContext(SamplerContext)`
  builds a `DensitySamplerSet`; the per-chunk cell interpolation lives in
  `op/InterpolatedFunction` (236 lines — MUST read fully; it replaces 1.21.8's
  NoiseChunk interpolators: cell grid, Y-flip lerp order, flat-cache equivalence).
* New ops vs 1.21.8: `LerpFunction`, `PowFunction`, `RoundFunction`, `SliceFunction`,
  `FindTopSurfaceFunction`, `IntervalSelectFunction`. Known 1.21.8 ops missing/renamed?
  (VERIFY against DensityFunctions.DIRECT_CODEC switch list.)
* `NoiseRouter` fields are all string refs to density_function registry files
  (`minecraft:overworld/final_density` etc.); router keys 26.3:
  chunk_surface_level, continents, depth, erosion, final_density, ridges, temperature,
  vegetation — barrier/fluid_*/vein_*/initial_density_without_jaggedness are GONE
  (aquifers became a noise_settings section `{barrier:..., exclusivity...}`, ore-vein
  DFs removed). Sea level carve/jaggedness semantics -> VERIFY in ChunkGen pipeline
  (moonrise chunk_gen patch) where final_density is consumed.
* `MaterialSystem` (levelgen.material) replaces SurfaceSystem wiring for the material
  rule; noise_settings now references `material_rule`/`material_condition` REGISTRY
  entries (vanilla files: reference/vanilla263/data/minecraft/worldgen/material_rule|condition).

## Rust architecture (native/src/n263.rs continuation)

Keep ONE tree representation (like current density.rs) — no separate compile stage:

```rust
pub enum Df263 {
    Constant(f32),
    Noise { noise: Rc<Noise263>, xz_scale: f64, y_scale: f64 },
    ShiftNoise { ... },            // shift_x/shift_z families
    Binary { op: BinOp, a: Box<Df263>, b: Box<Df263> },   // add/mul/min/max
    Unary { op: UnOp, a: Box<Df263> },                    // abs/quarter/squeeze/half_negative
    Clamp { lo: f32, hi: f32, a: Box<Df263> },
    Lerp { a, lo, hi }, Pow { a, b }, Round { a },
    RangeChoice { a, lo, hi, lo_val, hi_val },
    Spline { ... },                // CubicSpline f32 nodes (verify float semantics!)
    Interpolated { cell_w: u32, cell_h: u32, a: Box<Df263> },  // per-chunk cell cache
    FlatCache { a: Box<Df263> },   // xz-only cache (verify existence in 26.3)
    FindTopSurface { ... },
    EndIslands, YGradient, ...
}
```

Evaluation context (per chunk fill / per height query):
```rust
pub struct DfCtx263 {
    pub min_x: i32, pub min_y: i32, pub min_z: i32,   // chunk/block origin
    pub cell_w: u32, pub cell_h: u32,                  // from NoiseSettings equivalent
    // cell-corner caches: HashMap keyed by (df-node-id, cell) — or precomputed
    // columnar buffers mirroring DensityBuffer tiling (TilingMode.java).
}
```

Bit-parity rules (cont-47b lesson):
* ALL node math in f32 EXCEPT coordinate transforms that vanilla keeps f64
  (noise.get(x*frequency...) takes f64; BlendedNoise multipliers f64).
* Read every op's bytecode with javap if vineflower output looks truncated
  (the NormalNoise octave-halving was lost to a truncated render).
* Golden procedure: extend oracle263/ to compile the overworld router's
  DF tree against the real jar (RandomState.create via registry access),
  dump sampleValue grids (e.g. 4x4x4 points per cell) -> Rust goldens.

## Reading queue (exact files, reference/dec263/)

1. op/InterpolatedFunction.java (236) — **READ, semantics frozen (see below)**
2. op/SliceFunction.java + TilingMode.java — volume tiling
3. DensityFunctions.java (408) — DIRECT_CODEC op list + HolderHolder semantics
4. DensityFunctionCompiler.java (87) — JSON ref resolution + caching
5. NoiseRouterData.java — how the overworld router tree is BUILT (wired refs)
6. op/FindTopSurfaceFunction.java — new surface detection (feeds chunk_surface_level?)
7. NoiseRouter.java + NoiseGeneratorSettings.java — settings shape for worldgen.rs loader
8. material/MaterialSystem.java — surface rule host (port of surface.rs target)
9. moonrise chunk_gen patch: ChunkGenPerlinNoise/CoordCache usage in fill pipeline

## InterpolatedFunction 26.3 — FROZEN semantics (read 2026-10-24)

Two DIFFERENT numeric paths (both vanilla; do NOT unify):

* **Volume fill** (chunk gen): input sampled at cell CORNERS (cellVolume step=cellSize);
  per cell: alphaZ = z*cellSizeXzInv -> Z-lerp first (v00_/v01_/v10_/v11_),
  then alphaX -> X-lerp (v_0_/v_1_), then Y via ADDITIVE walk:
  `valueStep = (v_1_ - v_0_) * cellSizeYInv; value = v_0_ + valueStep*y0; value += valueStep`
  per y — f32 accumulation drift is REQUIRED for bit parity.
  Order: Z -> X -> Y-additive. (1.21.8 was Y->X->Z per-point lerp3 — CHANGED.)
* **Point query** (height service): 2x2x2 corner buffer at
  (blockX - xInCell, ..., step=cellSize), then Mth.lerp3(xInCell/cellXz, yInCell/cellY,
  zInCell/cellXz, corners...) — X,Y,Z lerp2/lerp3 path. If point sits ON a cell corner
  (xInCell==yInCell==zInCell==0): direct input sample.
* Cell coverage: cellVolume spans cells [floorDiv(min)..floorDiv(max)] with +1 cell when
  maxBlockX % cellSize != 0; Y same with cellSizeY.
* Implication for the Rust fill: precompute corner grid (like 1.21.8 slice slots) then
  reproduce fillCell EXACTLY (incl. additive Y) for chunk volumes; height queries use the
  point path.

## Harness deltas queued

* server_up.sh/prof_session.sh -> server263 variants (JDK 25, port 25566/25576)
* judge: reference dumps must be re-captured from vanilla 26.3 (.crsf format unchanged)
* plugin 26.3 fixes: markPosForPostprocessing rename, bulk-layout "registry" field rename,
  biome source swap retarget
