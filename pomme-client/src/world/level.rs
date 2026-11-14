//! Conceptual equivalent to Vanilla ClientLevel/Level.  Provides access to
//! resources other game systems need, without exposing internals of GameState
//! generally.
use std::cell::RefCell;
use std::rc::Rc;

use azalea_block::BlockState;
use azalea_protocol::common::client_information::ParticleStatus;
use glam::DVec3;

use super::block::block_registry::BlockRegistry;
use super::block::particle_appearance::BlockParticleAppearance;
use super::block::{BlockStateTable, BlockTags, FluidKind, model};
use super::chunk::ChunkStore;
use crate::particles::particle_engine_access::ParticleEngineAccess;
use crate::particles::render::ParticleLight;
use crate::particles::types::{ParticleOptions, SimpleParticleType};
use crate::particles::{ParticleRef, TerrainParticle};
use crate::physics::aabb::Aabb;
use crate::physics::block_shape::{self, CollisionContext, LocalBox};
use crate::physics::collision::{CollisionWorld, resolve_collision, state_boxes};
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
    pub(crate) block_tags: &'a BlockTags,
    pub(crate) camera_render_position: DVec3,
    pub(crate) particle_status: ParticleStatus,
    pub(crate) random: &'a RandomSource,
    pub(crate) particle_access: &'a dyn ParticleEngineAccess,
}

impl<'a> LevelContext<'a> {
    pub(crate) fn new(
        chunks: &'a ChunkStore,
        block_particle_appearance: &'a BlockParticleAppearance<'a>,
        block_tags: &'a BlockTags,
        camera_render_position: DVec3,
        particle_status: ParticleStatus,
        random: &'a RandomSource,
        particle_access: &'a dyn ParticleEngineAccess,
    ) -> Self {
        Self {
            chunks,
            block_particle_appearance,
            block_tags,
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

    pub(crate) fn is_block_tag(&self, state: BlockState, tag: &str) -> bool {
        self.block_tags
            .contains(tag, self.block_state_table().block_state_data(state).id())
    }

    pub(crate) fn block_particle_appearance(&self) -> &'a BlockParticleAppearance<'a> {
        self.block_particle_appearance
    }

    /// Missing chunks and positions outside the dimension height read as air.
    pub fn get_block_state(&self, pos: BlockPos) -> BlockState {
        self.chunks.get_block_state(pos.x, pos.y, pos.z)
    }

    pub(crate) fn get_fluid_height(&self, pos: BlockPos) -> f64 {
        let table = self.block_state_table();
        let fluid = table.block_state_data(self.get_block_state(pos)).fluid();
        if fluid.kind == FluidKind::Empty {
            return 0.0;
        }
        let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
        if table
            .block_state_data(self.get_block_state(above))
            .fluid()
            .kind
            == fluid.kind
        {
            1.0
        } else {
            f64::from(f32::from(fluid.amount) / 9.0_f32)
        }
    }

    pub(crate) fn get_collision_shape(&self, pos: BlockPos) -> Vec<Aabb> {
        let mut boxes = Vec::new();
        state_boxes(
            self.block_state_table(),
            self.get_block_state(pos),
            pos,
            &CollisionContext::EMPTY,
            DVec3::ZERO,
            &mut boxes,
        );
        boxes
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

    /// One terrain particle at the struck face of the whole outline's bounds.
    /// COMPAT: cached static outlines do not track animated shulker lids.
    pub fn add_breaking_block_effect(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        direction: model::Direction,
    ) {
        use crate::particles::Particle;

        let block_state_data = self.block_state_table().block_state_data(state);
        if block_state_data.is_air()
            || block_state_data.is_invisible()
            || matches!(block_state_data.id(), "barrier" | "structure_void")
        {
            return;
        }
        let boxes = block_shape::outline_shape(block_state_data);
        let Some((first, remaining)) = boxes.split_first() else {
            // COMPAT: skip empty outlines instead of Source bounds()'s
            // UnsupportedOperationException; do not substitute a full cube.
            return;
        };
        let mut min = DVec3::new(first[0], first[1], first[2]);
        let mut max = DVec3::new(first[3], first[4], first[5]);
        for b in remaining {
            min = min.min(DVec3::new(b[0], b[1], b[2]));
            max = max.max(DVec3::new(b[3], b[4], b[5]));
        }
        let origin = DVec3::new(f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
        // Cached outlines include ZERO's offset; compensate in local bounds.
        let offset = block_state_data.outline_shape_position(pos.x, pos.y, pos.z) - origin;
        min += offset;
        max += offset;

        let random = self.get_random().clone();
        let r = f64::from(0.1_f32);
        let inset = f64::from(0.2_f32);
        // All three LEVEL draws precede the face override and sprite lookup.
        let mut spawn = DVec3::new(
            origin.x + random.next_double() * (max.x - min.x - inset) + r + min.x,
            origin.y + random.next_double() * (max.y - min.y - inset) + r + min.y,
            origin.z + random.next_double() * (max.z - min.z - inset) + r + min.z,
        );
        match direction {
            model::Direction::Down => spawn.y = origin.y + min.y - r,
            model::Direction::Up => spawn.y = origin.y + max.y + r,
            model::Direction::North => spawn.z = origin.z + min.z - r,
            model::Direction::South => spawn.z = origin.z + max.z + r,
            model::Direction::West => spawn.x = origin.x + min.x - r,
            model::Direction::East => spawn.x = origin.x + max.x + r,
        }

        let appearance = self.block_particle_appearance();
        let sprite = appearance.sprite(state);
        let tint = appearance.tint(state, pos, self.chunks());
        let mut particle = TerrainParticle::new(spawn, DVec3::ZERO, sprite, tint);
        let velocity = particle.get_velocity();
        let power = f64::from(0.2_f32);
        particle.set_velocity(DVec3::new(
            velocity.x * power,
            (velocity.y - r) * power + r,
            velocity.z * power,
        ));
        particle.scale(0.6_f32);
        let particle: ParticleRef = Rc::new(RefCell::new(particle));
        self.particle_engine().add(particle);
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
        lava: &SimpleParticleType,
        underwater: &SimpleParticleType,
        dripping_water: &SimpleParticleType,
        dripping_lava: &SimpleParticleType,
    ) {
        let random = level.get_random().clone();
        animate_tick_with_draw(
            block_registry,
            player_pos,
            |radius| random.next_int(radius as usize) as i32,
            level,
            fastrand::Rng::new(),
            lava,
            underwater,
            dripping_water,
            dripping_lava,
        );
    }

    fn try_spawn_drip_particles(
        block_registry: &BlockRegistry,
        level: &mut LevelContext<'_>,
        pos: BlockPos,
        dripping_water: &SimpleParticleType,
        has_watertight_bottom: bool,
    ) {
        let table = level.block_state_table();
        let state = table.block_state_data(level.get_block_state(pos));
        if state.fluid().kind != FluidKind::Empty {
            return;
        }
        let (min, max) = Self::shape_bounds(level, pos);
        let origin = DVec3::new(f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
        if max.y < 1.0 {
            if has_watertight_bottom {
                Self::spawn_fluid_particle(
                    level,
                    dripping_water,
                    origin,
                    origin + DVec3::ONE,
                    origin.y + 1.0 - 0.05,
                );
            }
        } else if !block_registry
            .tags()
            .contains("minecraft:impermeable", state.id())
        {
            if min.y > 0.0 {
                Self::spawn_fluid_particle(
                    level,
                    dripping_water,
                    origin + min,
                    origin + max,
                    origin.y + min.y - 0.05,
                );
            } else {
                let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
                let below_state = table.block_state_data(level.get_block_state(below));
                if Self::shape_bounds(level, below).1.y < 1.0
                    && below_state.fluid().kind == FluidKind::Empty
                {
                    Self::spawn_fluid_particle(
                        level,
                        dripping_water,
                        origin + min,
                        origin + max,
                        origin.y - 0.05,
                    );
                }
            }
        }
    }

    fn shape_bounds(level: &LevelContext<'_>, pos: BlockPos) -> (DVec3, DVec3) {
        let origin = DVec3::new(f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
        level.get_collision_shape(pos).into_iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(min, max), bounds| (min.min(bounds.min - origin), max.max(bounds.max - origin)),
        )
    }

    fn spawn_fluid_particle(
        level: &mut LevelContext<'_>,
        dripping_water: &SimpleParticleType,
        min: DVec3,
        max: DVec3,
        y: f64,
    ) {
        let x = min.x + level.get_random().next_double() * (max.x - min.x);
        let z = min.z + level.get_random().next_double() * (max.z - min.z);
        level.add_particle(dripping_water, DVec3::new(x, y, z), DVec3::ZERO);
    }
}

/// Own the effect stream for this pass; all sampled callbacks borrow it.
// TODO: creative marker and environmental ambient particles remain absent.
// The drip and ambient steps draw from the level random between samples.
// Coordinate draws release their random borrow before each block/fluid
// callback.
fn animate_tick_with_draw(
    block_registry: &BlockRegistry,
    player_pos: DVec3,
    next: impl FnMut(i32) -> i32,
    level: &mut LevelContext<'_>,
    mut random: fastrand::Rng,
    lava: &SimpleParticleType,
    underwater: &SimpleParticleType,
    dripping_water: &SimpleParticleType,
    dripping_lava: &SimpleParticleType,
) {
    sample_animation_blocks(player_pos, next, |pos| {
        let table = level.block_state_table();
        let state = table.block_state_data(level.get_block_state(pos));
        block_registry.animate_tick(pos, level, &mut random);
        let fluid = table.block_state_data(level.get_block_state(pos)).fluid();
        super::fluid::animate_tick(pos, fluid, level, &mut random, lava, underwater);
        if let Some(drip) = super::fluid::get_drip_particle(fluid, dripping_water, dripping_lava)
            && level.get_random().next_int(10) == 0
        {
            // COMPAT: absent sturdy metadata is conservatively not watertight.
            let has_watertight_bottom = state
                .is_full_face_sturdy(model::Direction::Down)
                .unwrap_or(false);
            LevelRuntime::try_spawn_drip_particles(
                block_registry,
                level,
                BlockPos::new(pos.x, pos.y - 1, pos.z),
                drip,
                has_watertight_bottom,
            );
        }
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
