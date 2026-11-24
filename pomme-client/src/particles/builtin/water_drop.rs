use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{
    Particle, ParticleRef, bounds_at, randomized_motion, resized_bounds, set_particle_size,
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
use crate::position::BlockPos;
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors(exclude(friction, speed_up_when_y_motion_is_blocked,))]
#[pomme_particle_macros::quad_accessors]
pub struct WaterDropParticle {
    random: fastrand::Rng,
}

impl WaterDropParticle {
    fn new(position: DVec3, sprite: AtlasRegion) -> Self {
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let mut velocity = randomized_motion(&mut random, DVec3::ZERO);
        let quad_size = randomized_quad_size(&mut random);
        velocity.x *= f64::from(0.3_f32);
        velocity.y = f64::from(random.f32() * 0.2_f32 + 0.1_f32);
        velocity.z *= f64::from(0.3_f32);
        let lifetime = (8.0_f64 / (f64::from(random.f32()) * 0.8_f64 + 0.2_f64)) as u32;
        Self {
            position,
            previous_position: position,
            velocity,
            bounds: resized_bounds(bounds_at(position, 0.2_f32, 0.2_f32), 0.01_f32, 0.01_f32),
            width: 0.01,
            height: 0.01,
            removed: false,
            on_ground: false,
            stopped_by_collision: false,
            has_physics: true,
            age: 0,
            lifetime,
            gravity: 0.06,
            quad_size,
            color: [1.0; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
            random,
        }
    }
}

impl Particle for WaterDropParticle {
    crate::particles::accessors::forward_particle_accessors!(WaterDropParticle);

    fn as_single_quad(&self) -> Option<&dyn SingleQuadParticle> {
        Some(self)
    }

    fn tick(&mut self, context: &mut LevelContext<'_>) {
        if self.removed {
            return;
        }
        self.previous_position = self.position;
        if self.lifetime == 0 {
            self.remove();
            return;
        }
        self.lifetime -= 1;
        self.velocity.y -= f64::from(self.gravity);
        self.move_by(context, self.velocity);
        self.velocity *= f64::from(0.98_f32);
        if self.on_ground {
            if self.random.f32() < 0.5_f32 {
                self.remove();
            }
            self.velocity.x *= f64::from(0.7_f32);
            self.velocity.z *= f64::from(0.7_f32);
        }
        let pos = BlockPos::containing(self.position);
        // VoxelShape.max(Y, localX, localZ) probes Z then X in Vanilla 26.3.
        let column_x = f64::from(pos.x) + (self.position.z - f64::from(pos.z));
        let column_z = f64::from(pos.z) + (self.position.x - f64::from(pos.x));
        let shape_height = context
            .get_collision_shape(pos)
            .into_iter()
            .filter(|bounds| {
                column_x >= bounds.min.x
                    && column_x < bounds.max.x
                    && column_z >= bounds.min.z
                    && column_z < bounds.max.z
            })
            .map(|bounds| bounds.max.y - f64::from(pos.y))
            .fold(f64::NEG_INFINITY, f64::max);
        let offset = shape_height.max(context.get_fluid_height(pos));
        if offset > 0.0 && self.position.y < f64::from(pos.y) + offset {
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
        self.set_size(0.2_f32 * factor, 0.2_f32 * factor);
    }
}

impl SingleQuadParticle for WaterDropParticle {
    crate::particles::accessors::forward_quad_accessors!(WaterDropParticle);

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

pub struct SplashProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for SplashProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        let mut particle = WaterDropParticle::new(position, sprite);
        particle.gravity = 0.04_f32;
        if auxiliary.y == 0.0 && (auxiliary.x != 0.0 || auxiliary.z != 0.0) {
            particle.velocity = DVec3::new(auxiliary.x, 0.1_f64, auxiliary.z);
        }
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct SplashParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for SplashParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(SplashProvider {
            sprites: sprite_set,
        })
    }
}

pub struct RainProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for RainProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(WaterDropParticle::new(
            position, sprite,
        ))))
    }
}

pub(in crate::particles) struct RainParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for RainParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(RainProvider {
            sprites: sprite_set,
        })
    }
}
