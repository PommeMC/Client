use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::physics::block_shape::outline_shape;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct WitherRoseBlock {
    block_data: BlockData,
    smoke: SimpleParticleType,
}

impl WitherRoseBlock {
    pub(in crate::world::block) fn new(block_data: BlockData, smoke: SimpleParticleType) -> Self {
        Self { block_data, smoke }
    }
}

impl Block for WitherRoseBlock {
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
        let table = level.block_state_table();
        let data = table.block_state_data(state);
        let zero = data.block_position_offset(0, 0);
        let actual = data.block_position_offset(pos.x, pos.z);
        // Remove the baked ZERO offset before moving each local outline bound.
        let mut bounds = outline_shape(data).iter().map(|b| {
            [
                (b[0] - zero.x) + actual.x,
                (b[2] - zero.z) + actual.z,
                (b[3] - zero.x) + actual.x,
                (b[5] - zero.z) + actual.z,
            ]
        });
        let Some([mut min_x, mut min_z, mut max_x, mut max_z]) = bounds.next() else {
            return;
        };
        for [x0, z0, x1, z1] in bounds {
            min_x = min_x.min(x0);
            min_z = min_z.min(z0);
            max_x = max_x.max(x1);
            max_z = max_z.max(z1);
        }
        let center_x = min_x + 0.5_f64 * (max_x - min_x);
        let center_z = min_z + 0.5_f64 * (max_z - min_z);
        let x = f64::from(pos.x) + center_x;
        let z = f64::from(pos.z) + center_z;

        for _ in 0..3 {
            if random.bool() {
                let position = DVec3::new(
                    x + random.f64() / 5.0,
                    f64::from(pos.y) + (0.5 - random.f64()),
                    z + random.f64() / 5.0,
                );
                level.add_particle(&self.smoke, position, DVec3::ZERO);
            }
        }
    }
}
