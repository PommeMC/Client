use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct RespawnAnchorBlock {
    block_data: BlockData,
    reverse_portal: SimpleParticleType,
}

impl RespawnAnchorBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        reverse_portal: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            reverse_portal,
        }
    }
}

impl Block for RespawnAnchorBlock {
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
        if !matches!(properties.get("charges"), Some("1" | "2" | "3" | "4")) {
            return;
        }

        // COMPAT: Preserve the ambient sound gate; its volume and pitch are constant.
        let _ = random.u32(0..100);
        let x = f64::from(pos.x) + 0.5 + (0.5 - random.f64());
        let y = f64::from(pos.y) + 1.0;
        let z = f64::from(pos.z) + 0.5 + (0.5 - random.f64());
        let ya = f64::from(random.f32()) * 0.04;
        level.add_particle(
            &self.reverse_portal,
            DVec3::new(x, y, z),
            DVec3::new(0.0, ya, 0.0),
        );
    }
}
