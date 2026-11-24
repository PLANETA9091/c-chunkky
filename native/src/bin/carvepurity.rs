// cont-151 diagnostic: carve ORDER-PURITY probe.
// Carves a synthetic all-stone pack at (1033,1033) twice — alone vs after
// carving (1024,1024)+(1023,1023) on the same Gen263 instance — and diffs
// the ops streams. A nonzero diff = order-dependent shared state in the
// carve pipeline (the radial dAIR gradient suspect).
use crussty_chunkgen::fill263::{Gen263, SectionPack};
use crussty_chunkgen::nochunk::OUT_LEN;

fn stone_pack() -> (Vec<i32>, Vec<i64>, Vec<i32>, Vec<i16>, Vec<i32>) {
    let bits: Vec<i32> = vec![0i32; 24]; // every section uniform single-value palette
    let raw = Vec::new();
    let raw_off: Vec<i32> = vec![0i32; 24];
    let pal = vec![1i16]; // stone
    let pal_off: Vec<i32> = vec![0i32; 24];
    (bits, raw, raw_off, pal, pal_off)
}

fn carve_at(g: &Gen263, cx: i32, cz: i32, p: &SectionPack) -> (Vec<i64>, Vec<i64>) {
    let mut io = [0u16; OUT_LEN];
    let mut marks = Vec::new();
    let mut ops = Vec::new();
    g.carve_chunk(cx, cz, 3257840388504953787, p, &mut io, &mut marks, &mut ops, None, None, false);
    (ops, marks)
}

fn main() {
    let (bits, raw, raw_off, pal, pal_off) = stone_pack();
    let p = SectionPack { bits: &bits, raw: &raw, raw_off: &raw_off, pal: &pal, pal_off: &pal_off };
    let g = Gen263::new(3257840388504953787, -64, 384, 63);

    // Per-chunk op counts over the 12x12 region (ops, then a second pass to
    // verify determinism) — compare with the dAIR matrix from the A/B pair.
    let mut first = Vec::new();
    for cz in 1022..1034 {
        for cx in 1022..1034 {
            let (ops, _) = carve_at(&g, cx, cz, &p);
            first.push(ops.len());
        }
    }
    let mut second = Vec::new();
    for cz in 1022..1034 {
        for cx in 1022..1034 {
            let (ops, _) = carve_at(&g, cx, cz, &p);
            second.push(ops.len());
        }
    }
    println!("pass1 vs pass2 identical: {}", first == second);
    for cz in 0..12 {
        for cx in 0..12 {
            print!("{} ", first[cz * 12 + cx]);
        }
        println!();
    }
}
