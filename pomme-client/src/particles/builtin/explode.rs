//! Concrete Vanilla Explode behavior for POOF, with its own simulation state.

use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{
    Particle, ParticleRef, bounds_at, move_without_collision, set_particle_size,
};
use super::super::particle_provider::ParticleProvider;
use super::super::render::{
    ParticleLayer, ParticleRenderType, ParticleView, QuadParticleRenderState,
};
use super::super::resources::particle_resources::SpriteParticleRegistration;
use super::super::resources::sprite_set::SpriteSet;
use super::super::single_quad_particle::{SingleQuadParticle, extract_camera_quad};
use super::super::types::SimpleParticleType;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors(exclude(
    gravity,
    friction,
    speed_up_when_y_motion_is_blocked,
))]
#[pomme_particle_macros::quad_accessors]
pub struct ExplodeParticle {
    sprites: Rc<dyn SpriteSet>,
}

impl ExplodeParticle {
    fn new(position: DVec3, auxiliary: DVec3, sprites: Rc<dyn SpriteSet>) -> Self {
        let sprite = sprites.first();
        // Own stream: preserve Java draw purposes/order, not seeded bit sequences.
        let mut random = fastrand::Rng::new();
        // Position-only base constructors consume these before Explode overwrites
        // them. No full-motion base constructor draws or normalization.
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9 + 0.1)) as u32;
        let _base_quad_size = 0.1_f32 * (random.f32() * 0.5 + 0.5) * 2.0;
        let velocity = DVec3::new(
            auxiliary.x + f64::from((random.f32() * 2.0 - 1.0) * 0.05),
            auxiliary.y + f64::from((random.f32() * 2.0 - 1.0) * 0.05),
            auxiliary.z + f64::from((random.f32() * 2.0 - 1.0) * 0.05),
        );
        let gray = random.f32() * 0.3 + 0.7;
        let quad_size = 0.1_f32 * (random.f32() * random.f32() * 6.0 + 1.0);
        let lifetime = (16.0_f64 / (f64::from(random.f32()) * 0.8 + 0.2)) as u32 + 2;
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
            color: [gray; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprites,
            sprite,
        };
        if !particle.removed {
            particle.sprite = particle.sprites.get(particle.age, particle.lifetime);
        }
        particle
    }
}

impl Particle for ExplodeParticle {
    crate::particles::accessors::forward_particle_accessors!(ExplodeParticle);

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
            self.velocity *= f64::from(0.9_f32);
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
            move_without_collision(&mut self.bounds, &mut self.position, allowed);
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

impl SingleQuadParticle for ExplodeParticle {
    crate::particles::accessors::forward_quad_accessors!(ExplodeParticle);

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

pub struct ExplodeProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for ExplodeProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        _random: &RandomSource,
    ) -> Option<ParticleRef> {
        Some(Rc::new(RefCell::new(ExplodeParticle::new(
            position,
            auxiliary,
            Rc::clone(&self.sprites),
        ))))
    }
}

pub(in crate::particles) struct ExplodeParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for ExplodeParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(ExplodeProvider {
            sprites: sprite_set,
        })
    }
}
