//! Source SculkChargePopParticle and its Simple provider (not sculk_charge's
//! roll payload).

use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{
    Particle, ParticleRef, bounds_at, move_without_collision, randomized_motion, set_particle_size,
};
use super::super::particle_provider::ParticleProvider;
use super::super::render::{
    ParticleLayer, ParticleLight, ParticleRenderType, ParticleView, QuadParticleRenderState,
};
use super::super::resources::particle_resources::SpriteParticleRegistration;
use super::super::resources::sprite_set::SpriteSet;
use super::super::single_quad_particle::{
    SingleQuadParticle, extract_quantized_camera_quad, randomized_quad_size,
};
use super::super::types::SimpleParticleType;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors(exclude(
    age,
    lifetime,
    gravity,
    friction,
    speed_up_when_y_motion_is_blocked,
))]
#[pomme_particle_macros::quad_accessors]
pub struct SculkChargePopParticle {
    age: i32,
    lifetime: i32,
    sprites: Rc<dyn SpriteSet>,
}

impl SculkChargePopParticle {
    fn new(position: DVec3, auxiliary: DVec3, sprites: Rc<dyn SpriteSet>) -> Self {
        let sprite = sprites.first();
        let mut random = fastrand::Rng::new();
        let lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as i32;
        // Keep all five full-motion parent draws/calculations, despite provider
        // overwrite.
        let velocity = randomized_motion(&mut random, auxiliary);
        let quad_size = randomized_quad_size(&mut random);
        let mut particle = Self {
            position,
            previous_position: position,
            velocity,
            bounds: bounds_at(position, 0.2_f32, 0.2_f32),
            width: 0.2,
            height: 0.2,
            removed: false,
            on_ground: false,
            stopped_by_collision: false,
            has_physics: true,
            age: 0,
            lifetime,
            quad_size,
            color: [1.0; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
            sprites: Rc::clone(&sprites),
        };
        particle.scale(1.0_f32);
        particle.has_physics = false;
        particle.set_sprite_from_age(sprites.as_ref());
        particle
    }
}

impl Particle for SculkChargePopParticle {
    crate::particles::accessors::forward_particle_accessors!(SculkChargePopParticle);

    fn as_single_quad(&self) -> Option<&dyn SingleQuadParticle> {
        Some(self)
    }
    fn tick(&mut self, context: &mut LevelContext<'_>) {
        self.previous_position = self.position;
        let age = self.age;
        self.age = self.age.wrapping_add(1);
        if age >= self.lifetime {
            self.remove();
        } else {
            self.velocity.y -= 0.04_f64 * f64::from(0.0_f32);
            self.move_by(context, self.velocity);
            self.velocity *= f64::from(0.96_f32);
            if self.on_ground {
                self.velocity.x *= f64::from(0.7_f32);
                self.velocity.z *= f64::from(0.7_f32);
            }
        }
        let sprites = Rc::clone(&self.sprites);
        self.set_sprite_from_age(sprites.as_ref());
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
    fn get_age(&self) -> u32 {
        self.age.max(0) as u32
    }
    fn get_lifetime(&self) -> u32 {
        self.lifetime.max(0) as u32
    }
    fn set_lifetime(&mut self, lifetime: u32) {
        self.lifetime = lifetime.min(i32::MAX as u32) as i32;
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
        self.set_size(0.2_f32 * factor, 0.2_f32 * factor);
    }
    fn get_light_coords(&self, world: &LevelContext<'_>, _partial_tick: f32) -> ParticleLight {
        let mut light = world.get_light_coords(self.position);
        light.block = 240;
        light
    }
}

impl SingleQuadParticle for SculkChargePopParticle {
    crate::particles::accessors::forward_quad_accessors!(SculkChargePopParticle);

    fn set_sprite_from_age(&mut self, sprites: &dyn SpriteSet) {
        if !self.removed {
            self.sprite = sprites.get(self.get_age(), self.get_lifetime());
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
        extract_quantized_camera_quad(self, self.previous_roll, world, view, output);
    }
}

pub struct SculkChargePopProvider {
    sprites: Rc<dyn SpriteSet>,
}
impl ParticleProvider<SimpleParticleType> for SculkChargePopProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let mut particle =
            SculkChargePopParticle::new(position, auxiliary, Rc::clone(&self.sprites));
        particle.set_alpha(1.0_f32);
        particle.set_velocity(auxiliary);
        particle.set_lifetime(random.next_int(4) as u32 + 6);
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct SculkChargePopParticleRegistration;
impl SpriteParticleRegistration<SimpleParticleType> for SculkChargePopParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(SculkChargePopProvider {
            sprites: sprite_set,
        })
    }
}
