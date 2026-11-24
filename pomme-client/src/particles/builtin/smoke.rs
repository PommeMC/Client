//! Concrete Vanilla Smoke behavior, independent of engine/runtime adoption.

use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{
    Particle, ParticleRef, bounds_at, randomized_motion, set_particle_size,
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
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors(exclude(
    gravity,
    friction,
    speed_up_when_y_motion_is_blocked
))]
#[pomme_particle_macros::quad_accessors]
pub struct SmokeParticle {
    sprites: Rc<dyn SpriteSet>,
}

impl SmokeParticle {
    fn new(position: DVec3, auxiliary: DVec3, scale: f32, sprites: Rc<dyn SpriteSet>) -> Self {
        let sprite = sprites.first();
        // Vanilla Smoke owns its construction stream, ignoring the provider's
        // engine RNG. fastrand preserves draw purposes, not Java seed sequences.
        let mut random = fastrand::Rng::new();
        // The base lifetime is overwritten, but its draw still precedes motion.
        let _base_lifetime = (4.0 / (random.f32() * 0.9 + 0.1)) as u32;
        let mut velocity = randomized_motion(&mut random, DVec3::ZERO);
        let mut quad_size = randomized_quad_size(&mut random);
        velocity = velocity * f64::from(0.1_f32) + auxiliary;
        let color = [random.f32() * 0.3; 3];
        quad_size *= 0.75 * scale;
        // Accepted f32 adaptation: Java's final lifetime denominator uses doubles.
        let lifetime = ((8.0 / (random.f32() * 0.8 + 0.2) * scale) as u32).max(1);
        let mut particle = Self {
            position,
            previous_position: position,
            velocity,
            bounds: bounds_at(position, 0.2, 0.2),
            width: 0.2,
            height: 0.2,
            removed: false,
            on_ground: false,
            stopped_by_collision: false,
            has_physics: true,
            age: 0,
            lifetime,
            quad_size,
            color,
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprites,
            sprite,
        };
        particle.sprite = particle.sprites.get(particle.age, particle.lifetime);
        particle
    }
}

impl Particle for SmokeParticle {
    crate::particles::accessors::forward_particle_accessors!(SmokeParticle);

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
            self.velocity.y -= 0.04 * f64::from(-0.1_f32);
            self.move_by(context, self.velocity);
            if self.position.y == self.previous_position.y {
                self.velocity.x *= 1.1;
                self.velocity.z *= 1.1;
            }
            self.velocity *= f64::from(0.96_f32);
            if self.on_ground {
                self.velocity.x *= f64::from(0.7_f32);
                self.velocity.z *= f64::from(0.7_f32);
            }
        }
        if !self.removed {
            self.sprite = self.sprites.get(self.age, self.lifetime);
        }
    }

    fn move_by(&mut self, world: &LevelContext<'_>, displacement: DVec3) {
        if self.stopped_by_collision {
            return;
        }
        let allowed = if self.has_physics
            && displacement != DVec3::ZERO
            && displacement.length_squared() < 10000.0
        {
            world.collide(&self.bounds, displacement, self.position.y)
        } else {
            displacement
        };
        if allowed != DVec3::ZERO {
            self.bounds = self.bounds.offset(allowed);
            self.position = DVec3::new(
                (self.bounds.min.x + self.bounds.max.x) / 2.0,
                self.bounds.min.y,
                (self.bounds.min.z + self.bounds.max.z) / 2.0,
            );
        }
        if displacement.y.abs() >= f64::from(1.0e-5_f32) && allowed.y.abs() < f64::from(1.0e-5_f32)
        {
            self.stopped_by_collision = true;
        }
        self.on_ground = displacement.y != allowed.y && displacement.y < 0.0;
        if displacement.x != allowed.x {
            self.velocity.x = 0.0;
        }
        if displacement.z != allowed.z {
            self.velocity.z = 0.0;
        }
    }

    fn get_group(&self) -> ParticleRenderType {
        ParticleRenderType::SingleQuads
    }

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

impl SingleQuadParticle for SmokeParticle {
    crate::particles::accessors::forward_quad_accessors!(SmokeParticle);

    fn set_sprite_from_age(&mut self, sprites: &dyn SpriteSet) {
        if !self.removed {
            self.sprite = sprites.get(self.age, self.lifetime);
        }
    }

    fn get_quad_size(&self, partial_tick: f32) -> f32 {
        self.quad_size
            * (((self.age as f32 + partial_tick) / self.lifetime as f32) * 32.0).clamp(0.0, 1.0)
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

/// Rust spelling of Vanilla `SmokeParticle.Provider`.
pub struct SmokeProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl SmokeProvider {
    pub fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<SimpleParticleType> for SmokeProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        _random: &RandomSource,
    ) -> Option<ParticleRef> {
        Some(Rc::new(RefCell::new(SmokeParticle::new(
            position,
            auxiliary,
            1.0,
            Rc::clone(&self.sprites),
        ))))
    }
}

pub(in crate::particles) struct SmokeParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for SmokeParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(SmokeProvider::new(sprite_set))
    }
}

pub struct LargeSmokeProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl LargeSmokeProvider {
    pub fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<SimpleParticleType> for LargeSmokeProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        _random: &RandomSource,
    ) -> Option<ParticleRef> {
        Some(Rc::new(RefCell::new(SmokeParticle::new(
            position,
            auxiliary,
            2.5_f32,
            Rc::clone(&self.sprites),
        ))))
    }
}

pub(in crate::particles) struct LargeSmokeParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for LargeSmokeParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(LargeSmokeProvider::new(sprite_set))
    }
}

pub struct WhiteSmokeProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for WhiteSmokeProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        _random: &RandomSource,
    ) -> Option<ParticleRef> {
        // Vanilla WhiteSmoke shares Smoke's BaseAsh parameters, then overrides birth
        // color.
        let mut particle =
            SmokeParticle::new(position, auxiliary, 1.0_f32, Rc::clone(&self.sprites));
        particle.set_color([0.7294118_f32, 0.69411767_f32, 0.7607843_f32]);
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct WhiteSmokeParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for WhiteSmokeParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(WhiteSmokeProvider {
            sprites: sprite_set,
        })
    }
}
