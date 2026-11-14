use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct MyceliumBlock {
    block_data: BlockData,
    mycelium: SimpleParticleType,
}

impl MyceliumBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        mycelium: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            mycelium,
        }
    }
}

impl Block for MyceliumBlock {
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
        if random.usize(..10) != 0 {
            return;
        }
        let position = DVec3::new(
            f64::from(pos.x) + random.f64(),
            f64::from(pos.y) + 1.1_f64,
            f64::from(pos.z) + random.f64(),
        );
        level.add_particle(&self.mycelium, position, DVec3::ZERO);
    }
}
