use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct EnderChestBlock {
    block_data: BlockData,
    portal: SimpleParticleType,
}

impl EnderChestBlock {
    pub(in crate::world::block) fn new(block_data: BlockData, portal: SimpleParticleType) -> Self {
        Self { block_data, portal }
    }
}

impl Block for EnderChestBlock {
    fn block_data(&self) -> &BlockData {
        &self.block_data
    }

    fn animate_tick(
        &self,
        _state: BlockState,
        pos: BlockPos,
        level: &mut LevelContext<'_>,
        random: &mut fastrand::Rng,
    ) {
        for _ in 0..3 {
            let flip_x = random.i32(0..2) * 2 - 1;
            let flip_z = random.i32(0..2) * 2 - 1;
            let x = f64::from(pos.x) + 0.5 + 0.25 * f64::from(flip_x);
            let y = f64::from(pos.y as f32 + random.f32());
            let z = f64::from(pos.z) + 0.5 + 0.25 * f64::from(flip_z);
            let xa = f64::from(random.f32() * flip_x as f32);
            let ya = (f64::from(random.f32()) - 0.5) * 0.125;
            let za = f64::from(random.f32() * flip_z as f32);
            level.add_particle(&self.portal, DVec3::new(x, y, z), DVec3::new(xa, ya, za));
        }
    }
}
