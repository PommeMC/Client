//! Concrete Vanilla Crit behavior and its three description-bound providers.

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

pub struct CritParticle {
    position: DVec3,
    previous_position: DVec3,
    velocity: DVec3,
    bounds: Aabb,
    width: f32,
    height: f32,
    removed: bool,
    on_ground: bool,
    stopped_by_collision: bool,
    has_physics: bool,
    gravity: f32,
    friction: f32,
    speed_up_when_y_motion_is_blocked: bool,
    age: u32,
    lifetime: u32,
    quad_size: f32,
    color: [f32; 3],
    alpha: f32,
    roll: f32,
    previous_roll: f32,
    sprite: AtlasRegion,
}

impl CritParticle {
    fn new(
        world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        sprite: AtlasRegion,
    ) -> Self {
        let mut random = fastrand::Rng::new();
        let bounds = bounds_at(position, 0.2, 0.2);
        // Retain the full-motion superclass draws, including the overwritten lifetime.
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let mut velocity = randomized_motion(&mut random, DVec3::ZERO);
        let mut quad_size = randomized_quad_size(&mut random);
        velocity *= f64::from(0.1_f32);
        velocity += auxiliary * 0.4;
        let gray = random.f32() * 0.3 + 0.6;
        quad_size *= 0.75;
        let lifetime = ((6.0 / (f64::from(random.f32()) * 0.8 + 0.6)) as u32).max(1);
        let mut particle = Self {
            position,
            previous_position: position,
            velocity,
            bounds,
            width: 0.2,
            height: 0.2,
            removed: false,
            on_ground: false,
            stopped_by_collision: false,
            has_physics: false,
            gravity: 0.5,
            friction: 0.7,
            speed_up_when_y_motion_is_blocked: false,
            age: 0,
            lifetime,
            quad_size,
            color: [gray; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
        };
        // Source returns an already moved, damped and color-faded age-one particle.
        particle.tick(world);
        particle
    }
}

impl Particle for CritParticle {
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
            self.velocity.y -= 0.04 * f64::from(self.gravity);
            self.move_by(context, self.velocity);
            if self.speed_up_when_y_motion_is_blocked && self.position.y == self.previous_position.y
            {
                self.velocity.x *= 1.1;
                self.velocity.z *= 1.1;
            }
            self.velocity *= f64::from(self.friction);
            if self.on_ground {
                self.velocity.x *= f64::from(0.7_f32);
                self.velocity.z *= f64::from(0.7_f32);
            }
        }
        self.color[1] *= 0.96;
        self.color[2] *= 0.9;
    }

    fn move_by(&mut self, world: &LevelContext<'_>, displacement: DVec3) {
        if self.stopped_by_collision {
            return;
        }
        let mut movement = displacement;
        if self.has_physics
            && displacement != DVec3::ZERO
            && displacement.length_squared() < 10000.0
        {
            movement = world.collide(&self.bounds, displacement, self.position.y);
        }
        if movement != DVec3::ZERO {
            move_without_collision(&mut self.bounds, &mut self.position, movement);
        }
        if displacement.y.abs() >= f64::from(1.0e-5_f32) && movement.y.abs() < f64::from(1.0e-5_f32)
        {
            self.stopped_by_collision = true;
        }
        self.on_ground = displacement.y != movement.y && displacement.y < 0.0;
        if displacement.x != movement.x {
            self.velocity.x = 0.0;
        }
        if displacement.z != movement.z {
            self.velocity.z = 0.0;
        }
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

    fn get_light_coords(&self, world: &LevelContext<'_>, _partial_tick: f32) -> ParticleLight {
        world.get_light_coords(self.position)
    }
}

impl SingleQuadParticle for CritParticle {
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
        self.quad_size
            * ((self.age as f32 + partial_tick) / self.lifetime as f32 * 32.0).clamp(0.0, 1.0)
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

/// Rust spelling of Vanilla `CritParticle.Provider`.
pub struct CritProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl CritProvider {
    fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<SimpleParticleType> for CritProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        let particle = CritParticle::new(world, position, auxiliary, sprite);
        Some(Rc::new(RefCell::new(particle)))
    }
}

/// Rust spelling of Vanilla `CritParticle.MagicProvider`.
pub struct MagicProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl MagicProvider {
    fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<SimpleParticleType> for MagicProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        let mut particle = CritParticle::new(world, position, auxiliary, sprite);
        particle.color[0] *= 0.3;
        particle.color[1] *= 0.8;
        Some(Rc::new(RefCell::new(particle)))
    }
}

/// Rust spelling of Vanilla `CritParticle.DamageIndicatorProvider`.
pub struct DamageIndicatorProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl DamageIndicatorProvider {
    fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<SimpleParticleType> for DamageIndicatorProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        let mut particle = CritParticle::new(
            world,
            position,
            DVec3::new(auxiliary.x, auxiliary.y + 1.0, auxiliary.z),
            sprite,
        );
        particle.set_lifetime(20);
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct CritParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for CritParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(CritProvider::new(sprite_set))
    }
}

pub(in crate::particles) struct EnchantedHitParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for EnchantedHitParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(MagicProvider::new(sprite_set))
    }
}

pub(in crate::particles) struct DamageIndicatorParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for DamageIndicatorParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(DamageIndicatorProvider::new(sprite_set))
    }
}
