// cont-151: native aquifer substance decisions (density=0.0) for a chunk's
// y-band — mirrors CarveSubstanceOracle.java (real vanilla aquifer) cell for
// cell. Output: "wx wy wz decision" lines.
use crussty_chunkgen::aquifer263::FluidTy;
use crussty_chunkgen::df263::{DensityVolume, SamplerCtx263};
use crussty_chunkgen::fill263::Gen263;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cx: i32 = args[1].parse().unwrap();
    let cz: i32 = args[2].parse().unwrap();
    let y_lo: i32 = args[3].parse().unwrap();
    let y_hi: i32 = args[4].parse().unwrap();

    let g = Gen263::new(3257840388504953787, -64, 384, 63);
    let volume = DensityVolume::block_step((16, 384, 16), (cx * 16, -64, cz * 16));
    let mut ctx = SamplerCtx263::cached();
    let mut aquifer = g.aquifer_for(&mut ctx, &volume);

    let mut out = String::with_capacity(1 << 18);
    for lx in 0..16 {
        for lz in 0..16 {
            let wx = cx * 16 + lx;
            let wz = cz * 16 + lz;
            for wy in y_lo..=y_hi {
                let d = match aquifer.compute_substance(&mut ctx, wx, wy, wz, 0.0) {
                    None => "null",
                    Some(FluidTy::Air) => "air",
                    Some(FluidTy::Water) => "water",
                    Some(FluidTy::Lava) => "lava",
                };
                out.push_str(&format!("{} {} {} {}\n", wx, wy, wz, d));
            }
        }
    }
    print!("{}", out);
}
