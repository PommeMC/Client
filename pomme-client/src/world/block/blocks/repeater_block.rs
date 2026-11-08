use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::dust_particle_options::DustParticleOptions;
use crate::position::BlockPos;
use crate::world::block::model::Direction;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct RepeaterBlock {
    block_data: BlockData,
    dust: DustParticleOptions,
}

impl RepeaterBlock {
    pub(in crate::world::block) fn new(block_data: BlockData, dust: DustParticleOptions) -> Self {
        Self { block_data, dust }
    }
}

impl Block for RepeaterBlock {
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
        let delay = match properties.get("delay") {
            Some("1") => 1,
            Some("2") => 2,
            Some("3") => 3,
            Some("4") => 4,
            _ => return,
        };

        let mut position = DVec3::new(
            pos.x as f64 + 0.5_f64 + (random.f64() - 0.5_f64) * 0.2_f64,
            pos.y as f64 + 0.4_f64 + (random.f64() - 0.5_f64) * 0.2_f64,
            pos.z as f64 + 0.5_f64 + (random.f64() - 0.5_f64) * 0.2_f64,
        );
        let offset = if random.bool() {
            (2 * delay - 1) as f32
        } else {
            -5.0_f32
        } / 16.0_f32;
        let [step_x, _, step_z] = facing.offset();
        position.x += (offset * step_x as f32) as f64;
        position.z += (offset * step_z as f32) as f64;
        level.add_particle(&self.dust, position, DVec3::ZERO);
    }
}
