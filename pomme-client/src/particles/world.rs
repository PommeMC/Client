//! Proposed narrow Rust world-query adapter; Vanilla has no ParticleWorld
//! class. No persistent level borrow, camera, renderer, or GameState ownership.

use azalea_block::BlockState;
use glam::DVec3;

use super::render::ParticleLight;
use crate::physics::aabb::Aabb;
use crate::position::BlockPos;

pub trait ParticleWorld {
    /// Missing chunks/out-of-height positions read as air, like LevelContext.
    fn get_block_state(&self, position: BlockPos) -> BlockState;
    /// Resolve against the session's selected block-state table and collision
    /// shapes; returns allowed displacement, not a new absolute position.
    fn collide(&self, bounds: &Aabb, displacement: DVec3) -> DVec3;
    /// Unloaded chunks use full sky, zero block light. Effects may override it.
    fn get_light_coords(&self, position: DVec3) -> ParticleLight;
}

// Terrain/item model, tint and material queries remain deferred contracts;
// they must use the existing model services, not particle texture descriptions.
