use azalea_block::BlockState;
use glam::DVec3;

use super::super::model::Direction;
use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct CryingObsidianBlock {
    block_data: BlockData,
    dripping_obsidian_tear: SimpleParticleType,
}

impl CryingObsidianBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        dripping_obsidian_tear: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            dripping_obsidian_tear,
        }
    }
}

impl Block for CryingObsidianBlock {
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
        if random.usize(..5) != 0 {
            return;
        }
        let direction = Direction::VALUES[random.usize(..Direction::VALUES.len())];
        if direction == Direction::Up {
            return;
        }
        let neighbor_state = level.get_block_state(pos.relative(direction));
        let table = level.block_state_table();
        let source = table.block_state_data(state);
        let neighbor = table.block_state_data(neighbor_state);
        if !source.light_props().can_occlude
            || !neighbor
                .is_full_face_sturdy(direction.opposite())
                .unwrap_or(false)
        {
            let [step_x, step_y, step_z] = direction.offset();
            let x = if step_x == 0 {
                random.f64()
            } else {
                0.5_f64 + f64::from(step_x) * 0.6_f64
            };
            let y = if step_y == 0 {
                random.f64()
            } else {
                0.5_f64 + f64::from(step_y) * 0.6_f64
            };
            let z = if step_z == 0 {
                random.f64()
            } else {
                0.5_f64 + f64::from(step_z) * 0.6_f64
            };
            level.add_particle(
                &self.dripping_obsidian_tear,
                DVec3::new(
                    f64::from(pos.x) + x,
                    f64::from(pos.y) + y,
                    f64::from(pos.z) + z,
                ),
                DVec3::ZERO,
            );
        }
    }
}
