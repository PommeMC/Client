use azalea_block::BlockState;
use glam::DVec3;

use super::super::model::Direction;
use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct NetherPortalBlock {
    block_data: BlockData,
    portal: SimpleParticleType,
}

impl NetherPortalBlock {
    pub(in crate::world::block) fn new(block_data: BlockData, portal: SimpleParticleType) -> Self {
        Self { block_data, portal }
    }

    fn is_portal(level: &LevelContext<'_>, pos: BlockPos) -> bool {
        let state = level.get_block_state(pos);
        matches!(
            level.block_state_table().block_state_data(state).id(),
            "nether_portal" | "minecraft:nether_portal"
        )
    }
}

impl Block for NetherPortalBlock {
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
        if random.u32(0..100) == 0 {
            // Audio is unchanged; preserve only its conditional pitch draw.
            let _ = random.f32() * 0.4_f32 + 0.8_f32;
        }
        for _ in 0..4 {
            let mut x = f64::from(pos.x) + random.f64();
            let y = f64::from(pos.y) + random.f64();
            let mut z = f64::from(pos.z) + random.f64();
            let mut xa = (f64::from(random.f32()) - 0.5) * 0.5;
            let ya = (f64::from(random.f32()) - 0.5) * 0.5;
            let mut za = (f64::from(random.f32()) - 0.5) * 0.5;
            let flip = random.i32(0..2) * 2 - 1;
            if !Self::is_portal(level, pos.relative(Direction::West))
                && !Self::is_portal(level, pos.relative(Direction::East))
            {
                x = f64::from(pos.x) + 0.5 + 0.25 * f64::from(flip);
                xa = f64::from(random.f32() * 2.0_f32 * flip as f32);
            } else {
                z = f64::from(pos.z) + 0.5 + 0.25 * f64::from(flip);
                za = f64::from(random.f32() * 2.0_f32 * flip as f32);
            }
            level.add_particle(&self.portal, DVec3::new(x, y, z), DVec3::new(xa, ya, za));
        }
    }
}

pub(in crate::world::block) struct EndPortalBlock {
    block_data: BlockData,
    smoke: SimpleParticleType,
}

impl EndPortalBlock {
    pub(in crate::world::block) fn new(block_data: BlockData, smoke: SimpleParticleType) -> Self {
        Self { block_data, smoke }
    }
}

impl Block for EndPortalBlock {
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
        let x = f64::from(pos.x) + random.f64();
        let y = f64::from(pos.y) + 0.8;
        let z = f64::from(pos.z) + random.f64();
        level.add_particle(&self.smoke, DVec3::new(x, y, z), DVec3::ZERO);
    }
}
