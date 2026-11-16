//! Concrete Vanilla GustSeed behavior, using the existing NoRender group.

use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{Particle, ParticleRef, bounds_at, randomized_motion, set_particle_size};
use super::super::particle_provider::ParticleProvider;
use super::super::render::ParticleRenderType;
use super::super::single_quad_particle::SingleQuadParticle;
use super::super::types::{ParticleType, SimpleParticleType};
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors(exclude(
    gravity,
    friction,
    speed_up_when_y_motion_is_blocked,
))]
pub struct GustSeedParticle {
    random: fastrand::Rng,
    scatter_scale: f64,
    tick_delay: usize,
    gust: SimpleParticleType,
}

impl GustSeedParticle {
    fn new(
        position: DVec3,
        scatter_scale: f64,
        lifetime: usize,
        tick_delay: usize,
        gust: Rc<ParticleType<SimpleParticleType>>,
    ) -> Self {
        // NoRender's full-motion constructor consumes six OWN floats before
        // the same stream supplies child scatter, separate from LEVEL/ENGINE.
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let velocity = randomized_motion(&mut random, DVec3::ZERO);
        Self {
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
            lifetime: lifetime as u32,
            random,
            scatter_scale,
            tick_delay,
            gust: SimpleParticleType::new(gust),
        }
    }
}

impl Particle for GustSeedParticle {
    crate::particles::accessors::forward_particle_accessors!();

    fn as_single_quad(&self) -> Option<&dyn SingleQuadParticle> {
        None
    }

    fn tick(&mut self, context: &mut LevelContext<'_>) {
        if self.age as usize % (self.tick_delay + 1) == 0 {
            for _ in 0..3 {
                let x =
                    self.position.x + (self.random.f64() - self.random.f64()) * self.scatter_scale;
                let y =
                    self.position.y + (self.random.f64() - self.random.f64()) * self.scatter_scale;
                let z =
                    self.position.z + (self.random.f64() - self.random.f64()) * self.scatter_scale;
                context.add_particle(
                    &self.gust,
                    DVec3::new(x, y, z),
                    DVec3::new(f64::from(self.age as f32 / self.lifetime as f32), 0.0, 0.0),
                );
            }
        }
        let expired = self.age == self.lifetime;
        self.age += 1;
        if expired {
            self.remove();
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
        ParticleRenderType::NoRender
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
        self.set_size(0.2 * factor, 0.2 * factor);
    }
}

pub struct GustSeedProvider {
    scatter_scale: f64,
    lifetime: usize,
    tick_delay: usize,
    gust: Rc<ParticleType<SimpleParticleType>>,
}

impl GustSeedProvider {
    pub fn new(
        scatter_scale: f64,
        lifetime: usize,
        tick_delay: usize,
        gust: Rc<ParticleType<SimpleParticleType>>,
    ) -> Self {
        Self {
            scatter_scale,
            lifetime,
            tick_delay,
            gust,
        }
    }
}

impl ParticleProvider<SimpleParticleType> for GustSeedProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        _random: &RandomSource,
    ) -> Option<ParticleRef> {
        Some(Rc::new(RefCell::new(GustSeedParticle::new(
            position,
            self.scatter_scale,
            self.lifetime,
            self.tick_delay,
            Rc::clone(&self.gust),
        ))))
    }
}
