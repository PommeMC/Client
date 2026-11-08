use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::dust_particle_options::DustParticleOptions;
use crate::position::BlockPos;
use crate::world::block::model::Direction;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct LeverBlock {
    block_data: BlockData,
    dust: DustParticleOptions,
}

impl LeverBlock {
    pub(in crate::world::block) fn new(block_data: BlockData, dust: DustParticleOptions) -> Self {
        Self { block_data, dust }
    }
}

impl Block for LeverBlock {
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
        let properties = level
            .block_state_table()
            .block_state_data(state)
            .properties();
        if properties.get("powered") != Some("true") {
            return;
        }
        let Some(facing) = properties
            .get("facing")
            .and_then(Direction::from_str)
            .filter(|direction| direction.axis() != 1)
        else {
            return;
        };
        let connected = match properties.get("face") {
            Some("ceiling") => Direction::Down,
            Some("floor") => Direction::Up,
            Some("wall") => facing,
            _ => return,
        };
        if random.f32() >= 0.25_f32 {
            return;
        }

        let opposite_facing = facing.opposite().offset();
        let opposite_connected = connected.opposite().offset();
        let position = DVec3::new(
            ((pos.x as f64 + 0.5_f64) + 0.1_f64 * opposite_facing[0] as f64)
                + 0.2_f64 * opposite_connected[0] as f64,
            ((pos.y as f64 + 0.5_f64) + 0.1_f64 * opposite_facing[1] as f64)
                + 0.2_f64 * opposite_connected[1] as f64,
            ((pos.z as f64 + 0.5_f64) + 0.1_f64 * opposite_facing[2] as f64)
                + 0.2_f64 * opposite_connected[2] as f64,
        );
        level.add_particle(&self.dust, position, DVec3::ZERO);
    }
}
