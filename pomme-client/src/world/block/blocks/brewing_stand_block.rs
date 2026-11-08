use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct BrewingStandBlock {
    block_data: BlockData,
    smoke: SimpleParticleType,
}

impl BrewingStandBlock {
    pub(in crate::world::block) fn new(block_data: BlockData, smoke: SimpleParticleType) -> Self {
        Self { block_data, smoke }
    }
}

impl Block for BrewingStandBlock {
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
        let x = (f64::from(pos.x) + 0.4_f64) + f64::from(random.f32()) * 0.2_f64;
        let y = (f64::from(pos.y) + 0.7_f64) + f64::from(random.f32()) * 0.3_f64;
        let z = (f64::from(pos.z) + 0.4_f64) + f64::from(random.f32()) * 0.2_f64;
        level.add_particle(&self.smoke, DVec3::new(x, y, z), DVec3::ZERO);
    }
}
