use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{Particle, ParticleRef, bounds_at, set_particle_size};
use super::super::particle_provider::ParticleProvider;
use super::super::power_particle_option::PowerParticleOption;
use super::super::render::{ParticleLayer, ParticleRenderType, ParticleView, QuadParticleRenderState};
use super::super::resources::particle_resources::SpriteParticleRegistration;
use super::super::resources::sprite_set::SpriteSet;
use super::super::single_quad_particle::{
    SingleQuadParticle, extract_camera_quad, randomized_quad_size,
};
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors(exclude(gravity, speed_up_when_y_motion_is_blocked))]
#[pomme_particle_macros::quad_accessors]
pub(crate) struct DragonBreathParticle {
    has_hit_ground: bool,
    sprites: Rc<dyn SpriteSet>,
}

impl DragonBreathParticle {
    fn new(position: DVec3, auxiliary: DVec3, sprites: Rc<dyn SpriteSet>) -> Self {
        let sprite = sprites.first();
        let mut random = fastrand::Rng::new();
        // Position-only super: retain its overwritten lifetime draw, not motion noise.
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9 + 0.1)) as u32;
        let mut quad_size = randomized_quad_size(&mut random);
        let red = random.f32() * (0.8745098_f32 - 0.7176471_f32) + 0.7176471_f32;
        // Mth.nextFloat's equal green endpoints consume no draw.
        let blue = random.f32() * (0.9764706_f32 - 0.8235294_f32) + 0.8235294_f32;
        quad_size *= 0.75;
        let lifetime = (20.0 / (f64::from(random.f32()) * 0.8 + 0.2)) as u32;
        let mut particle = Self {
            position,
            previous_position: position,
            velocity: auxiliary,
            bounds: bounds_at(position, 0.2, 0.2),
            width: 0.2,
            height: 0.2,
            removed: false,
            on_ground: false,
            stopped_by_collision: false,
            has_physics: false,
            has_hit_ground: false,
            friction: 0.96,
            age: 0,
            lifetime,
            quad_size,
            color: [red, 0.0, blue],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprites,
            sprite,
        };
        particle.sprite = particle.sprites.get(particle.age, particle.lifetime);
        particle
    }

    fn set_power(&mut self, power: f32) {
        let power = f64::from(power);
        self.velocity.x *= power;
        self.velocity.y = (self.velocity.y - f64::from(0.1_f32)) * power + f64::from(0.1_f32);
        self.velocity.z *= power;
    }
}

impl Particle for DragonBreathParticle {
    crate::particles::accessors::forward_particle_accessors!(DragonBreathParticle);

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
            if !self.removed {
                self.sprite = self.sprites.get(self.age, self.lifetime);
            }
            if self.on_ground {
                self.velocity.y = 0.0;
                self.has_hit_ground = true;
            }
            if self.has_hit_ground {
                self.velocity.y += 0.002;
            }
            self.move_by(context, self.velocity);
            if self.position.y == self.previous_position.y {
                self.velocity.x *= 1.1;
                self.velocity.z *= 1.1;
            }
            self.velocity.x *= f64::from(self.friction);
            self.velocity.z *= f64::from(self.friction);
            if self.has_hit_ground {
                self.velocity.y *= f64::from(self.friction);
            }
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

impl SingleQuadParticle for DragonBreathParticle {
    crate::particles::accessors::forward_quad_accessors!(DragonBreathParticle);

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

pub(crate) struct DragonBreathProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl DragonBreathProvider {
    fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<PowerParticleOption> for DragonBreathProvider {
    fn create_particle(
        &self,
        options: &PowerParticleOption,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        _random: &RandomSource,
    ) -> Option<ParticleRef> {
        let mut particle = DragonBreathParticle::new(position, auxiliary, Rc::clone(&self.sprites));
        particle.set_power(options.get_power());
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct DragonBreathParticleRegistration;

impl SpriteParticleRegistration<PowerParticleOption> for DragonBreathParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<PowerParticleOption>> {
        Rc::new(DragonBreathProvider::new(sprite_set))
    }
}
