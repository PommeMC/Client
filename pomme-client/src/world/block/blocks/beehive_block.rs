use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData, FluidKind};
use crate::particles::types::SimpleParticleType;
use crate::physics::block_shape::is_collision_shape_full_block;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct BeehiveBlock {
    block_data: BlockData,
    dripping_honey: SimpleParticleType,
}

impl BeehiveBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        dripping_honey: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            dripping_honey,
        }
    }

    fn try_spawn_drip_particles(
        &self,
        level: &mut LevelContext<'_>,
        pos: BlockPos,
        state: BlockState,
    ) {
        let table = level.block_state_table();
        if table.block_state_data(state).fluid().kind != FluidKind::Empty {
            return;
        }
        if level.get_random().next_float() < 0.3_f32 {
            return;
        }
        let origin = DVec3::new(f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
        let (min, max) = level.get_collision_shape(pos).iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(min, max), bounds| (min.min(bounds.min - origin), max.max(bounds.max - origin)),
        );
        if max.y >= 1.0 && !level.is_block_tag(state, "minecraft:impermeable") {
            if min.y > 0.0 {
                self.spawn_particle(level, origin + min, origin + max, origin.y + min.y - 0.05);
            } else {
                let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
                let below_state = table.block_state_data(level.get_block_state(below));
                let below_max_y = level
                    .get_collision_shape(below)
                    .iter()
                    .map(|bounds| bounds.max.y - f64::from(below.y))
                    .fold(f64::NEG_INFINITY, f64::max);
                if (below_max_y < 1.0 || !is_collision_shape_full_block(below_state))
                    && below_state.fluid().kind == FluidKind::Empty
                {
                    self.spawn_particle(level, origin + min, origin + max, origin.y - 0.05);
                }
            }
        }
    }

    fn spawn_particle(&self, level: &mut LevelContext<'_>, min: DVec3, max: DVec3, y: f64) {
        let x = min.x + level.get_random().next_double() * (max.x - min.x);
        let z = min.z + level.get_random().next_double() * (max.z - min.z);
        level.add_particle(&self.dripping_honey, DVec3::new(x, y, z), DVec3::ZERO);
    }
}

impl Block for BeehiveBlock {
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
        let honey_level = level
            .block_state_table()
            .block_state_data(state)
            .properties()
            .get("honey_level")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        if honey_level >= 5 {
            let mut i = 0;
            while i < random.usize(..1) + 1 {
                self.try_spawn_drip_particles(level, pos, state);
                i += 1;
            }
        }
    }
}
