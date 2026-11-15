use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{
    Particle, ParticleRef, bounds_at, move_without_collision, randomized_motion, resized_bounds,
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
use crate::position::BlockPos;
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors]
#[pomme_particle_macros::quad_accessors]
pub struct FireflyParticle {
    random: fastrand::Rng,
}

impl FireflyParticle {
    fn new(position: DVec3, auxiliary: DVec3, sprite: AtlasRegion) -> Self {
        let mut random = fastrand::Rng::new();
        let lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let mut velocity = randomized_motion(&mut random, auxiliary);
        let mut quad_size = randomized_quad_size(&mut random);
        quad_size *= 0.75_f32;
        velocity.y *= f64::from(0.8_f32);
        velocity.x *= f64::from(0.8_f32);
        velocity.z *= f64::from(0.8_f32);
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
            lifetime,
            gravity: 0.0,
            friction: 0.96,
            speed_up_when_y_motion_is_blocked: true,
            random,
            quad_size,
            color: [1.0; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
        }
    }

    fn lifetime_progress(&self, current_age: f32) -> f32 {
        (current_age / self.lifetime as f32).clamp(0.0, 1.0)
    }

    fn fade_amount(progress: f32, fade_in_time: f32, fade_out_time: f32) -> f32 {
        if progress >= 1.0_f32 - fade_in_time {
            (1.0_f32 - progress) / fade_in_time
        } else if progress <= fade_out_time {
            progress / fade_out_time
        } else {
            1.0
        }
    }
}

impl Particle for FireflyParticle {
    crate::particles::accessors::forward_particle_accessors!();

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
            self.velocity.y -= 0.04_f64 * f64::from(self.gravity);
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
        let state = context.get_block_state(BlockPos::containing(self.position));
        if !context.block_state_table().block_state_data(state).is_air() {
            self.remove();
        } else {
            self.set_alpha(Self::fade_amount(
                self.lifetime_progress(self.age as f32),
                0.3,
                0.5,
            ));
            if self.random.f32() > 0.95_f32 || self.age == 1 {
                let x = -0.05_f32 + 0.1_f32 * self.random.f32();
                let y = -0.05_f32 + 0.1_f32 * self.random.f32();
                let z = -0.05_f32 + 0.1_f32 * self.random.f32();
                self.set_velocity(DVec3::new(f64::from(x), f64::from(y), f64::from(z)));
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
    fn get_light_coords(&self, _world: &LevelContext<'_>, partial_tick: f32) -> ParticleLight {
        let progress = self.lifetime_progress(self.age as f32 + partial_tick);
        ParticleLight {
            block: (255.0_f32 * Self::fade_amount(progress, 0.1, 0.3)) as u8,
            sky: 0,
        }
    }
}

impl SingleQuadParticle for FireflyParticle {
    crate::particles::accessors::forward_quad_accessors!();

    fn set_sprite_from_age(&mut self, sprites: &dyn SpriteSet) {
        if !self.removed {
            self.sprite = sprites.get(self.age, self.lifetime);
        }
    }
    fn get_quad_size(&self, _partial_tick: f32) -> f32 {
        self.quad_size
    }
    fn get_layer(&self) -> ParticleLayer {
        ParticleLayer::Translucent
    }
    fn extract(
        &self,
        world: &LevelContext<'_>,
        view: &ParticleView<'_>,
        output: &mut Vec<QuadParticleRenderState>,
    ) {
        let index = output.len();
        extract_camera_quad(self, self.previous_roll, world, view, output);
        if let Some(quad) = output.get_mut(index) {
            quad.color = quad
                .color
                .map(|channel| (channel * 255.0_f32).floor() / 255.0_f32);
        }
    }
}

pub struct FireflyProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for FireflyProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let x = 0.5 - random.next_double();
        let y = if random.next_boolean() {
            auxiliary.y
        } else {
            -auxiliary.y
        };
        let z = 0.5 - random.next_double();
        let sprite = self.sprites.get_random(random);
        let mut particle = FireflyParticle::new(position, DVec3::new(x, y, z), sprite);
        particle.set_lifetime(random.next_int(101) as u32 + 200);
        particle.scale(1.5);
        particle.set_alpha(0.0);
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct FireflyParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for FireflyParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(FireflyProvider {
            sprites: sprite_set,
        })
    }
}
