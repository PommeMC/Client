use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct FurnaceBlock {
    block_data: BlockData,
    smoke: SimpleParticleType,
    flame: SimpleParticleType,
}

impl FurnaceBlock {
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

impl Block for FurnaceBlock {
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
        if properties.get("lit") != Some("true") {
            return;
        }

        // COMPAT: Ambient audio is not implemented; retain Vanilla's sound-chance RNG
        // draw.
        let _ = random.f64();
        let (step_x, step_z) = match properties.get("facing") {
            Some("north") => (0.0, -1.0),
            Some("south") => (0.0, 1.0),
            Some("west") => (-1.0, 0.0),
            Some("east") => (1.0, 0.0),
            _ => return,
        };
        let lateral = random.f64() * 0.6 - 0.3;
        let vertical = random.f64() * 6.0 / 16.0;
        let position = front_particle_position(pos, step_x, step_z, lateral, vertical);
        level.add_particle(&self.smoke, position, DVec3::ZERO);
        level.add_particle(&self.flame, position, DVec3::ZERO);
    }
}

pub(in crate::world::block) struct BlastFurnaceBlock {
    block_data: BlockData,
    smoke: SimpleParticleType,
}

impl BlastFurnaceBlock {
    pub(in crate::world::block) fn new(block_data: BlockData, smoke: SimpleParticleType) -> Self {
        Self { block_data, smoke }
    }
}

impl Block for BlastFurnaceBlock {
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
        if properties.get("lit") != Some("true") {
            return;
        }

        // COMPAT: Ambient audio is not implemented; retain Vanilla's sound-chance RNG
        // draw.
        let _ = random.f64();
        let (step_x, step_z) = match properties.get("facing") {
            Some("north") => (0.0, -1.0),
            Some("south") => (0.0, 1.0),
            Some("west") => (-1.0, 0.0),
            Some("east") => (1.0, 0.0),
            _ => return,
        };
        let lateral = random.f64() * 0.6 - 0.3;
        let vertical = random.f64() * 9.0 / 16.0;
        let position = front_particle_position(pos, step_x, step_z, lateral, vertical);
        level.add_particle(&self.smoke, position, DVec3::ZERO);
    }
}

fn front_particle_position(
    pos: BlockPos,
    step_x: f64,
    step_z: f64,
    lateral: f64,
    vertical: f64,
) -> DVec3 {
    let dx = if step_x != 0.0 {
        step_x * 0.52
    } else {
        lateral
    };
    let dz = if step_z != 0.0 {
        step_z * 0.52
    } else {
        lateral
    };
    DVec3::new(
        pos.x as f64 + 0.5 + dx,
        pos.y as f64 + vertical,
        pos.z as f64 + 0.5 + dz,
    )
}

pub(in crate::world::block) struct SmokerBlock {
    block_data: BlockData,
    smoke: SimpleParticleType,
}

impl SmokerBlock {
    pub(in crate::world::block) fn new(block_data: BlockData, smoke: SimpleParticleType) -> Self {
        Self { block_data, smoke }
    }
}

impl Block for SmokerBlock {
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
        if properties.get("lit") != Some("true") {
            return;
        }

        // COMPAT: Ambient audio is not implemented; retain Vanilla's sound-chance RNG
        // draw.
        let _ = random.f64();
        let position = DVec3::new(pos.x as f64 + 0.5, pos.y as f64 + 1.1, pos.z as f64 + 0.5);
        level.add_particle(&self.smoke, position, DVec3::ZERO);
    }
}
