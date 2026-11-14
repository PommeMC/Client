use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::dust_color_transition_options::DustColorTransitionOptions;
use crate::position::BlockPos;
use crate::world::block::model::Direction;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct SculkSensorBlock {
    block_data: BlockData,
    options: DustColorTransitionOptions,
}

impl SculkSensorBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        options: DustColorTransitionOptions,
    ) -> Self {
        Self {
            block_data,
            options,
        }
    }
}

impl Block for SculkSensorBlock {
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
        let state_data = level.block_state_table().block_state_data(state);
        if state_data.properties().get("sculk_sensor_phase") != Some("active") {
            return;
        }
        let direction = Direction::VALUES[random.usize(..Direction::VALUES.len())];
        if matches!(direction, Direction::Up | Direction::Down) {
            return;
        }
        let [step_x, _, step_z] = direction.offset();
        let x = f64::from(pos.x)
            + 0.5_f64
            + if step_x == 0 {
                0.5_f64 - random.f64()
            } else {
                f64::from(step_x) * 0.6_f64
            };
        let y = f64::from(pos.y) + 0.25_f64;
        let z = f64::from(pos.z)
            + 0.5_f64
            + if step_z == 0 {
                0.5_f64 - random.f64()
            } else {
                f64::from(step_z) * 0.6_f64
            };
        let auxiliary = DVec3::new(0.0, f64::from(random.f32()) * 0.04_f64, 0.0);
        level.add_particle(&self.options, DVec3::new(x, y, z), auxiliary);
    }
}
