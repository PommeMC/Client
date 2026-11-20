//! Ambient behavior from 26.3 FallingBlock, SandBlock and ParticleUtils.

use std::rc::Rc;

use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::block_particle_option::BlockParticleOption;
use crate::particles::types::ParticleType;
use crate::position::{BlockPos, ChunkPos};
use crate::world::level::LevelContext;

pub(in crate::world::block) struct FallingBlock {
    block_data: BlockData,
    falling_dust: Rc<ParticleType<BlockParticleOption>>,
    sand_sounds: bool,
}

impl FallingBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        falling_dust: Rc<ParticleType<BlockParticleOption>>,
        sand_sounds: bool,
    ) -> Self {
        Self {
            block_data,
            falling_dust,
            sand_sounds,
        }
    }
}

impl Block for FallingBlock {
    fn block_data(&self) -> &BlockData {
        &self.block_data
    }

    fn animate_tick(
        &self,
        state: BlockState,
        pos: BlockPos,
        level: &mut LevelContext<'_>,
        random: &mut fastrand::Rng,
    ) {
        emit_falling_dust(&self.falling_dust, state, pos, level, random);
        if self.sand_sounds {
            let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
            let chunks = level.chunks();
            let chunk_pos = ChunkPos::new(above.x.div_euclid(16), above.z.div_euclid(16));
            if above.y >= chunks.min_y()
                && i64::from(above.y) < i64::from(chunks.min_y()) + i64::from(chunks.height())
                && chunks.get_chunk(&chunk_pos).is_some()
                && level
                    .block_state_table()
                    .block_state_data(level.get_block_state(above))
                    .id()
                    == "air"
            {
                // COMPAT: Preserve the trailing callback draw, not unavailable
                // WORLD_SURFACE/audio gates or their conditional LEVEL nextLong.
                let _sound_chance = random.usize(0..2100);
            }
        }
    }
}

pub(super) fn emit_falling_dust(
    falling_dust: &Rc<ParticleType<BlockParticleOption>>,
    state: BlockState,
    pos: BlockPos,
    level: &mut LevelContext<'_>,
    random: &mut fastrand::Rng,
) {
    if random.usize(0..16) != 0 {
        return;
    }
    let below = level.get_block_state(BlockPos::new(pos.x, pos.y - 1, pos.z));
    let data = level.block_state_table().block_state_data(below);
    // Source liquid() is not fluid presence (which also includes waterlogging).
    if !(data.is_air()
        || level.is_block_tag(below, "minecraft:fire")
        || matches!(data.id(), "water" | "lava" | "bubble_column"))
    {
        let Some(replaceable) = data.replaceable else {
            return;
        };
        if !replaceable {
            return;
        }
    }
    let position = DVec3::new(
        f64::from(pos.x) + random.f64(),
        f64::from(pos.y) - 0.05,
        f64::from(pos.z) + random.f64(),
    );
    level.add_particle(
        &BlockParticleOption::new(Rc::clone(falling_dust), state),
        position,
        DVec3::ZERO,
    );
}
