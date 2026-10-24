//! fill263 probe — aquifer/fill internals for a known-divergent column
//! (chunk 0,0 x=5 z=0): density buffer values + computeSubstance trace.

use crussty_chunkgen::aquifer263::{Aquifer263, FluidTy};
use crussty_chunkgen::df263::{DensityVolume, SamplerCtx263};
use crussty_chunkgen::fill263::Gen263;

fn main() {
    let g = Gen263::new(3257840388504953787, -64, 384, 63);
    let (cx, cz) = (-2, 3);
    let (x, z) = (0, 0);
    let volume = DensityVolume::block_step((16, 384, 16), (cx * 16, -64, cz * 16));
    let mut ctx = SamplerCtx263::cached();
    let mut aquifer = Aquifer263::new(
        &mut ctx,
        crussty_chunkgen::aquifer263::AquiferConfig263 {
            barrier: g.aquifer_cfg.barrier.clone(),
            floodedness: g.aquifer_cfg.floodedness.clone(),
            spread: g.aquifer_cfg.spread.clone(),
            lava: g.aquifer_cfg.lava.clone(),
            exclusion: g.aquifer_cfg.exclusion.clone(),
            surface_level: g.aquifer_cfg.surface_level.clone(),
        },
        g.aquifer_factory.clone(),
        &volume,
        63,
    );
    eprintln!("skip_sampling_above_y = {}", aquifer.skip_sampling_above_y);
    let mut buf = ctx.scratch.acquire(volume.size());
    g.final_density.sample_volume(&mut ctx, &mut buf, &volume);
    for y_rel in [200usize, 128, 127, 112, 110, 103] {
        let density = buf[volume.index_unchecked(x, y_rel as i32, z)];
        let by = -64 + y_rel as i32;
        let sub = aquifer.compute_substance(&mut ctx, x, by, z, density as f64);
        let sub_str = match sub {
            None => "null(stone)".to_string(),
            Some(FluidTy::Air) => "AIR".into(),
            Some(FluidTy::Water) => "WATER".into(),
            Some(FluidTy::Lava) => "LAVA".into(),
        };
        eprintln!(
            "y_rel={y_rel} blockY={by} density={density:.6} (bits {:08x}) -> {sub_str} mark={}",
            density.to_bits(),
            aquifer.should_schedule_fluid_update() as i32
        );
    }
    // exclusion/floodedness raw samples at one spot
    let ex = g.aquifer_cfg.exclusion.sample_value(&mut ctx, x as i32, 0, z as i32);
    let fl = g.aquifer_cfg.floodedness.sample_value(&mut ctx, x as i32, 0, z as i32);
    let sl = g.aquifer_cfg.surface_level.sample_value(&mut ctx, x as i32, 0, z as i32);
    eprintln!("exclusion(5,0,0)={ex} floodedness={fl} surface_level={sl}");
}
