use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct BubbleColumnBlock {
    block_data: BlockData,
    bubble_column_up: SimpleParticleType,
    current_down: SimpleParticleType,
}

impl BubbleColumnBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        bubble_column_up: SimpleParticleType,
        current_down: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            bubble_column_up,
            current_down,
        }
    }
}

impl Block for BubbleColumnBlock {
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
        let x = f64::from(pos.x);
        let y = f64::from(pos.y);
        let z = f64::from(pos.z);
        match level
            .block_state_table()
            .block_state_data(state)
            .properties()
            .get("drag")
        {
            Some("true") => {
                level.add_always_visible_particle(
                    &self.current_down,
                    DVec3::new(x + 0.5, y + 0.8, z),
                    DVec3::ZERO,
                );
            }
            Some("false") => {
                let auxiliary = DVec3::new(0.0, 0.04, 0.0);
                level.add_always_visible_particle(
                    &self.bubble_column_up,
                    DVec3::new(x + 0.5, y, z + 0.5),
                    auxiliary,
                );
                let position = DVec3::new(
                    x + f64::from(random.f32()),
                    y + f64::from(random.f32()),
                    z + f64::from(random.f32()),
                );
                level.add_always_visible_particle(&self.bubble_column_up, position, auxiliary);
            }
            _ => return,
        }
        // COMPAT: Retain callback sound draws without implementing ambient audio.
        if random.usize(..200) == 0 {
            let _volume = 0.2_f32 + random.f32() * 0.2_f32;
            let _pitch = 0.9_f32 + random.f32() * 0.15_f32;
        }
    }
}
