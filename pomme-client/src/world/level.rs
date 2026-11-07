//! Conceptual equivalent to Vanilla ClientLevel/Level.  Provides access to
//! resources other game systems need, without exposing internals of GameState
//! generally.
use std::cell::RefCell;
use std::rc::Rc;

use azalea_block::BlockState;
use azalea_protocol::common::client_information::ParticleStatus;
use glam::DVec3;

use super::block::BlockStateTable;
use super::block::block_registry::BlockRegistry;
use super::block::particle_appearance::BlockParticleAppearance;
use super::chunk::ChunkStore;
use crate::particles::particle_engine_access::ParticleEngineAccess;
use crate::particles::render::ParticleLight;
use crate::particles::types::ParticleOptions;
use crate::particles::{ParticleRef, TerrainParticle};
use crate::physics::aabb::Aabb;
use crate::physics::block_shape::{self, CollisionContext, LocalBox};
use crate::physics::collision::{CollisionWorld, resolve_collision};
use crate::position::{BlockPos, ChunkPos};
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::renderer::chunk::block_lighter::{WorldLight, light_coords};
use crate::util::random_source::RandomSource;

/// Scoped world queries and callbacks; never retains a chunk snapshot.
/// Camera, particle status, the persistent level random stream and the
/// canonical particle engine's creation services are required at construction.
pub struct LevelContext<'a> {
    chunks: &'a ChunkStore,
    block_particle_appearance: &'a BlockParticleAppearance<'a>,
    pub(crate) camera_render_position: DVec3,
    pub(crate) particle_status: ParticleStatus,
    pub(crate) random: &'a RandomSource,
    pub(crate) particle_access: &'a dyn ParticleEngineAccess,
}

impl<'a> LevelContext<'a> {
    pub(crate) fn new(
        chunks: &'a ChunkStore,
        block_particle_appearance: &'a BlockParticleAppearance<'a>,
        camera_render_position: DVec3,
        particle_status: ParticleStatus,
        random: &'a RandomSource,
        particle_access: &'a dyn ParticleEngineAccess,
    ) -> Self {
        Self {
            chunks,
            block_particle_appearance,
            camera_render_position,
            particle_status,
            random,
            particle_access,
        }
    }

    pub(crate) fn chunks(&self) -> &ChunkStore {
        self.chunks
    }

    pub(crate) fn block_state_table(&self) -> BlockStateTable {
        self.block_particle_appearance.block_state_table()
    }

    pub(crate) fn block_particle_appearance(&self) -> &'a BlockParticleAppearance<'a> {
        self.block_particle_appearance
    }

    /// Missing chunks and positions outside the dimension height read as air.
    pub fn get_block_state(&self, pos: BlockPos) -> BlockState {
        self.chunks.get_block_state(pos.x, pos.y, pos.z)
    }

    /// A grid of terrain particles across each box of the block's outline.
    pub fn add_destroy_block_effect(&mut self, pos: BlockPos, state: BlockState) {
        let block_state_data = self.block_state_table().block_state_data(state);
        if block_state_data.is_air()
            || matches!(block_state_data.id(), "barrier" | "structure_void")
        {
            return;
        }
        let appearance = self.block_particle_appearance();
        let sprite: AtlasRegion = appearance.sprite(state);
        let tint: [f32; 3] = appearance.tint(state, pos, self.chunks());
        let boxes: &[LocalBox] = block_shape::outline_shape(block_state_data);
        let shape_origin = block_state_data.outline_shape_position(pos.x, pos.y, pos.z);

        for b in boxes {
            let width_x = (b[3] - b[0]).min(1.0);
            let width_y = (b[4] - b[1]).min(1.0);
            let width_z = (b[5] - b[2]).min(1.0);
            let count_x = ((width_x / 0.25).ceil() as i32).max(2);
            let count_y = ((width_y / 0.25).ceil() as i32).max(2);
            let count_z = ((width_z / 0.25).ceil() as i32).max(2);
            for xx in 0..count_x {
                for yy in 0..count_y {
                    for zz in 0..count_z {
                        let rel_x = (xx as f64 + 0.5) / count_x as f64;
                        let rel_y = (yy as f64 + 0.5) / count_y as f64;
                        let rel_z = (zz as f64 + 0.5) / count_z as f64;
                        let spawn = DVec3::new(
                            shape_origin.x + rel_x * width_x + b[0],
                            shape_origin.y + rel_y * width_y + b[1],
                            shape_origin.z + rel_z * width_z + b[2],
                        );
                        let particle: ParticleRef = Rc::new(RefCell::new(TerrainParticle::new(
                            spawn,
                            DVec3::new(rel_x - 0.5, rel_y - 0.5, rel_z - 0.5),
                            sprite,
                            tint,
                        )));
                        self.particle_engine().add(particle);
                    }
                }
            }
        }
    }
}

#[allow(dead_code, reason = "Particle spawning awaits producer integration")]
impl<'a> LevelContext<'a> {
    /// Rust borrow-split access, not a Vanilla ClientLevel creation method:
    /// callbacks borrow shared state without borrowing active engine groups.
    /// Copy the stored reference with its backing lifetime so callers can pass
    /// this context mutably to the engine after this getter returns.
    pub(crate) fn particle_engine(&self) -> &'a dyn ParticleEngineAccess {
        self.particle_access
    }

    /// The shared level stream, not engine creation randomness.
    pub fn get_random(&self) -> &RandomSource {
        self.random
    }

    /// Vanilla ClientLevel.addParticle's policy-aware default overload.
    pub fn add_particle(
        &mut self,
        options: &dyn ParticleOptions,
        position: DVec3,
        auxiliary: DVec3,
    ) {
        self.do_add_particle(
            options,
            options.get_type().get_override_limiter(),
            false,
            position,
            auxiliary,
        );
    }

    /// Explicit Rust name for Vanilla's addParticle overload with flags.
    pub fn add_particle_with_flags(
        &mut self,
        options: &dyn ParticleOptions,
        override_limiter: bool,
        always_show: bool,
        position: DVec3,
        auxiliary: DVec3,
    ) {
        self.do_add_particle(
            options,
            options.get_type().get_override_limiter() || override_limiter,
            always_show,
            position,
            auxiliary,
        );
    }

    /// Unlike the explicit-override overload, this ignores the type's override.
    pub fn add_always_visible_particle(
        &mut self,
        options: &dyn ParticleOptions,
        position: DVec3,
        auxiliary: DVec3,
    ) {
        self.do_add_particle(options, false, true, position, auxiliary);
    }

    /// Explicit Rust name for Vanilla's addAlwaysVisibleParticle override
    /// overload.
    pub fn add_always_visible_particle_with_override(
        &mut self,
        options: &dyn ParticleOptions,
        override_limiter: bool,
        position: DVec3,
        auxiliary: DVec3,
    ) {
        self.do_add_particle(
            options,
            options.get_type().get_override_limiter() || override_limiter,
            true,
            position,
            auxiliary,
        );
    }

    fn do_add_particle(
        &mut self,
        options: &dyn ParticleOptions,
        override_limiter: bool,
        always_show: bool,
        position: DVec3,
        auxiliary: DVec3,
    ) {
        let engine = self.particle_engine();
        let mut status = self.particle_status;
        if always_show && status == ParticleStatus::Minimal && self.random.next_int(10) == 0 {
            status = ParticleStatus::Decreased;
        }
        if status == ParticleStatus::Decreased && self.random.next_int(3) == 0 {
            status = ParticleStatus::Minimal;
        }
        // Draws precede override/distance checks, even for rejected requests.
        if !override_limiter {
            if self.camera_render_position.distance_squared(position) > 1024.0 {
                return;
            }
            if status == ParticleStatus::Minimal {
                return;
            }
        }
        engine.create_particle(self, options, position, auxiliary);
    }

    /// Resolve against the session's selected block-state table and collision
    /// shapes; returns allowed displacement, not a new absolute position.
    /// `position_y` is the particle's Y for position-dependent shapes,
    /// independent of the supplied bounds: setting a bounding box need not
    /// update position.
    /// TODO: Revisit this adapter method once concrete particle movement
    /// establishes its needs.
    pub fn collide(&self, bounds: &Aabb, displacement: DVec3, position_y: f64) -> DVec3 {
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

    /// Unloaded chunks use full sky, zero block light. Effects may override it.
    /// TODO: Revisit whether concrete particles should compute light
    /// coordinates using world-query helpers instead.
    pub fn get_light_coords(&self, position: DVec3) -> ParticleLight {
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

// Item model queries remain deferred; they must use the existing model
// services.

/// Owns the persistent level random stream for animation sampling and
/// particles. Block rules, session protocol bindings and chunks remain
/// externally owned.
pub struct LevelRuntime {
    // Preserve vanilla's distribution and draw order, not its seeded sequence.
    random: RandomSource,
}

impl LevelRuntime {
    pub(crate) fn new() -> Self {
        Self {
            random: RandomSource::new(),
        }
    }

    pub(crate) fn get_random(&self) -> &RandomSource {
        &self.random
    }

    /// Vanilla `ClientLevel.animateTick`, called once per simulation tick.
    // TODO: vanilla gates this on `!pause && isLevelRunningNormally()`, as it
    // does the particle tick.
    pub(crate) fn animate_tick(
        &mut self,
        block_registry: &BlockRegistry,
        player_pos: DVec3,
        level: &mut LevelContext<'_>,
    ) {
        let random = level.get_random().clone();
        animate_tick_with_draw(
            block_registry,
            player_pos,
            |radius| random.next_int(radius as usize) as i32,
            level,
            fastrand::Rng::new(),
        );
    }
}

/// Own the effect stream for this pass; all sampled callbacks borrow it.
// TODO: the rest of `doAnimateTick`: the fluid's `animateTick` and drip
// particles, the creative marker particle and the ambient particles. The drip
// and ambient steps draw from the level random between samples. Coordinate
// draws release their random borrow before each block callback.
fn animate_tick_with_draw(
    block_registry: &BlockRegistry,
    player_pos: DVec3,
    next: impl FnMut(i32) -> i32,
    level: &mut LevelContext<'_>,
    mut random: fastrand::Rng,
) {
    sample_animation_blocks(player_pos, next, |pos| {
        block_registry.animate_tick(pos, level, &mut random);
    });
}

/// Each bounded draw is in `0..radius`; preserve vanilla's radius and XYZ
/// order.
fn sample_animation_blocks(
    player_pos: DVec3,
    mut next: impl FnMut(i32) -> i32,
    mut sample: impl FnMut(BlockPos),
) {
    let center = BlockPos::new(
        player_pos.x.floor() as i32,
        player_pos.y.floor() as i32,
        player_pos.z.floor() as i32,
    );
    for _ in 0..667 {
        for radius in [16, 32] {
            let x = center.x + next(radius) - next(radius);
            let y = center.y + next(radius) - next(radius);
            let z = center.z + next(radius) - next(radius);
            sample(BlockPos::new(x, y, z));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_follow_vanilla_draw_order() {
        let mut radii = Vec::new();
        let mut samples = Vec::new();
        sample_animation_blocks(
            DVec3::new(10.5, 64.0, -3.5),
            |radius| {
                radii.push(radius);
                radii.len() as i32 % radius
            },
            |pos| samples.push(pos),
        );
        assert_eq!(samples.len(), 667 * 2);
        // Each sample draws x, y, then z, two draws apiece, at radius 16 then 32.
        assert_eq!(radii.len(), samples.len() * 6);
        for (i, sample) in radii.chunks(6).enumerate() {
            let radius = if i % 2 == 0 { 16 } else { 32 };
            assert!(
                sample.iter().all(|&r| r == radius),
                "sample {i}: {sample:?}"
            );
        }
        assert_eq!(
            samples[0],
            BlockPos::new(10 + 1 - 2, 64 + 3 - 4, -4 + 5 - 6)
        );
    }
}
