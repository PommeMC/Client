//! Narrow Rust world-query contract; Vanilla has no ParticleWorld
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
    /// `position_y` is the particle's Y for position-dependent shapes,
    /// independent of the supplied bounds: setting a bounding box need not
    /// update position.
    /// TODO: Revisit this adapter method once concrete particle movement
    /// establishes its needs.
    fn collide(&self, bounds: &Aabb, displacement: DVec3, position_y: f64) -> DVec3;
    /// Unloaded chunks use full sky, zero block light. Effects may override it.
    /// TODO: Revisit whether concrete particles should compute light
    /// coordinates using world-query helpers instead.
    fn get_light_coords(&self, position: DVec3) -> ParticleLight;
}

// Terrain/item model, tint and material queries remain deferred contracts;
// they must use the existing model services, not particle texture descriptions.
