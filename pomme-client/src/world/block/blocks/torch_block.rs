use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct TorchBlock {
    block_data: BlockData,
    smoke: SimpleParticleType,
    flame: SimpleParticleType,
}

impl TorchBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        smoke: SimpleParticleType,
        flame: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            smoke,
            flame,
        }
    }
}

impl Block for TorchBlock {
    fn block_data(&self) -> &BlockData {
        &self.block_data
    }

    fn animate_tick(
        &self,
        _state: BlockState,
        pos: BlockPos,
        level: &mut LevelContext<'_>,
        _random: &mut fastrand::Rng,
    ) {
        let position = DVec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.7, pos.z as f64 + 0.5);
        emit_torch_particles(level, position, &self.smoke, &self.flame);
    }
}

pub(in crate::world::block) struct WallTorchBlock {
    block_data: BlockData,
    smoke: SimpleParticleType,
    flame: SimpleParticleType,
}

impl WallTorchBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        smoke: SimpleParticleType,
        flame: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            smoke,
            flame,
        }
    }
}

impl Block for WallTorchBlock {
    fn block_data(&self) -> &BlockData {
        &self.block_data
    }

    fn animate_tick(
        &self,
        state: BlockState,
        pos: BlockPos,
        level: &mut LevelContext<'_>,
        _random: &mut fastrand::Rng,
    ) {
        let properties = level
            .block_state_table()
            .block_state_data(state)
            .properties();
        // FACING points away from support; the tip shifts toward support.
        let (offset_x, offset_z) = match properties.get("facing") {
            Some("north") => (0.0, 0.27),
            Some("south") => (0.0, -0.27),
            Some("west") => (0.27, 0.0),
            Some("east") => (-0.27, 0.0),
            _ => return,
        };
        let position = DVec3::new(
            pos.x as f64 + 0.5 + offset_x,
            pos.y as f64 + 0.7 + 0.22,
            pos.z as f64 + 0.5 + offset_z,
        );
        emit_torch_particles(level, position, &self.smoke, &self.flame);
    }
}

fn emit_torch_particles(
    level: &mut LevelContext<'_>,
    position: DVec3,
    smoke: &SimpleParticleType,
    flame: &SimpleParticleType,
) {
    level.add_particle(smoke, position, DVec3::ZERO);
    level.add_particle(flame, position, DVec3::ZERO);
}
