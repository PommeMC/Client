//! Ambient-only 26.3 BrushableBlock; gameplay remains table-driven.

use std::rc::Rc;

use azalea_block::BlockState;

use super::super::{Block, BlockData};
use super::falling_block::emit_falling_dust;
use crate::particles::block_particle_option::BlockParticleOption;
use crate::particles::types::ParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct BrushableBlock {
    block_data: BlockData,
    falling_dust: Rc<ParticleType<BlockParticleOption>>,
}

impl BrushableBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        falling_dust: Rc<ParticleType<BlockParticleOption>>,
    ) -> Self {
        Self {
            block_data,
            falling_dust,
        }
    }
}

impl Block for BrushableBlock {
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
    }
}
