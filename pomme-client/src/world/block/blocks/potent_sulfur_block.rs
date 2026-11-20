use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData, FluidKind};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct PotentSulfurBlock {
    block_data: BlockData,
    sulfur_bubbles: SimpleParticleType,
}

impl PotentSulfurBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        sulfur_bubbles: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            sulfur_bubbles,
        }
    }
}

impl Block for PotentSulfurBlock {
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
        if !matches!(
            table
                .block_state_data(state)
                .properties()
                .get("potent_sulfur_state"),
            Some("wet" | "dormant" | "erupting" | "continuous")
        ) {
            return;
        }
        let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
        let fluid = table.block_state_data(level.get_block_state(above)).fluid();
        if fluid.kind != FluidKind::Water || fluid.amount != 8 || fluid.falling {
            return;
        }
        for _ in 0..2 {
            let position = DVec3::new(
                f64::from(pos.x) + f64::from(random.f32()),
                f64::from(pos.y + 1) + f64::from(random.f32()),
                f64::from(pos.z) + f64::from(random.f32()),
            );
            level.add_always_visible_particle(&self.sulfur_bubbles, position, DVec3::ZERO);
        }
        // COMPAT: Preserve the callback's sound gate draw; ambient audio is omitted.
        let _sound_gate = random.i32(0..10);
    }
}
