use azalea_block::BlockState;
use glam::DVec3;

use super::super::model::Direction;
use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct WetSpongeBlock {
    block_data: BlockData,
    dripping_water: SimpleParticleType,
}

impl WetSpongeBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        dripping_water: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            dripping_water,
        }
    }
}

impl Block for WetSpongeBlock {
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
            let mut xx = f64::from(pos.x);
            let mut yy = f64::from(pos.y);
            let mut zz = f64::from(pos.z);
            if direction == Direction::Down {
                yy -= 0.05;
                xx += random.f64();
                zz += random.f64();
            } else {
                yy += random.f64() * 0.8;
                if matches!(direction, Direction::East | Direction::West) {
                    zz += random.f64();
                    if direction == Direction::East {
                        xx += 1.1;
                    } else {
                        xx += 0.05;
                    }
                } else {
                    xx += random.f64();
                    if direction == Direction::South {
                        zz += 1.1;
                    } else {
                        zz += 0.05;
                    }
                }
            }
            level.add_particle(&self.dripping_water, DVec3::new(xx, yy, zz), DVec3::ZERO);
        }
    }
}
