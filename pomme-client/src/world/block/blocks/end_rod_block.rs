use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct EndRodBlock {
    block_data: BlockData,
    end_rod: SimpleParticleType,
}

impl EndRodBlock {
    pub(in crate::world::block) fn new(block_data: BlockData, end_rod: SimpleParticleType) -> Self {
        Self {
            block_data,
            end_rod,
        }
    }
}

impl Block for EndRodBlock {
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
        let (step_x, step_y, step_z) = match properties.get("facing") {
            Some("north") => (0.0, 0.0, -1.0),
            Some("east") => (1.0, 0.0, 0.0),
            Some("south") => (0.0, 0.0, 1.0),
            Some("west") => (-1.0, 0.0, 0.0),
            Some("up") => (0.0, 1.0, 0.0),
            Some("down") => (0.0, -1.0, 0.0),
            _ => return,
        };

        // Vanilla keeps jitter and offset arithmetic in f32 before widening,
        // and consumes all five floats even when the emission gate fails.
        let x = pos.x as f64 + 0.55 - f64::from(random.f32() * 0.1_f32);
        let y = pos.y as f64 + 0.55 - f64::from(random.f32() * 0.1_f32);
        let z = pos.z as f64 + 0.55 - f64::from(random.f32() * 0.1_f32);
        let offset = f64::from(0.4_f32 - (random.f32() + random.f32()) * 0.4_f32);
        if random.usize(..5) != 0 {
            return;
        }

        let position = DVec3::new(
            x + step_x * offset,
            y + step_y * offset,
            z + step_z * offset,
        );
        let auxiliary = DVec3::new(
            next_gaussian(random) * 0.005,
            next_gaussian(random) * 0.005,
            next_gaussian(random) * 0.005,
        );
        level.add_particle(&self.end_rod, position, auxiliary);
    }
}

fn next_gaussian(random: &mut fastrand::Rng) -> f64 {
    loop {
        let v1 = 2.0 * random.f64() - 1.0;
        let v2 = 2.0 * random.f64() - 1.0;
        let s = v1 * v1 + v2 * v2;
        if s > 0.0 && s < 1.0 {
            return v1 * (-2.0 * s.ln() / s).sqrt();
        }
    }
}
