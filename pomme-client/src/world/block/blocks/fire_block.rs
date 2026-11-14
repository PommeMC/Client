use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData, BlockStateData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::block::model::Direction;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct FireBlock {
    block_data: BlockData,
    large_smoke: SimpleParticleType,
}

impl FireBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        large_smoke: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            large_smoke,
        }
    }

    fn can_burn(state: &BlockStateData) -> bool {
        if state.properties().get("waterlogged") == Some("true") {
            return false;
        }
        let name = match state.id().split_once(':') {
            Some(("minecraft", path)) => path,
            Some(_) => return false,
            None => state.id(),
        };
        // COMPAT: 1.20.1 FireBlock.bootStrap registers GRASS; selected older
        // tables retain that name for modern short_grass. No other aliases.
        let name = if name == "grass" { "short_grass" } else { name };
        // 26.3 FireBlock.bootStrap: exact positive igniteOdds membership,
        // including all colors of WOOL, WOOL_STAIRS, WOOL_SLAB and CARPET.
        matches!(
            name,
            "acacia_fence"
                | "acacia_fence_gate"
                | "acacia_leaves"
                | "acacia_log"
                | "acacia_planks"
                | "acacia_shelf"
                | "acacia_slab"
                | "acacia_stairs"
                | "acacia_wood"
                | "allium"
                | "azalea"
                | "azalea_leaves"
                | "azure_bluet"
                | "bamboo"
                | "bamboo_block"
                | "bamboo_fence"
                | "bamboo_fence_gate"
                | "bamboo_mosaic"
                | "bamboo_mosaic_slab"
                | "bamboo_mosaic_stairs"
                | "bamboo_planks"
                | "bamboo_shelf"
                | "bamboo_slab"
                | "bamboo_stairs"
                | "bee_nest"
                | "beehive"
                | "big_dripleaf"
                | "big_dripleaf_stem"
                | "birch_fence"
                | "birch_fence_gate"
                | "birch_leaves"
                | "birch_log"
                | "birch_planks"
                | "birch_shelf"
                | "birch_slab"
                | "birch_stairs"
                | "birch_wood"
                | "black_carpet"
                | "black_wool"
                | "black_wool_slab"
                | "black_wool_stairs"
                | "blue_carpet"
                | "blue_orchid"
                | "blue_wool"
                | "blue_wool_slab"
                | "blue_wool_stairs"
                | "bookshelf"
                | "brown_carpet"
                | "brown_wool"
                | "brown_wool_slab"
                | "brown_wool_stairs"
                | "bush"
                | "cactus_flower"
                | "cave_vines"
                | "cave_vines_plant"
                | "cherry_fence"
                | "cherry_fence_gate"
                | "cherry_leaves"
                | "cherry_log"
                | "cherry_planks"
                | "cherry_shelf"
                | "cherry_slab"
                | "cherry_stairs"
                | "cherry_wood"
                | "closed_eyeblossom"
                | "coal_block"
                | "composter"
                | "cornflower"
                | "cyan_carpet"
                | "cyan_wool"
                | "cyan_wool_slab"
                | "cyan_wool_stairs"
                | "dandelion"
                | "dark_oak_fence"
                | "dark_oak_fence_gate"
                | "dark_oak_leaves"
                | "dark_oak_log"
                | "dark_oak_planks"
                | "dark_oak_shelf"
                | "dark_oak_slab"
                | "dark_oak_stairs"
                | "dark_oak_wood"
                | "dead_bush"
                | "dried_kelp_block"
                | "fern"
                | "firefly_bush"
                | "flowering_azalea"
                | "flowering_azalea_leaves"
                | "glow_lichen"
                | "golden_dandelion"
                | "gray_carpet"
                | "gray_wool"
                | "gray_wool_slab"
                | "gray_wool_stairs"
                | "green_carpet"
                | "green_wool"
                | "green_wool_slab"
                | "green_wool_stairs"
                | "hanging_roots"
                | "hay_block"
                | "jungle_fence"
                | "jungle_fence_gate"
                | "jungle_leaves"
                | "jungle_log"
                | "jungle_planks"
                | "jungle_shelf"
                | "jungle_slab"
                | "jungle_stairs"
                | "jungle_wood"
                | "large_fern"
                | "leaf_litter"
                | "lectern"
                | "light_blue_carpet"
                | "light_blue_wool"
                | "light_blue_wool_slab"
                | "light_blue_wool_stairs"
                | "light_gray_carpet"
                | "light_gray_wool"
                | "light_gray_wool_slab"
                | "light_gray_wool_stairs"
                | "lilac"
                | "lily_of_the_valley"
                | "lime_carpet"
                | "lime_wool"
                | "lime_wool_slab"
                | "lime_wool_stairs"
                | "magenta_carpet"
                | "magenta_wool"
                | "magenta_wool_slab"
                | "magenta_wool_stairs"
                | "mangrove_fence"
                | "mangrove_fence_gate"
                | "mangrove_leaves"
                | "mangrove_log"
                | "mangrove_planks"
                | "mangrove_roots"
                | "mangrove_shelf"
                | "mangrove_slab"
                | "mangrove_stairs"
                | "mangrove_wood"
                | "oak_fence"
                | "oak_fence_gate"
                | "oak_leaves"
                | "oak_log"
                | "oak_planks"
                | "oak_shelf"
                | "oak_slab"
                | "oak_stairs"
                | "oak_wood"
                | "open_eyeblossom"
                | "orange_carpet"
                | "orange_poplar_leaves"
                | "orange_tulip"
                | "orange_wool"
                | "orange_wool_slab"
                | "orange_wool_stairs"
                | "oxeye_daisy"
                | "pale_hanging_moss"
                | "pale_moss_block"
                | "pale_moss_carpet"
                | "pale_oak_fence"
                | "pale_oak_fence_gate"
                | "pale_oak_leaves"
                | "pale_oak_log"
                | "pale_oak_planks"
                | "pale_oak_shelf"
                | "pale_oak_slab"
                | "pale_oak_stairs"
                | "pale_oak_wood"
                | "peony"
                | "pink_carpet"
                | "pink_petals"
                | "pink_tulip"
                | "pink_wool"
                | "pink_wool_slab"
                | "pink_wool_stairs"
                | "pitcher_plant"
                | "poplar_fence"
                | "poplar_fence_gate"
                | "poplar_log"
                | "poplar_planks"
                | "poplar_shelf"
                | "poplar_slab"
                | "poplar_stairs"
                | "poplar_wood"
                | "poppy"
                | "purple_carpet"
                | "purple_wool"
                | "purple_wool_slab"
                | "purple_wool_stairs"
                | "red_carpet"
                | "red_poplar_leaves"
                | "red_shrub"
                | "red_tulip"
                | "red_wool"
                | "red_wool_slab"
                | "red_wool_stairs"
                | "rose_bush"
                | "scaffolding"
                | "short_dry_grass"
                | "short_grass"
                | "small_dripleaf"
                | "spore_blossom"
                | "spruce_fence"
                | "spruce_fence_gate"
                | "spruce_leaves"
                | "spruce_log"
                | "spruce_planks"
                | "spruce_shelf"
                | "spruce_slab"
                | "spruce_stairs"
                | "spruce_wood"
                | "straw_bed"
                | "stripped_acacia_log"
                | "stripped_acacia_wood"
                | "stripped_bamboo_block"
                | "stripped_birch_log"
                | "stripped_birch_wood"
                | "stripped_cherry_log"
                | "stripped_cherry_wood"
                | "stripped_dark_oak_log"
                | "stripped_dark_oak_wood"
                | "stripped_jungle_log"
                | "stripped_jungle_wood"
                | "stripped_mangrove_log"
                | "stripped_mangrove_wood"
                | "stripped_oak_log"
                | "stripped_oak_wood"
                | "stripped_pale_oak_log"
                | "stripped_pale_oak_wood"
                | "stripped_poplar_log"
                | "stripped_poplar_wood"
                | "stripped_spruce_log"
                | "stripped_spruce_wood"
                | "sunflower"
                | "sweet_berry_bush"
                | "tall_dry_grass"
                | "tall_grass"
                | "target"
                | "tnt"
                | "torchflower"
                | "vine"
                | "white_carpet"
                | "white_tulip"
                | "white_wool"
                | "white_wool_slab"
                | "white_wool_stairs"
                | "wildflowers"
                | "wither_rose"
                | "yellow_carpet"
                | "yellow_poplar_leaves"
                | "yellow_wool"
                | "yellow_wool_slab"
                | "yellow_wool_stairs"
        )
    }
}

impl Block for FireBlock {
    fn block_data(&self) -> &BlockData {
        &self.block_data
    }

    fn animate_tick(
        &self,
        _state: BlockState,
        pos: BlockPos,
        level: &mut LevelContext<'_>,
        random: &mut fastrand::Rng,
    ) {
        animate_fire(pos, &self.large_smoke, level, random, Self::can_burn);
    }
}

pub(in crate::world::block) struct SoulFireBlock {
    block_data: BlockData,
    large_smoke: SimpleParticleType,
}

impl SoulFireBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        large_smoke: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            large_smoke,
        }
    }

    fn can_burn(_state: &BlockStateData) -> bool {
        true
    }
}

impl Block for SoulFireBlock {
    fn block_data(&self) -> &BlockData {
        &self.block_data
    }

    fn animate_tick(
        &self,
        _state: BlockState,
        pos: BlockPos,
        level: &mut LevelContext<'_>,
        random: &mut fastrand::Rng,
    ) {
        animate_fire(pos, &self.large_smoke, level, random, Self::can_burn);
    }
}

// Vanilla BaseFireBlock.animateTick; only ambient particles, not fire
// simulation.
fn animate_fire(
    pos: BlockPos,
    large_smoke: &SimpleParticleType,
    level: &mut LevelContext<'_>,
    random: &mut fastrand::Rng,
    can_burn: fn(&BlockStateData) -> bool,
) {
    if random.u32(0..24) == 0 {
        // COMPAT: Audio is excluded; preserve the volume then pitch RNG draws.
        let _ = 1.0_f32 + random.f32();
        let _ = random.f32() * 0.7_f32 + 0.3_f32;
    }

    let below = level.get_block_state(pos.relative(Direction::Down));
    let below_data = level.block_state_table().block_state_data(below);
    let floor = can_burn(below_data)
        || below_data
            .is_full_face_sturdy(Direction::Up)
            .unwrap_or(false);
    let directions: &[Direction] = if floor {
        &[Direction::Down]
    } else {
        &[
            Direction::West,
            Direction::East,
            Direction::North,
            Direction::South,
            Direction::Up,
        ]
    };
    let x = f64::from(pos.x);
    let y = f64::from(pos.y);
    let z = f64::from(pos.z);
    let thickness = f64::from(0.1_f32);
    for &direction in directions {
        if !floor {
            let neighbor = level.get_block_state(pos.relative(direction));
            if !can_burn(level.block_state_table().block_state_data(neighbor)) {
                continue;
            }
        }
        for _ in 0..if floor { 3 } else { 2 } {
            let rx = random.f64();
            let ry = random.f64();
            let rz = random.f64();
            let position = match direction {
                Direction::Down => DVec3::new(x + rx, y + ry * 0.5 + 0.5, z + rz),
                Direction::West => DVec3::new(x + rx * thickness, y + ry, z + rz),
                Direction::East => DVec3::new(x + 1.0 - rx * thickness, y + ry, z + rz),
                Direction::North => DVec3::new(x + rx, y + ry, z + rz * thickness),
                Direction::South => DVec3::new(x + rx, y + ry, z + 1.0 - rz * thickness),
                Direction::Up => DVec3::new(x + rx, y + 1.0 - ry * thickness, z + rz),
            };
            level.add_particle(large_smoke, position, DVec3::ZERO);
        }
    }
}
