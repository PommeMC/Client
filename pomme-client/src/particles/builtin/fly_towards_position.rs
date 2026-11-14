use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{
    Particle, ParticleRef, bounds_at, move_without_collision, resized_bounds,
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

pub struct FlyTowardsPositionParticle {
    position: DVec3,
    previous_position: DVec3,
    anchor: DVec3,
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
    alpha: Cell<f32>,
    roll: f32,
    previous_roll: f32,
    sprite: AtlasRegion,
}

impl FlyTowardsPositionParticle {
    fn new(anchor: DVec3, auxiliary: DVec3, sprite: AtlasRegion) -> Self {
        // Direct subclass coordinates leave the inherited bounds at the anchor.
        let bounds = bounds_at(anchor, 0.2, 0.2);
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let _base_quad_size = randomized_quad_size(&mut random);
        let position = anchor + auxiliary;
        let quad_size = 0.1_f32 * (random.f32() * 0.5_f32 + 0.2_f32);
        let brightness = random.f32() * 0.6_f32 + 0.4_f32;
        let lifetime = (random.f32() * 10.0_f32) as u32 + 30;
        Self {
            position,
            previous_position: position,
            anchor,
            velocity: auxiliary,
            bounds,
            width: 0.2,
            height: 0.2,
            removed: false,
            has_physics: false,
            age: 0,
            lifetime,
            quad_size,
            color: [0.9 * brightness, 0.9 * brightness, brightness],
            alpha: Cell::new(1.0),
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
        }
    }
}

impl Particle for FlyTowardsPositionParticle {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_single_quad(&self) -> Option<&dyn SingleQuadParticle> {
        Some(self)
    }

    fn tick(&mut self, _context: &mut LevelContext<'_>) {
        self.previous_position = self.position;
        let age = self.age;
        self.age += 1;
        if age >= self.lifetime {
            self.remove();
        } else {
            let mut pos = self.age as f32 / self.lifetime as f32;
            pos = 1.0 - pos;
            let mut pp = 1.0 - pos;
            pp *= pp;
            pp *= pp;
            // Vanilla assigns coordinates directly, leaving bounds unchanged.
            self.position = DVec3::new(
                self.anchor.x + self.velocity.x * f64::from(pos),
                self.anchor.y + self.velocity.y * f64::from(pos) - f64::from(pp * 1.2_f32),
                self.anchor.z + self.velocity.z * f64::from(pos),
            );
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

    fn get_light_coords(&self, world: &LevelContext<'_>, _partial_tick: f32) -> ParticleLight {
        let base = world.get_light_coords(self.position);
        let mut brightness = self.age as f32 / self.lifetime as f32;
        brightness *= brightness;
        brightness *= brightness;
        let emitted = (brightness.clamp(0.0, 1.0) * 240.0) as u16;
        ParticleLight {
            block: (u16::from(base.block) + emitted).min(240) as u8,
            sky: base.sky,
        }
    }
}

impl SingleQuadParticle for FlyTowardsPositionParticle {
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
        self.alpha.get()
    }

    fn set_alpha(&mut self, alpha: f32) {
        self.alpha.set(alpha);
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

    fn get_quad_size(&self, _partial_tick: f32) -> f32 {
        self.quad_size
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
        self.alpha.set(1.0);
        extract_camera_quad(self, self.previous_roll, world, view, output);
    }
}

pub struct EnchantProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl EnchantProvider {
    pub fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<SimpleParticleType> for EnchantProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(FlyTowardsPositionParticle::new(
            position, auxiliary, sprite,
        ))))
    }
}

pub(in crate::particles) struct EnchantParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for EnchantParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(EnchantProvider::new(sprite_set))
    }
}

pub struct NautilusProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for NautilusProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(FlyTowardsPositionParticle::new(
            position, auxiliary, sprite,
        ))))
    }
}

pub(in crate::particles) struct NautilusParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for NautilusParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(NautilusProvider {
            sprites: sprite_set,
        })
    }
}
