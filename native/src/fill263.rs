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

use crate::aquifer263::{Aquifer263, FluidTy};
use crate::df263::{DensityVolume, SamplerCtx263};
use crate::df263_router::RouterBuilder263;
use crate::nochunk::OUT_LEN;

pub const STONE_ID: u16 = 1;
pub const WATER_ID: u16 = 3;
pub const LAVA_ID: u16 = 17;

pub struct Gen263 {
    pub seed: i64,
    pub sea_level: i32,
    pub min_y: i32,
    pub height: i32,
    pub final_density: crate::df263::SamRef,
    pub aquifer_cfg: crate::aquifer263::AquiferConfig263,
    /// RandomState.getOrCreateRandomFactory("minecraft:aquifer").
    pub aquifer_factory: crate::rng::XoroshiroPositionalRandomFactory,
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
        Gen263 {
            seed,
            sea_level,
            min_y,
            height,
            final_density: router.final_density,
            aquifer_cfg,
            aquifer_factory,
        }
    }

    fn aquifer_for<'a>(
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
        let mut ctx = SamplerCtx263::cached();
        ctx.beard = crate::beard263::Beardifier263::decode(beard_list);

        // Aquifer 26.3 constructor (maxSurfaceLevel prefill) runs BEFORE the
        // final_density volume sample — the Java createNoiseChunk ordering
        // (aquifer inside NoiseChunk ctor, doFill samples after), so the
        // shared cache cells converge exactly like cachingSamplers.
        let mut aquifer = self.aquifer_for(&mut ctx, &volume);

        let mut buf = ctx.scratch.acquire(volume.size());
        self.final_density.sample_volume(&mut ctx, &mut buf, &volume);

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

    /// Native iterateNoiseColumn 26.3 (getBaseHeight): 1-cell volume walk,
    /// EMPTY beardifier, aquifer substance, tester stops the walk.
    /// mode 0 = NOT_AIR (WORLD_SURFACE*), 1 = motion-blocking
    /// (OCEAN_FLOOR* — stone in the noise-stage state space).
    pub fn base_height(&self, x: i32, z: i32, mode: u8) -> i32 {
        let volume = DensityVolume::block_step((1, self.height, 1), (x, self.min_y, z));
        let mut ctx = SamplerCtx263::cached();
        let mut aquifer = self.aquifer_for(&mut ctx, &volume);
        let mut buf = ctx.scratch.acquire(volume.size());
        self.final_density.sample_volume(&mut ctx, &mut buf, &volume);
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
    /// AIR carves, fluids write + marks. The vanilla grass→dirt
    /// topMaterial replacement is deferred to the MaterialSystem port
    /// (vanilla keeps the dirt when the optional is empty).
    /// `io` is the post-surface center chunk buffer (JNI layout).
    pub fn apply_carvers(
        &self,
        cx: i32,
        cz: i32,
        seed: i64,
        io: &mut [u16],
        changed: &mut Vec<i64>,
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
                if block == 4 {
                    has_grass = true; // grass_block (mycelium not in the id table)
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
                let _ = has_grass;
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
    /// mask cell whose block changed; Java applies them via the NMS states.
    pub fn carve_chunk(
        &self,
        cx: i32,
        cz: i32,
        seed: i64,
        pack: &SectionPack,
        io: &mut [u16; OUT_LEN],
        marks: &mut Vec<i64>,
        ops: &mut Vec<i64>,
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
        let changed_marks = self.apply_carvers(cx, cz, seed, io, ops);
        marks.extend(changed_marks);
    }
}
