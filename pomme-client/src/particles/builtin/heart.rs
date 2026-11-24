//! Concrete Vanilla Heart behavior and its two description-bound providers.

use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{
    Particle, ParticleRef, bounds_at, move_without_collision, randomized_motion, set_particle_size,
};
use super::super::particle_provider::ParticleProvider;
use super::super::render::{
    ParticleLayer, ParticleRenderType, ParticleView, QuadParticleRenderState,
};
use super::super::resources::particle_resources::SpriteParticleRegistration;
use super::super::resources::sprite_set::SpriteSet;
use super::super::single_quad_particle::{
    SingleQuadParticle, extract_camera_quad, randomized_quad_size,
};
use super::super::types::SimpleParticleType;
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors]
#[pomme_particle_macros::quad_accessors]
pub struct HeartParticle {}

impl HeartParticle {
    fn new(position: DVec3, sprite: AtlasRegion) -> Self {
        let mut random = fastrand::Rng::new();
        let bounds = bounds_at(position, 0.2, 0.2);
        // The full-motion base constructor consumes these before Heart overrides them.
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let mut velocity = randomized_motion(&mut random, DVec3::ZERO);
        let mut quad_size = randomized_quad_size(&mut random);
        velocity *= f64::from(0.01_f32);
        velocity.y += 0.1;
        quad_size *= 1.5;
        Self {
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
            gravity: 0.0,
            friction: 0.86,
            speed_up_when_y_motion_is_blocked: true,
            age: 0,
            lifetime: 16,
            quad_size,
            color: [1.0; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
        }
    }
}

impl Particle for HeartParticle {
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

    fn get_group(&self) -> ParticleRenderType {
        ParticleRenderType::SingleQuads
    }

    crate::particles::accessors::forward_particle_accessors!();

    fn set_pos(&mut self, position: DVec3) {
        self.position = position;
        self.bounds = bounds_at(position, self.width, self.height);
    }

    fn set_size(&mut self, width: f32, height: f32) {
        set_particle_size(
            &mut self.width,
            &mut self.height,
            &mut self.bounds,
            width,
            height,
        );
    }

    fn scale(&mut self, factor: f32) {
        self.quad_size *= factor;
        self.set_size(0.2 * factor, 0.2 * factor);
    }
}

impl SingleQuadParticle for HeartParticle {
    crate::particles::accessors::forward_quad_accessors!();

    fn set_sprite_from_age(&mut self, sprites: &dyn SpriteSet) {
        if !self.removed {
            self.sprite = sprites.get(self.age, self.lifetime);
        }
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

/// Rust spelling of Vanilla `HeartParticle.Provider`.
pub struct HeartProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl HeartProvider {
    fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<SimpleParticleType> for HeartProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(HeartParticle::new(position, sprite))))
    }
}

/// Rust spelling of Vanilla `HeartParticle.AngryVillagerProvider`.
pub struct AngryVillagerProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl AngryVillagerProvider {
    fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<SimpleParticleType> for AngryVillagerProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        let mut particle =
            HeartParticle::new(DVec3::new(position.x, position.y + 0.5, position.z), sprite);
        particle.set_color([1.0; 3]);
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct HeartParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for HeartParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(HeartProvider::new(sprite_set))
    }
}

pub(in crate::particles) struct AngryVillagerParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for AngryVillagerParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(AngryVillagerProvider::new(sprite_set))
    }
}
