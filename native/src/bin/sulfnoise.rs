use crussty_chunkgen::df263::NoiseFactory263;
use crussty_chunkgen::n263::{NormalNoiseParameters, Normalization};

/// cont-54: compare the native SULFUR_CAVE_GRADIENT at the exact divergent
/// blocks found by the chunkdump A/B (native painted X, vanilla painted Y).
/// Params = data/minecraft/worldgen/noise/sulfur_cave_gradient.json
/// (base_amplitude 1.1817507833955654, base_octave -5, octave_count 3,
/// amplitude_modifiers [1.0, 0.0, 1.0], normalize default ENABLED).
fn main() {
    let mut f = NoiseFactory263::new(3257840388504953787);
    let params = NormalNoiseParameters {
        base_amplitude: 1.1817507833955654,
        base_octave: -5,
        octave_count: 3,
        normalize: Normalization::Enabled,
        amplitude_modifiers: vec![1.0, 0.0, 1.0],
    };
    let n = f.normal("sulfur_cave_gradient", params);
    let blocks: [(i32, i32, i32, &str, &str); 26] = [
        (16348, -9, 16557, "DEEPSLATE", "SULFUR"),
        (16342, 21, 16558, "SULFUR", "CINNABAR"),
        (16337, 19, 16545, "CINNABAR", "SULFUR"),
        (16344, -2, 16555, "SULFUR", "DEEPSLATE"),
        (16350, 12, 16550, "SULFUR", "CINNABAR"),
        (16346, -9, 16555, "SULFUR", "DEEPSLATE"),
        (16338, -7, 16550, "SULFUR", "DEEPSLATE"),
        (16340, 22, 16559, "SULFUR", "CINNABAR"),
        (16342, 13, 16547, "SULFUR", "CINNABAR"),
        (16343, -9, 16556, "SULFUR", "CINNABAR"),
        (16342, -12, 16554, "SULFUR", "DEEPSLATE"),
        (16340, -8, 16556, "SULFUR", "CINNABAR"),
        (16343, 2, 16555, "SULFUR", "DEEPSLATE"),
        (16337, -9, 16554, "SULFUR", "CINNABAR"),
        (16348, -1, 16551, "SULFUR", "DEEPSLATE"),
        (16345, 16, 16554, "SULFUR", "CINNABAR"),
        (16351, -1, 16550, "SULFUR", "DEEPSLATE"),
        (16347, -6, 16553, "SULFUR", "CINNABAR"),
        (16349, -11, 16552, "SULFUR", "CINNABAR"),
        (16351, -2, 16553, "SULFUR", "CINNABAR"),
        (16338, -3, 16554, "SULFUR", "DEEPSLATE"),
        (16351, 31, 16551, "SULFUR", "CINNABAR"),
        (16336, -12, 16546, "SULFUR", "DEEPSLATE"),
        (16341, 23, 16545, "CINNABAR", "SULFUR"),
        (16350, 37, 16556, "CINNABAR", "SULFUR"),
        (16341, -17, 16549, "SULFUR", "CINNABAR"),
    ];
    for (x, y, z, mn, mv) in blocks {
        let v = n.get(x as f64, y as f64, z as f64);
        println!("grad({x},{y},{z}) native={v:.9?} native_mat={mn} vanilla_mat={mv}");
    }
}
