use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct DriedGhastBlock {
    block_data: BlockData,
    white_smoke: SimpleParticleType,
    happy_villager: SimpleParticleType,
}

impl DriedGhastBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        white_smoke: SimpleParticleType,
        happy_villager: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            white_smoke,
            happy_villager,
        }
    }
}

impl Block for DriedGhastBlock {
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
        let waterlogged = match state_data.properties().get("waterlogged") {
            Some("false") => false,
            Some("true") => true,
            _ => return,
        };
        let center = DVec3::new(
            f64::from(pos.x) + 0.5_f64,
            f64::from(pos.y) + 0.5_f64,
            f64::from(pos.z) + 0.5_f64,
        );
        // Preserve Vanilla's audio gate draw without playing sound or querying its tag.
        let _ = random.usize(..40);
        if random.usize(..6) != 0 {
            return;
        }
        if waterlogged {
            let position = DVec3::new(
                center.x + f64::from((random.f32() * 2.0_f32 - 1.0_f32) / 3.0_f32),
                center.y + 0.4_f64,
                center.z + f64::from((random.f32() * 2.0_f32 - 1.0_f32) / 3.0_f32),
            );
            let auxiliary = DVec3::new(0.0, f64::from(random.f32()), 0.0);
            level.add_particle(&self.happy_villager, position, auxiliary);
        } else {
            level.add_particle(&self.white_smoke, center, DVec3::new(0.0, 0.02_f64, 0.0));
        }
    }
}
