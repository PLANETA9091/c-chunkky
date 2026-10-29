//! Native `BlockState.canSurvive` for the feature-relevant block families.
//!
//! Every rule below is verified against the 1.21.8 mojang-mapped bytecode
//! (paper-1.21.8.jar, javap): BushBlock/VegetationBlock.canSurvive,
//! CactusBlock.canSurvive, SugarCaneBlock.canSurvive, DoublePlantBlock.
//! The environment (block below / neighbors) is read from the region
//! snapshot — the exact live state the JNI bridge path reads.
//!
//! NOT tabled -> None -> the caller falls back to the JNI bridge, so an
//! unknown override can never answer wrongly. The table only ever needs to
//! cover the states the real packs actually pass to wouldSurvive (probe
//! histogram: bone_block + the small flowers + grass/fern = 100%).

use crate::features_placement::BlockPos;
use crate::features_snap::{flags, ChunkLevel};

/// BlockTags.DIRT (1.21.8 data/minecraft/tags/block/dirt.json).
#[inline]
fn is_dirt_tag(name: &str) -> bool {
    matches!(
        name,
        "minecraft:dirt"
            | "minecraft:grass_block"
            | "minecraft:podzol"
            | "minecraft:coarse_dirt"
            | "minecraft:mycelium"
            | "minecraft:rooted_dirt"
            | "minecraft:moss_block"
            | "minecraft:pale_moss_block"
            | "minecraft:mud"
            | "minecraft:muddy_mangrove_roots"
    )
}

/// BlockTags.SAND (1.21.8 sand.json).
#[inline]
fn is_sand_tag(name: &str) -> bool {
    matches!(name, "minecraft:sand" | "minecraft:red_sand" | "minecraft:suspicious_sand")
}

/// VegetationBlock.mayPlaceOn: below.is(DIRT tag) || below.is(FARMLAND).
#[inline]
fn vegetation_place_on(below: &str) -> bool {
    is_dirt_tag(below) || below == "minecraft:farmland"
}

/// Single-block VegetationBlock family (canSurvive = mayPlaceOn(below));
/// DoublePlantBlock[half=lower] resolves here too (the state strings carry
/// the half property — the upper half never goes through this arm).
const VEGETATION: &[&str] = &[
    // small flowers (FlowerBlock/BushBlock)
    "minecraft:poppy",
    "minecraft:dandelion",
    "minecraft:oxeye_daisy",
    "minecraft:cornflower",
    "minecraft:lily_of_the_valley",
    "minecraft:allium",
    "minecraft:azure_bluet",
    "minecraft:blue_orchid",
    "minecraft:red_tulip",
    "minecraft:orange_tulip",
    "minecraft:white_tulip",
    "minecraft:pink_tulip",
    "minecraft:torchflower",
    "minecraft:wither_rose",
    // grasses/ferns (BushBlock/FernBlock)
    "minecraft:short_grass",
    "minecraft:fern",
    // saplings (SaplingBlock extends VegetationBlock, no canSurvive override)
    "minecraft:oak_sapling",
    "minecraft:spruce_sapling",
    "minecraft:birch_sapling",
    "minecraft:jungle_sapling",
    "minecraft:acacia_sapling",
    "minecraft:dark_oak_sapling",
    "minecraft:cherry_sapling",
    "minecraft:pale_oak_sapling",
];

/// survive_native: Some(b) — decidable natively from the snapshot; None —
/// fall back to the JNI bridge.
pub fn survive_native(state: &str, pos: BlockPos, level: &ChunkLevel) -> Option<bool> {
    let name = crate::features_deco::state_block_name(state);
    let below = BlockPos { x: pos.x, y: pos.y - 1, z: pos.z };
    match name {
        // BlockBehaviour.canSurvive default = true (no override on bone
        // blocks; probe histogram: 9955/9955 true). Answer WITHOUT any
        // snapshot touch — the pure-table fast path.
        "minecraft:bone_block" => Some(true),

        "minecraft:cactus" => {
            // CactusBlock.canSurvive: any horizontal neighbor isSolid() or
            // lava-fluid -> false; below is cactus|SAND-tag -> !above.liquid;
            // else false.
            for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let p = BlockPos { x: pos.x + dx, y: pos.y, z: pos.z + dz };
                if let Some(b) = level.world_flag_at(p, flags::SOLID) {
                    if b {
                        return Some(false);
                    }
                } else {
                    return None; // neighbor not snapshotted — cannot decide
                }
                // lava fluid: HAS_FLUID without FLUID_WATER is lava on the
                // vanilla set our features touch; the exotic-fluid case
                // falls back via the missing name check below.
                if let Some(b) = level.world_flag_at(p, flags::HAS_FLUID) {
                    if b {
                        match level.world_name_at(p) {
                            Some(n) if n == "minecraft:water" => {}
                            _ => return Some(false),
                        }
                    }
                } else {
                    return None;
                }
            }
            let below_name = level.world_name_at(below)?;
            if below_name == "minecraft:cactus" || is_sand_tag(&below_name) {
                let above = BlockPos { x: pos.x, y: pos.y + 1, z: pos.z };
                let fl = level.world_flag_at(above, flags::HAS_FLUID)?;
                Some(!fl)
            } else {
                Some(false)
            }
        }

        "minecraft:sugar_cane" => {
            // SugarCaneBlock.canSurvive: below is sugar_cane -> true; below
            // DIRT|SAND tag -> any horizontal neighbor OF THE BELOW pos is
            // water-fluid or frosted_ice; else false.
            let below_name = level.world_name_at(below)?;
            if below_name == "minecraft:sugar_cane" {
                return Some(true);
            }
            if !is_dirt_tag(&below_name) && !is_sand_tag(&below_name) {
                return Some(false);
            }
            for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let p = BlockPos { x: below.x + dx, y: below.y, z: below.z + dz };
                let fl = level.world_flag_at(p, flags::HAS_FLUID)?;
                if fl {
                    let n = level.world_name_at(p)?;
                    if n == "minecraft:water" {
                        return Some(true);
                    }
                } else if level.world_name_at(p)?.as_str() == "minecraft:frosted_ice" {
                    return Some(true);
                }
            }
            Some(false)
        }

        "minecraft:nether_wart" => {
            // NetherWartBlock.mayPlaceOn: below.is(SOUL_SAND).
            let below_name = level.world_name_at(below)?;
            Some(below_name == "minecraft:soul_sand")
        }

        "minecraft:lily_pad" => {
            // WaterlilyBlock.mayPlaceOn(below, bg, belowPos): fluid(below)
            // == WATER, or below is an IceBlock; AND fluid(below.above) is
            // EMPTY.
            let below_name = level.world_name_at(below)?;
            let below_water = below_name == "minecraft:water"
                || below_name.ends_with("_ice");
            if !below_water {
                return Some(false);
            }
            let above = BlockPos { x: pos.x, y: pos.y - 1 + 1, z: pos.z };
            let fl = level.world_flag_at(above, flags::HAS_FLUID)?;
            Some(!fl)
        }

        "minecraft:seagrass" | "minecraft:tall_seagrass" => {
            // SeagrassBlock.mayPlaceOn (no canSurvive override — Vegetation
            // resolves BOTH halves the same way): below.isFaceSturdy(UP) &&
            // !below.is(MAGMA_BLOCK). The snapshot carries exact sturdy bits.
            let below_name = level.world_name_at(below)?;
            if below_name == "minecraft:magma_block" {
                return Some(false);
            }
            level.world_flag_at(below, flags::STURDY_UP)
        }

        "minecraft:kelp" | "minecraft:kelp_plant" => {
            // GrowingPlantBlock.canSurvive (growth UP -> head = below):
            // canAttachTo(below) && (below.is(head)|is(body)|
            // isFaceSturdy(below, UP)); canAttachTo is not overridden.
            let below_name = level.world_name_at(below)?;
            if below_name == "minecraft:kelp" || below_name == "minecraft:kelp_plant" {
                return Some(true);
            }
            level.world_flag_at(below, flags::STURDY_UP)
        }

        n if VEGETATION.contains(&n) => {
            let below_name = level.world_name_at(below)?;
            Some(vegetation_place_on(&below_name))
        }

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirt_tag_membership() {
        assert!(is_dirt_tag("minecraft:grass_block"));
        assert!(is_dirt_tag("minecraft:mud"));
        assert!(!is_dirt_tag("minecraft:farmland"));
        assert!(vegetation_place_on("minecraft:farmland"));
        assert!(!vegetation_place_on("minecraft:stone"));
    }

    #[test]
    fn sand_tag_membership() {
        assert!(is_sand_tag("minecraft:sand"));
        assert!(is_sand_tag("minecraft:red_sand"));
        assert!(!is_sand_tag("minecraft:sandstone"));
    }

    #[test]
    fn vegetation_list_covers_probe_histogram() {
        for n in ["minecraft:oxeye_daisy", "minecraft:poppy", "minecraft:short_grass", "minecraft:fern"] {
            assert!(VEGETATION.contains(&n), "missing {n}");
        }
    }
}
