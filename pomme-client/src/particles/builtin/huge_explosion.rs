//! Concrete Vanilla HugeExplosion particle and sprite provider.

use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{
    Particle, ParticleRef, bounds_at, randomized_motion, set_particle_size,
};
use super::super::particle_provider::ParticleProvider;
use super::super::render::{
    ParticleLayer, ParticleLight, ParticleRenderType, ParticleView, QuadParticleRenderState,
};
use super::super::resources::particle_resources::SpriteParticleRegistration;
use super::super::resources::sprite_set::SpriteSet;
use super::super::single_quad_particle::{
    SingleQuadParticle, extract_camera_quad, randomized_quad_size,
};
use super::super::types::SimpleParticleType;
use crate::renderer::chunk::block_lighter::LightLevels;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors(exclude(
    gravity,
    friction,
    speed_up_when_y_motion_is_blocked,
))]
#[pomme_particle_macros::quad_accessors]
pub struct HugeExplosionParticle {
    sprites: Rc<dyn SpriteSet>,
}

impl HugeExplosionParticle {
    fn new(position: DVec3, size: f64, sprites: Rc<dyn SpriteSet>) -> Self {
        let sprite = sprites.first();
        // OWN stream: preserve full-motion base constructor draws even though
        // this stationary tick replaces base physics. Not Java seeded parity.
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let velocity = randomized_motion(&mut random, DVec3::ZERO);
        let _base_quad_size = randomized_quad_size(&mut random);
        let lifetime = 6 + random.u32(0..4);
        let color = [random.f32() * 0.6_f32 + 0.4_f32; 3];
        let quad_size = 2.0_f32 * (1.0_f32 - (size as f32) * 0.5_f32);
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

impl Particle for HugeExplosionParticle {
    crate::particles::accessors::forward_particle_accessors!(HugeExplosionParticle);

    fn as_single_quad(&self) -> Option<&dyn SingleQuadParticle> {
        Some(self)
    }

    fn tick(&mut self, _context: &mut LevelContext<'_>) {
        self.previous_position = self.position;
        let age = self.age;
        self.age += 1;
        if age >= self.lifetime {
            self.remove();
        } else if !self.removed {
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

    fn get_light_coords(&self, _world: &LevelContext<'_>, _partial_tick: f32) -> ParticleLight {
        ParticleLight {
            block: LightLevels::FULL_BRIGHT.block() * 16,
            sky: LightLevels::FULL_BRIGHT.sky() * 16,
        }
    }
}

impl SingleQuadParticle for HugeExplosionParticle {
    crate::particles::accessors::forward_quad_accessors!(HugeExplosionParticle);

    fn set_sprite_from_age(&mut self, sprites: &dyn SpriteSet) {
        if !self.removed {
            self.sprite = sprites.get(self.age, self.lifetime);
        }
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
        extract_camera_quad(self, self.previous_roll, world, view, output);
    }
}

pub struct HugeExplosionProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for HugeExplosionProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        _random: &RandomSource,
    ) -> Option<ParticleRef> {
        Some(Rc::new(RefCell::new(HugeExplosionParticle::new(
            position,
            auxiliary.x,
            Rc::clone(&self.sprites),
        ))))
    }
}

pub(in crate::particles) struct HugeExplosionParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for HugeExplosionParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(HugeExplosionProvider {
            sprites: sprite_set,
        })
    }
}

pub struct SonicBoomProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for SonicBoomProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        _random: &RandomSource,
    ) -> Option<ParticleRef> {
        // COMPAT: Source's constructor-only SonicBoom subclass reuses Huge's simulation
        // through its canonical provider, like the WhiteSmoke/Town constructor
        // variants.
        let mut particle =
            HugeExplosionParticle::new(position, auxiliary.x, Rc::clone(&self.sprites));
        particle.lifetime = 16;
        particle.quad_size = 1.5_f32;
        particle.sprite = particle.sprites.get(particle.age, particle.lifetime);
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct SonicBoomParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for SonicBoomParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(SonicBoomProvider {
            sprites: sprite_set,
        })
    }
}
