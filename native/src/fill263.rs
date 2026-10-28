//! Native 26.3 fill — the doFill pipeline of NoiseBasedChunkGenerator 26.3:
//!
//!   volume = DensityVolume(16, 384, 16, minX, minY, minZ)          (step 1)
//!   NoiseChunk: SamplerContext(cached) + Beardifier user field
//!   buffer = finalDensity.sampleVolume(volume)                      (f32)
//!   for z, x, y top-down:
//!     density = buffer[index]
//!     state = aquifer.computeSubstance(bx, by, bz, (double) density)
//!     state == null  -> defaultBlock (stone)          (always written)
//!     state == AIR   -> no write (buffer stays 0)
//!     fluid          -> write + fluid-update marks (sticky aquifer flag)
//!
//! Output contract (unchanged JNI ABI): u16 block ids per block_registry.txt
//! (0 air, 1 stone, 3 water, 17 lava), layout (y-minY)*256 + z*16 + x.
//! Fluid-update marks packed BlockPos.asLong, same as the legacy path.

use crate::aquifer263::{Aquifer263, AquiferPrefill263, FluidTy};
use crate::df263::{DensityVolume, FuseProg, SamplerCtx263, PROF_ON_263};
use crate::df263_router::RouterBuilder263;
use crate::nochunk::OUT_LEN;

pub const STONE_ID: u16 = 1;
pub const WATER_ID: u16 = 3;
pub const LAVA_ID: u16 = 17;
pub const DIRT_ID: u16 = 5;

// ---------------------------------------------------------------------------
// post-fill WORLD_SURFACE_WG heightmap handoff (fill -> carve)
//
// Vanilla 26.3 freezes the _WG heightmaps at the BIOMES->TERRAIN transition:
// doFill updates them directly (worldSurface.update per written block), but
// every setBlockState during TERRAIN (surface + carve) touches only the
// FINAL heightmaps (ChunkStatus.heightmapsAfter). The carver-mask topMaterial
// therefore reads its surface gradients from the POST-FILL map even though
// the chunk has since been surfaced and carved. The plugin's pipeline has no
// Java-side _WG map at all — the fill parks the post-fill map here and the
// carve consumes it (same handoff pattern as V263_GRIDS).
// ---------------------------------------------------------------------------

static V263_WG_HM: std::sync::Mutex<Option<std::collections::HashMap<(i32, i32), Box<[i32; 256]>>>> =
    std::sync::Mutex::new(None);

/// Topmost non-air block y per column (WORLD_SURFACE_WG semantics, the same
/// algorithm apply_surface263 uses; none -> min_y - 1).
pub fn world_surface_heightmap(min_y: i32, height: i32, out: &[u16]) -> Box<[i32; 256]> {
    let mut hm = Box::new([min_y - 1; 256]);
    for col in 0..256usize {
        // single top-down scan per column over the flat y-major layout
        // (index = (y - min_y) * 256 + z * 16 + x)
        let mut top = min_y - 1;
        for ly in (0..height as usize).rev() {
            if out[ly * 256 + col] != 0 {
                top = min_y + ly as i32;
                break;
            }
        }
        hm[col] = top;
    }
    hm
}

/// Park the post-fill _WG heightmap for the carve stage (keyed by chunk).
/// The map is capped defensively: entries are consumed by the carve, and a
/// worldgen abort could otherwise grow it without bound.
pub fn park_wg_heightmap(cx: i32, cz: i32, hm: Box<[i32; 256]>) {
    let mut guard = V263_WG_HM.lock().unwrap();
    let map = guard.get_or_insert_with(std::collections::HashMap::new);
    if map.len() >= 8192 {
        map.clear();
    }
    map.insert((cx, cz), hm);
}

/// Consume the parked post-fill _WG heightmap (None = the chunk was never
/// filled natively — the caller falls back to the post-surface buffer).
pub fn take_wg_heightmap(cx: i32, cz: i32) -> Option<Box<[i32; 256]>> {
    let mut guard = V263_WG_HM.lock().unwrap();
    guard.as_mut()?.remove(&(cx, cz))
}

pub struct Gen263 {
    pub seed: i64,
    pub sea_level: i32,
    pub min_y: i32,
    pub height: i32,
    pub final_density: crate::df263::SamRef,
    /// Compiled pointwise fusion program for final_density (None = the
    /// subtree did not fit the stack budget; the tree walk is the fallback).
    pub final_density_fuse: Option<std::sync::Arc<FuseProg>>,
    /// CRUS_FUSE=1 — opt-in fused interpreter (bit-exact but measured
    /// slower than the walk on the current shallow-shell tree).
    pub fuse_enabled: bool,
    pub aquifer_cfg: crate::aquifer263::AquiferConfig263,
    /// RandomState.getOrCreateRandomFactory("minecraft:aquifer").
    pub aquifer_factory: crate::rng::XoroshiroPositionalRandomFactory,
    /// The six climate router nodes (temperature, vegetation, continents,
    /// erosion, depth, ridges) — the createClimateSampler fields; used by
    /// the native biome stage (biome263).
    pub climate: [crate::df263::SamRef; 6],
}

impl Gen263 {
    pub fn new(seed: i64, min_y: i32, height: i32, sea_level: i32) -> Self {
        let mut builder = RouterBuilder263::new(seed);
        let router = builder.build_overworld();
        let aquifer_cfg = builder.build_aquifer_config();
        // RandomState 26.3 getOrCreateRandomFactory(name):
        //   positionalRandoms.computeIfAbsent(name, k -> random.fromHashOf(k).forkPositional())
        let aquifer_factory = builder
            .factory
            .root_from_hash_of("minecraft:aquifer")
            .fork_positional();
        let final_density_fuse = FuseProg::compile(&router.final_density).map(std::sync::Arc::new);
        let fuse_enabled = std::env::var("CRUS_FUSE").ok().as_deref() == Some("1");
        Gen263 {
            seed,
            sea_level,
            min_y,
            height,
            final_density: router.final_density.clone(),
            final_density_fuse,
            fuse_enabled,
            aquifer_cfg,
            aquifer_factory,
            climate: [
                router.temperature.clone(),
                router.vegetation.clone(),
                router.continents.clone(),
                router.erosion.clone(),
                router.depth.clone(),
                router.ridges.clone(),
            ],
        }
    }

    /// Shared aquifer prefill for the strip threads (deterministic per
    /// chunk: quart surface-level cache + skip-above bound). Compute once
    /// in fill_chunk, hand &pre to every fill_strip — removes the
    /// per-strip maxSurfaceLevel walk (was 0.37ms x strips).
    pub fn aquifer_prefill(&self, volume: &DensityVolume) -> AquiferPrefill263 {
        let mut ctx = SamplerCtx263::cached();
        let cfg = crate::aquifer263::AquiferConfig263 {
            barrier: self.aquifer_cfg.barrier.clone(),
            floodedness: self.aquifer_cfg.floodedness.clone(),
            spread: self.aquifer_cfg.spread.clone(),
            lava: self.aquifer_cfg.lava.clone(),
            exclusion: self.aquifer_cfg.exclusion.clone(),
            surface_level: self.aquifer_cfg.surface_level.clone(),
        };
        Aquifer263::prefill(&mut ctx, &cfg, volume)
    }

    pub fn aquifer_with_prefill(
        &self,
        volume: &DensityVolume,
        pre: &AquiferPrefill263,
    ) -> Aquifer263 {
        Aquifer263::new_with_prefill(
            crate::aquifer263::AquiferConfig263 {
                barrier: self.aquifer_cfg.barrier.clone(),
                floodedness: self.aquifer_cfg.floodedness.clone(),
                spread: self.aquifer_cfg.spread.clone(),
                lava: self.aquifer_cfg.lava.clone(),
                exclusion: self.aquifer_cfg.exclusion.clone(),
                surface_level: self.aquifer_cfg.surface_level.clone(),
            },
            self.aquifer_factory.clone(),
            volume,
            self.sea_level,
            pre,
        )
    }

    pub fn aquifer_for<'a>(
        &'a self,
        ctx: &mut SamplerCtx263,
        volume: &DensityVolume,
    ) -> Aquifer263 {
        Aquifer263::new(
            ctx,
            crate::aquifer263::AquiferConfig263 {
                barrier: self.aquifer_cfg.barrier.clone(),
                floodedness: self.aquifer_cfg.floodedness.clone(),
                spread: self.aquifer_cfg.spread.clone(),
                lava: self.aquifer_cfg.lava.clone(),
                exclusion: self.aquifer_cfg.exclusion.clone(),
                surface_level: self.aquifer_cfg.surface_level.clone(),
            },
            self.aquifer_factory.clone(),
            volume,
            self.sea_level,
        )
    }

    /// final_density volume sample: fused flat program when enabled AND
    /// compiled (bit-exact vs the tree walk — CI-measured 0/175 parity —
    /// but SLOWER on the real tree: 16 ops / 6 slots, the interpreter loses
    /// to the walk's streaming auto-vectorized buffer passes). Opt-in via
    /// CRUS_FUSE=1 for future deep-shell trees; default is the tree walk.
    #[inline]
    pub fn sample_final_density(
        &self,
        ctx: &mut SamplerCtx263,
        buf: &mut Vec<f32>,
        vol: &DensityVolume,
    ) {
        if PROF_ON_263.load(std::sync::atomic::Ordering::Relaxed) {
            self.final_density.sample_volume(ctx, buf, vol);
        } else if self.fuse_enabled {
            if let Some(prog) = &self.final_density_fuse {
                prog.run(ctx, buf, vol);
            } else {
                self.final_density.sample_volume(ctx, buf, vol);
            }
        } else {
            self.final_density.sample_volume(ctx, buf, vol);
        }
    }

    /// One doFill pass. `beard_list` = the plugin-serialized Beardifier
    /// ([0,0] = none). Returns the packed fluid-update marks in `marks`.
    pub fn fill_chunk(
        &self,
        cx: i32,
        cz: i32,
        beard_list: &[i32],
        out: &mut [u16; OUT_LEN],
        marks: &mut Vec<i64>,
    ) {
        marks.clear();
        // The pool may hand us a reused array — vanilla semantics: AIR cells
        // are NOT written, so the destination must start zeroed.
        out.fill(0);
        let volume = DensityVolume::block_step((16, self.height, 16), (cx * 16, self.min_y, cz * 16));

        // ---- strip-parallel fill -----------------------------------------
        // The volume sample + substance walk are per-z-strip pure functions
        // (deterministic per point; per-strip caches converge to identical
        // values). Strips write disjoint out[] rows; marks concatenate in
        // strip (z) order — the same order the sequential walk produces.
        // The aquifer prefill runs per thread over the FULL volume so every
        // strip's 4-closest grid lookups see the whole chunk's surface grid.
        let threads = fill_threads();
        if threads > 1 && volume.size_z >= threads as i32 {
            let strips: Vec<(i32, i32)> = split_strips(volume.size_z, threads);
            if strips.len() > 1 {
                // shared aquifer prefill (deterministic per chunk): one
                // maxSurfaceLevel walk instead of one per strip
                let pre = self.aquifer_prefill(&volume);
                // per-strip private out buffer (disjoint z rows) merged
                // sequentially afterwards; marks concatenate in strip order —
                // identical to the sequential walk's z-major order.
                let volume_ref = &volume;
                let beard_ref = &beard_list;
                let strip_results: Result<Vec<(Vec<i64>, Box<[u16; OUT_LEN]>)>, ()> = std::thread::scope(|scope| {
                    let mut handles = Vec::with_capacity(strips.len() - 1);
                    for &(z0, z1) in &strips[1..] {
                        handles.push(scope.spawn(move || {
                            self.fill_strip(cx, cz, beard_ref, volume_ref, z0, z1, &pre)
                        }));
                    }
                    let mut results = Vec::with_capacity(strips.len());
                    let (z0, z1) = strips[0];
                    results.push(self.fill_strip(cx, cz, beard_list, &volume, z0, z1, &pre));
                    for h in handles {
                        if let Ok(r) = h.join() {
                            results.push(r);
                        } else {
                            // worker panicked — fall back to sequential fill
                            // to preserve correctness (marks stay empty here;
                            // the sequential path below repopulates out).
                            return Err(());
                        }
                    }
                    Ok(results)
                });
                if let Ok(results) = strip_results {
                    out.fill(0);
                    for ((local_marks, buf), &(z0, z1)) in results.iter().zip(strips.iter()) {
                        merge_strip_rows(buf, out, z0, z1);
                        marks.extend_from_slice(local_marks);
                    }
                    return;
                }
                // panic fallback: repair out and fall through to sequential
                out.fill(0);
            }
        }

        let mut ctx = SamplerCtx263::cached();
        ctx.beard = crate::beard263::Beardifier263::decode(beard_list);

        // Aquifer 26.3 constructor (maxSurfaceLevel prefill) runs BEFORE the
        // final_density volume sample — the Java createNoiseChunk ordering
        // (aquifer inside NoiseChunk ctor, doFill samples after), so the
        // shared cache cells converge exactly like cachingSamplers.
        let mut aquifer = self.aquifer_for(&mut ctx, &volume);

        let mut buf = ctx.scratch.acquire(volume.size());
        self.sample_final_density(&mut ctx, &mut buf, &volume);

        for z in 0..volume.size_z {
            for x in 0..volume.size_x {
                for y in (0..volume.size_y).rev() {
                    let block_y = volume.block_y(y);
                    let density = buf[volume.index_unchecked(x, y, z)];
                    let ty = aquifer.compute_substance(
                        &mut ctx,
                        volume.block_x(x),
                        block_y,
                        volume.block_z(z),
                        density as f64,
                    );
                    let Some(ty) = ty else {
                        // vanilla default block (stone) — always written
                        out[out_index(self.min_y, block_y, x, z)] = STONE_ID;
                        continue;
                    };
                    match ty {
                        FluidTy::Air => { /* vanilla skips the write */ }
                        FluidTy::Water => {
                            out[out_index(self.min_y, block_y, x, z)] = WATER_ID;
                            if aquifer.should_schedule_fluid_update() {
                                marks.push(crate::aquifer263::pos_as_long(
                                    volume.block_x(x),
                                    block_y,
                                    volume.block_z(z),
                                ));
                            }
                        }
                        FluidTy::Lava => {
                            out[out_index(self.min_y, block_y, x, z)] = LAVA_ID;
                            if aquifer.should_schedule_fluid_update() {
                                marks.push(crate::aquifer263::pos_as_long(
                                    volume.block_x(x),
                                    block_y,
                                    volume.block_z(z),
                                ));
                            }
                        }
                    }
                }
            }
        }
        ctx.scratch.release(buf);
    }

    /// One z-strip of the fill: own ctx/aquifer/buffer; writes its rows of
    /// out[] and appends fluid marks (z-order preserved). Bit-exact vs the
    /// sequential walk: every value is a pure function of (x, y, z).
    fn fill_strip(
        &self,
        cx: i32,
        cz: i32,
        beard_list: &[i32],
        volume: &DensityVolume,
        z0: i32,
        z1: i32,
        pre: &AquiferPrefill263,
    ) -> (Vec<i64>, Box<[u16; OUT_LEN]>) {
        let strip_vol = DensityVolume {
            size_x: volume.size_x,
            size_y: volume.size_y,
            size_z: z1 - z0,
            min_x: volume.min_x,
            min_y: volume.min_y,
            min_z: volume.min_z + z0 * volume.step_z,
            step_x: volume.step_x,
            step_y: volume.step_y,
            step_z: volume.step_z,
        };
        let mut ctx = SamplerCtx263::cached();
        ctx.beard = crate::beard263::Beardifier263::decode(beard_list);
        // aquifer from the SHARED prefill (the full-volume grid walk ran
        // once in fill_chunk — the strip grid converges to identical values)
        let mut aquifer = self.aquifer_with_prefill(volume, pre);
        let mut buf = ctx.scratch.acquire(strip_vol.size());
        let mut strip_out: Box<[u16; OUT_LEN]> = vec![0u16; OUT_LEN].into_boxed_slice().try_into().unwrap();
        let mut marks: Vec<i64> = Vec::new();
        self.sample_final_density(&mut ctx, &mut buf, &strip_vol);
        for z in 0..strip_vol.size_z {
            let z_global = z0 + z;
            for x in 0..strip_vol.size_x {
                for y in (0..strip_vol.size_y).rev() {
                    let density = buf[strip_vol.index_unchecked(x, y, z)];
                    let ty = aquifer.compute_substance(
                        &mut ctx,
                        strip_vol.block_x(x),
                        strip_vol.block_y(y),
                        strip_vol.block_z(z),
                        density as f64,
                    );
                    let Some(ty) = ty else {
                        strip_out[out_index(self.min_y, strip_vol.block_y(y), x, z_global)] = STONE_ID;
                        continue;
                    };
                    match ty {
                        FluidTy::Air => {}
                        FluidTy::Water => {
                            strip_out[out_index(self.min_y, strip_vol.block_y(y), x, z_global)] = WATER_ID;
                            if aquifer.should_schedule_fluid_update() {
                                marks.push(crate::aquifer263::pos_as_long(
                                    strip_vol.block_x(x),
                                    strip_vol.block_y(y),
                                    strip_vol.block_z(z),
                                ));
                            }
                        }
                        FluidTy::Lava => {
                            strip_out[out_index(self.min_y, strip_vol.block_y(y), x, z_global)] = LAVA_ID;
                            if aquifer.should_schedule_fluid_update() {
                                marks.push(crate::aquifer263::pos_as_long(
                                    strip_vol.block_x(x),
                                    strip_vol.block_y(y),
                                    strip_vol.block_z(z),
                                ));
                            }
                        }
                    }
                }
            }
        }
        ctx.scratch.release(buf);
        (marks, strip_out)
    }

    /// Native iterateNoiseColumn 26.3 (getBaseHeight): 1-cell volume walk,
    /// EMPTY beardifier, aquifer substance, tester stops the walk.
    /// mode 0 = NOT_AIR (WORLD_SURFACE*), 1 = motion-blocking
    /// (OCEAN_FLOOR* — stone in the noise-stage state space).
    pub fn base_height(&self, x: i32, z: i32, mode: u8) -> i32 {
        let volume = DensityVolume::block_step((1, self.height, 1), (x, self.min_y, z));
        let mut ctx = SamplerCtx263::cached();
        let mut aquifer = self.aquifer_for(&mut ctx, &volume);
        let mut buf = ctx.scratch.acquire(volume.size());
        self.sample_final_density(&mut ctx, &mut buf, &volume);
        let mut result = self.min_y;
        for y in (0..volume.size_y).rev() {
            let block_y = volume.block_y(y);
            let density = buf[volume.index_unchecked(0, y, 0)];
            let substance = aquifer.compute_substance(&mut ctx, x, block_y, z, density as f64);
            // None = stone (default block); Some(Air) = air; fluids = fluid.
            let is_stone = substance.is_none();
            let hits = match mode {
                0 => !matches!(substance, Some(FluidTy::Air)), // NOT_AIR
                _ => is_stone,
            };
            if hits {
                result = block_y + 1;
                break;
            }
        }
        ctx.scratch.release(buf);
        result
    }
}

#[inline]
fn out_index(min_y: i32, block_y: i32, x: i32, z: i32) -> usize {
    ((block_y - min_y) as usize) * 256 + (z as usize) * 16 + (x as usize)
}

impl Gen263 {
    /// NoiseBasedChunkGenerator.applyCarvingMask 26.3 over the carver mask:
    /// per masked cell (top-down): UNCARVABLE (bedrock) guard, grass flag,
    /// aquifer.computeSubstance(x, y, z, 0.0) -> null keeps the block,
    /// AIR carves, fluids write + marks, then the vanilla grass→dirt
    /// topMaterial fix: once a grass_block/mycelium was seen in the column,
    /// every carved cell whose below-block is dirt runs
    /// surfaceSystem.topMaterial(helperPos, underFluid) — a fresh 1x1x1
    /// MaterialRuleContext on the POINT-path biome resampler, with the
    /// FROZEN post-fill WORLD_SURFACE_WG gradients (the _WG heightmaps stop
    /// updating at the TERRAIN status). A Some result replaces the dirt and
    /// marks it for postprocessing when the material carries a fluid.
    /// `io` is the post-surface center chunk buffer (JNI layout).
    /// `wg_hm` = the frozen post-fill WORLD_SURFACE_WG heightmap (parked by
    /// the fill stage); `top_ids` = the grass/dirt/fluid id tables of the
    /// extended surface palette.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_carvers(
        &self,
        cx: i32,
        cz: i32,
        seed: i64,
        io: &mut [u16],
        changed: &mut Vec<i64>,
        sf: Option<&crate::surface263::SurfaceState263>,
        b263: Option<&std::sync::Arc<crate::biome263::BiomeState263>>,
        wg_hm: Option<&[i32; 256]>,
        top_ids: &crate::surface263::TopMatIds,
    ) -> Vec<i64> {
        let mut marks = Vec::new();
        let mask = crate::carver263::generate_carver_mask(seed, cx, cz, self.min_y, self.height);
        if mask.is_empty() {
            return marks;
        }
        let volume = DensityVolume::block_step((16, self.height, 16), (cx * 16, self.min_y, cz * 16));
        let mut ctx = SamplerCtx263::cached();
        let mut aquifer = self.aquifer_for(&mut ctx, &volume);
        let min_x = cx * 16;
        let min_z = cz * 16;
        // the frozen post-fill _WG heightmap for the topMaterial gradients;
        // fallback (chunk filled before this handoff existed): the current
        // post-surface buffer (vanilla-in-plugin priming order).
        let hm_fallback;
        let wg_hm: &[i32; 256] = match wg_hm {
            Some(h) => h,
            None => {
                hm_fallback = world_surface_heightmap(self.min_y, self.height, io);
                &hm_fallback
            }
        };
        // the topMaterial context needs the surface + biome state live;
        // without either, the fix degrades to keep-dirt (vanilla's common case).
        let zoom_seed = crate::sha256::obfuscate_seed(self.seed);
        let top_ready = sf.is_some() && b263.is_some();
        mask.visit(|x, z, bottom_y, top_y| {
            let mut has_grass = false;
            let world_x = min_x + x;
            let world_z = min_z + z;
            for world_y in (bottom_y..=top_y).rev() {
                let idx = ((world_y - self.min_y) * 256 + z * 16 + x) as usize;
                let block = io[idx];
                if block == 2 {
                    continue; // bedrock — the whole UNCARVABLE tag in 26.3
                }
                if top_ids.is_grass(block) {
                    has_grass = true; // grass_block / mycelium (extended ids included)
                }
                let Some(ty) = aquifer.compute_substance(&mut ctx, world_x, world_y, world_z, 0.0)
                else {
                    continue; // vanilla: null -> keep the block
                };
                let pre = io[idx];
                let write = match ty {
                    crate::aquifer263::FluidTy::Air => Some(0u16),
                    crate::aquifer263::FluidTy::Water => {
                        if aquifer.should_schedule_fluid_update() {
                            marks.push(crate::aquifer263::pos_as_long(world_x, world_y, world_z));
                        }
                        Some(WATER_ID)
                    }
                    crate::aquifer263::FluidTy::Lava => {
                        if aquifer.should_schedule_fluid_update() {
                            marks.push(crate::aquifer263::pos_as_long(world_x, world_y, world_z));
                        }
                        Some(LAVA_ID)
                    }
                };
                if let Some(w) = write {
                    if pre != w {
                        changed.push(((idx as i64) << 16) | w as i64);
                        io[idx] = w;
                    }
                }
                // vanilla grass→dirt topMaterial: helperPos = the block below
                // (DIRT-checked on the live state). The rule runs for AIR
                // carves too — underFluid = !state.getFluidState().isEmpty()
                // only feeds updateY's waterHeight.
                if has_grass && world_y > self.min_y && top_ready {
                    let below_idx = idx - 256;
                    let below = io[below_idx];
                    if top_ids.is_dirt(below) {
                        let under_fluid = !matches!(ty, crate::aquifer263::FluidTy::Air);
                        let tm = crate::surface263::top_material263(
                            sf.unwrap(),
                            b263.unwrap(),
                            zoom_seed,
                            wg_hm,
                            self.min_y,
                            self.height,
                            self.sea_level,
                            world_x,
                            world_y - 1,
                            world_z,
                            under_fluid,
                        );
                        if let Some(tm) = tm {
                            // vanilla setBlockState unconditionally; mark per
                            // the material's fluid state (independent of change)
                            io[below_idx] = tm;
                            changed.push(((below_idx as i64) << 16) | tm as i64);
                            if top_ids.is_fluid(tm) {
                                marks.push(crate::aquifer263::pos_as_long(
                                    world_x,
                                    world_y - 1,
                                    world_z,
                                ));
                            }
                        }
                    }
                }
            }
            true
        });
        marks
    }
}


/// One section's decoded payload for the carve JNI.
pub struct SectionPack<'a> {
    /// 24 entries: bits (0 = single-value palette, -1 = absent section).
    pub bits: &'a [i32],
    /// Concatenated BitStorage raw longs (offsets via raw_off).
    pub raw: &'a [i64],
    pub raw_off: &'a [i32],
    /// Concatenated palette id tables (our u16 ids; unknown states -> 1).
    pub pal: &'a [i16],
    pub pal_off: &'a [i32],
}

impl Gen263 {
    /// JNI carve pipeline: decode sections -> carver mask + apply -> changed
    /// cells + fluid marks. `ops` receives (idx << 16 | newId) for every
    /// changed cell (substance writes + the topMaterial grass→dirt fix);
    /// Java applies them via the NMS state table. `sf`/`b263` = the surface
    /// + biome engine state for the topMaterial context (None degrades the
    /// fix to keep-dirt).
    #[allow(clippy::too_many_arguments)]
    pub fn carve_chunk(
        &self,
        cx: i32,
        cz: i32,
        seed: i64,
        pack: &SectionPack,
        io: &mut [u16; OUT_LEN],
        marks: &mut Vec<i64>,
        ops: &mut Vec<i64>,
        sf: Option<&crate::surface263::SurfaceState263>,
        b263: Option<&std::sync::Arc<crate::biome263::BiomeState263>>,
    ) {
        io.fill(0);
        for s in 0..24usize {
            let bits = pack.bits[s];
            if bits < 0 {
                continue; // absent section — stays air
            }
            let base = s * 4096;
            if bits == 0 {
                let id = pack.pal[pack.pal_off[s] as usize] as u16;
                io[base..base + 4096].fill(id);
            } else {
                let ro = pack.raw_off[s] as usize;
                let po = pack.pal_off[s] as usize;
                for i in 0..4096usize {
                    let bit_idx = i * bits as usize;
                    let word = bit_idx / 64;
                    let shift = bit_idx % 64;
                    let mut v = (pack.raw[ro + word] >> shift) as i64;
                    if shift + bits as usize > 64 {
                        v |= pack.raw[ro + word + 1] << (64 - shift);
                    }
                    let v = (v & ((1i64 << bits) - 1)) as usize;
                    io[base + i] = pack.pal[po + v] as u16;
                }
            }
        }
        let top_ids = crate::surface263::TopMatIds::build(sf);
        let wg_hm = take_wg_heightmap(cx, cz);
        let changed_marks = self.apply_carvers(
            cx,
            cz,
            seed,
            io,
            ops,
            sf,
            b263,
            wg_hm.as_deref(),
            &top_ids,
        );
        marks.extend(changed_marks);
    }
}


/// Strip-parallel worker count: CRUS_FILL_THREADS_263 overrides, else
/// available_parallelism (min 1). 1 disables the strip path entirely.
fn fill_threads() -> usize {
    if let Ok(v) = std::env::var("CRUS_FILL_THREADS_263") {
        if let Ok(n) = v.parse::<usize>() {
            return n.max(1);
        }
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(4)
}

/// Split `total` rows into at most `parts` contiguous strips (balanced).
fn split_strips(total: i32, parts: usize) -> Vec<(i32, i32)> {
    let parts = parts.min(total.max(1) as usize);
    let base = total / parts as i32;
    let rem = total % parts as i32;
    let mut out = Vec::with_capacity(parts);
    let mut z = 0i32;
    for i in 0..parts as i32 {
        let len = base + if i < rem { 1 } else { 0 };
        out.push((z, z + len));
        z += len;
    }
    out
}


/// Copy a strip buffer's z-rows into the full out buffer (JNI layout:
/// index = (y-minY)*256 + z*16 + x — a z row is 16 contiguous u16s).
fn merge_strip_rows(src: &[u16; OUT_LEN], dst: &mut [u16; OUT_LEN], z0: i32, z1: i32) {
    // copy only the strip's own global z rows (y relative: idx = y*256 + z*16 + x)
    for y in 0..384i32 {
        for z in z0..z1 {
            let start = (y * 256 + z * 16) as usize;
            dst[start..start + 16].copy_from_slice(&src[start..start + 16]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fusion A/B: the compiled flat program must be bit-identical to the
    /// tree walk on every sampled volume shape (full chunk, z-strip,
    /// single column). Fresh ctx per path — cache-cell state is pure
    /// memoization, but a fresh ctx keeps the A/B honest end to end.
    #[test]
    fn fuse_bit_exact_vs_tree_walk() {
        let g = Gen263::new(3257840388504953787, -64, 384, 63);
        let prog = g
            .final_density_fuse
            .as_ref()
            .expect("final_density must compile to a fuse program (stack budget 48)");
        assert!(!prog.ops.is_empty());
        assert!(!prog.slots.is_empty());

        let volumes: Vec<DensityVolume> = vec![
            // full chunk
            DensityVolume::block_step((16, 384, 16), (16 * 3, -64, 16 * 7)),
            DensityVolume::block_step((16, 384, 16), (-16 * 11, -64, -16 * 3)),
            // z-strip (fill_strip shape)
            DensityVolume {
                size_x: 16,
                size_y: 384,
                size_z: 5,
                min_x: 16 * 3,
                min_y: -64,
                min_z: 16 * 7 + 3,
                step_x: 1,
                step_y: 1,
                step_z: 1,
            },
            // single column (base_height shape)
            DensityVolume::block_step((1, 384, 1), (-17, -64, 42)),
            // cell lattice shape (4/8/4 strided, as under Interpolated)
            DensityVolume {
                size_x: 5,
                size_y: 49,
                size_z: 5,
                min_x: 0,
                min_y: -64,
                min_z: 0,
                step_x: 4,
                step_y: 8,
                step_z: 4,
            },
        ];

        for (vi, vol) in volumes.iter().enumerate() {
            let mut walk = vec![0f32; vol.size()];
            let mut fuse = vec![0f32; vol.size()];
            {
                let mut ctx = SamplerCtx263::cached();
                ctx.beard = crate::beard263::Beardifier263::decode(&[0, 0]);
                g.final_density.sample_volume(&mut ctx, &mut walk, vol);
            }
            {
                let mut ctx = SamplerCtx263::cached();
                ctx.beard = crate::beard263::Beardifier263::decode(&[0, 0]);
                prog.run(&mut ctx, &mut fuse, vol);
            }
            let bad = walk
                .iter()
                .zip(fuse.iter())
                .position(|(a, b)| a.to_bits() != b.to_bits());
            assert!(
                bad.is_none(),
                "volume #{vi}: fused program diverged at index {:?} (walk {:08x} vs fuse {:08x})",
                bad,
                walk[bad.unwrap_or(0)].to_bits(),
                fuse[bad.unwrap_or(0)].to_bits()
            );
        }
    }
}
