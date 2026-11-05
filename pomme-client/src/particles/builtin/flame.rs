//! Concrete Vanilla Flame behavior, independent of engine/runtime adoption.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{
    Particle, ParticleRef, bounds_at, move_without_collision, randomized_motion, resized_bounds,
};
use super::super::particle_provider::ParticleProvider;
use super::super::render::{
    ParticleFacing, ParticleLayer, ParticleLight, ParticleRenderType, ParticleView,
    QuadParticleRenderState,
};
use super::super::resources::particle_resources::SpriteParticleRegistration;
use super::super::resources::sprite_set::SpriteSet;
use super::super::single_quad_particle::{
    SingleQuadParticle, extract_camera_quad, randomized_quad_size,
};
use super::super::types::{ParticleLimit, SimpleParticleType};
use crate::physics::aabb::Aabb;
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

pub struct FlameParticle {
    position: DVec3,
    previous_position: DVec3,
    velocity: DVec3,
    bounds: Aabb,
    width: f32,
    height: f32,
    removed: bool,
    has_physics: bool,
    age: u32,
    lifetime: u32,
    quad_size: f32,
    color: [f32; 3],
    alpha: f32,
    roll: f32,
    previous_roll: f32,
    sprite: AtlasRegion,
}

impl FlameParticle {
    fn new(position: DVec3, auxiliary: DVec3, sprite: AtlasRegion) -> Self {
        // Sprite selection already used the engine RNG. This independent stream
        // preserves Vanilla's instance draw purposes, not Java seed sequences.
        let mut random = fastrand::Rng::new();
        let bounds = bounds_at(position, 0.2, 0.2);
        let previous_position = position;
        // The base lifetime is overwritten, but its draw still precedes motion.
        let _base_lifetime = (4.0 / (random.f32() * 0.9 + 0.1)) as u32;
        let mut velocity = randomized_motion(&mut random, auxiliary);
        let quad_size = randomized_quad_size(&mut random);
        velocity = velocity * f64::from(0.01_f32) + auxiliary;
        // Rising jitters only current coordinates, leaving bounds and history
        // at the requested spawn until movement synchronizes from the bounds.
        let position = position
            + DVec3::new(
                f64::from((random.f32() - random.f32()) * 0.05),
                f64::from((random.f32() - random.f32()) * 0.05),
                f64::from((random.f32() - random.f32()) * 0.05),
            );
        // Accepted f32 adaptation: Java's final lifetime denominator uses doubles.
        let lifetime = (8.0_f32 / (random.f32() * 0.8_f32 + 0.2_f32)) as u32 + 4;
        Self {
            position,
            previous_position,
            velocity,
            bounds,
            width: 0.2,
            height: 0.2,
            removed: false,
            has_physics: true,
            age: 0,
            lifetime,
            quad_size,
            color: [1.0; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
        }
    }
}

impl Particle for FlameParticle {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_single_quad(&self) -> Option<&dyn SingleQuadParticle> {
        Some(self)
    }

    fn tick(&mut self, context: &mut LevelContext<'_>) {
        self.previous_position = self.position;
        let age = self.age;
        self.age += 1;
        if age >= self.lifetime {
            self.remove();
        } else {
            // Gravity is zero; Flame's move never establishes on-ground state.
            self.move_by(context, self.velocity);
            self.velocity *= f64::from(0.96_f32);
        }
    }

    fn move_by(&mut self, _world: &LevelContext<'_>, displacement: DVec3) {
        move_without_collision(&mut self.bounds, &mut self.position, displacement);
    }

    fn remove(&mut self) {
        self.removed = true;
    }

    fn is_alive(&self) -> bool {
        !self.removed
    }

    fn get_group(&self) -> ParticleRenderType {
        ParticleRenderType::SingleQuads
    }

    fn get_particle_limit(&self) -> Option<ParticleLimit> {
        None
    }

    fn get_pos(&self) -> DVec3 {
        self.position
    }

    fn set_pos(&mut self, position: DVec3) {
        self.position = position;
        self.bounds = bounds_at(position, self.width, self.height);
    }

    fn get_previous_pos(&self) -> DVec3 {
        self.previous_position
    }

    fn get_velocity(&self) -> DVec3 {
        self.velocity
    }

    fn set_velocity(&mut self, velocity: DVec3) {
        self.velocity = velocity;
    }

    fn get_age(&self) -> u32 {
        self.age
    }

    fn get_lifetime(&self) -> u32 {
        self.lifetime
    }

    fn set_lifetime(&mut self, lifetime: u32) {
        self.lifetime = lifetime;
    }

    fn get_bounding_box(&self) -> Aabb {
        self.bounds
    }

    fn set_bounding_box(&mut self, bounds: Aabb) {
        self.bounds = bounds;
    }

    fn set_size(&mut self, width: f32, height: f32) {
        if width != self.width || height != self.height {
            self.width = width;
            self.height = height;
            self.bounds = resized_bounds(self.bounds, width, height);
        }
    }

    fn scale(&mut self, factor: f32) {
        self.quad_size *= factor;
        self.set_size(0.2 * factor, 0.2 * factor);
    }

    fn has_physics(&self) -> bool {
        self.has_physics
    }

    fn set_has_physics(&mut self, enabled: bool) {
        self.has_physics = enabled;
    }

    fn get_light_coords(&self, world: &LevelContext<'_>, partial_tick: f32) -> ParticleLight {
        let base = world.get_light_coords(self.position);
        let s = (self.age as f32 + partial_tick) / self.lifetime as f32;
        let emitted = (s.clamp(0.0, 1.0) * 240.0) as u16;
        ParticleLight {
            block: (u16::from(base.block) + emitted).min(240) as u8,
            sky: base.sky,
        }
    }
}

impl SingleQuadParticle for FlameParticle {
    fn get_sprite(&self) -> AtlasRegion {
        self.sprite
    }

    fn set_sprite(&mut self, sprite: AtlasRegion) {
        self.sprite = sprite;
    }

    fn set_sprite_from_age(&mut self, sprites: &dyn SpriteSet) {
        if !self.removed {
            self.sprite = sprites.get(self.age, self.lifetime);
        }
    }

    fn get_uv(&self) -> [f32; 4] {
        [
            self.sprite.u_min,
            self.sprite.u_max,
            self.sprite.v_min,
            self.sprite.v_max,
        ]
    }

    fn get_color(&self) -> [f32; 3] {
        self.color
    }

    fn set_color(&mut self, color: [f32; 3]) {
        self.color = color;
    }

    fn get_alpha(&self) -> f32 {
        self.alpha
    }

    fn set_alpha(&mut self, alpha: f32) {
        self.alpha = alpha;
    }

    fn get_roll(&self) -> f32 {
        self.roll
    }

    fn set_roll(&mut self, roll: f32) {
        self.roll = roll;
    }

    fn get_facing(&self) -> ParticleFacing {
        ParticleFacing::Camera
    }

    fn get_quad_size(&self, partial_tick: f32) -> f32 {
        let s = (self.age as f32 + partial_tick) / self.lifetime as f32;
        self.quad_size * (1.0 - s * s * 0.5)
    }

    fn get_layer(&self) -> ParticleLayer {
        ParticleLayer::Opaque
    }

    fn extract(
        &self,
        world: &LevelContext<'_>,
        view: &ParticleView<'_>,
        output: &mut Vec<QuadParticleRenderState>,
    ) {
        extract_camera_quad(self, self.previous_roll, world, view, output);
    }
}

/// Rust spelling of Vanilla `FlameParticle.Provider`.
pub struct FlameProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl FlameProvider {
    pub fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<SimpleParticleType> for FlameProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(FlameParticle::new(
            position, auxiliary, sprite,
        ))))
    }
}

pub(in crate::particles) struct FlameParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for FlameParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(FlameProvider::new(sprite_set))
    }
}
