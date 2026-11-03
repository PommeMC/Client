//! Scoped live-world queries for particles.

use azalea_block::BlockState;
use glam::DVec3;

use super::render::ParticleLight;
use super::world::ParticleWorld;
use crate::physics::aabb::Aabb;
use crate::physics::block_shape::CollisionContext;
use crate::physics::collision::{CollisionWorld, resolve_collision};
use crate::position::{BlockPos, ChunkPos};
use crate::renderer::chunk::block_lighter::{WorldLight, light_coords};
use crate::world::level::LevelContext;

impl ParticleWorld for LevelContext<'_> {
    fn get_block_state(&self, position: BlockPos) -> BlockState {
        LevelContext::get_block_state(self, position)
    }

    fn collide(&self, bounds: &Aabb, displacement: DVec3, position_y: f64) -> DVec3 {
        // COMPAT: without block-entity animation data, CollisionWorld keeps
        // shulker lids closed and moving-piston shapes empty.
        resolve_collision(
            CollisionWorld::new(self.chunks(), self.block_state_table()),
            *bounds,
            displacement.into(),
            0.0,
            false,
            &CollisionContext::position(position_y),
        )
        .0
    }

    fn get_light_coords(&self, position: DVec3) -> ParticleLight {
        let BlockPos { x, y, z } = BlockPos::containing(position);
        let chunks = self.chunks();
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        if chunks.get_chunk(&chunk_pos).is_none() {
            return ParticleLight { block: 0, sky: 240 };
        }

        let levels = light_coords(
            &WorldLight {
                chunks,
                table: self.block_state_table(),
            },
            [x, y, z],
        );
        ParticleLight {
            block: levels.block() * 16,
            sky: levels.sky() * 16,
        }
    }
}
