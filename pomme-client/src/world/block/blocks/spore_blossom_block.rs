use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::physics::block_shape::is_collision_shape_full_block;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct SporeBlossomBlock {
    block_data: BlockData,
    falling_spore_blossom: SimpleParticleType,
    spore_blossom_air: SimpleParticleType,
}

impl SporeBlossomBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        falling_spore_blossom: SimpleParticleType,
        spore_blossom_air: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            falling_spore_blossom,
            spore_blossom_air,
        }
    }
}

impl Block for SporeBlossomBlock {
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
        let position = DVec3::new(
            f64::from(pos.x) + random.f64(),
            f64::from(pos.y) + 0.7_f64,
            f64::from(pos.z) + random.f64(),
        );
        level.add_particle(&self.falling_spore_blossom, position, DVec3::ZERO);

        for _ in 0..14 {
            let candidate = BlockPos::new(
                pos.x + (random.i32(0..21) - 10),
                pos.y - random.i32(0..10),
                pos.z + (random.i32(0..21) - 10),
            );
            let state = level.get_block_state(candidate);
            let state_data = level.block_state_table().block_state_data(state);
            if !is_collision_shape_full_block(state_data) {
                let position = DVec3::new(
                    f64::from(candidate.x) + random.f64(),
                    f64::from(candidate.y) + random.f64(),
                    f64::from(candidate.z) + random.f64(),
                );
                level.add_particle(&self.spore_blossom_air, position, DVec3::ZERO);
            }
        }
    }
}
