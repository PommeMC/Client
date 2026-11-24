use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{
    Particle, ParticleRef, bounds_at, move_without_collision, randomized_motion, resized_bounds,
    set_particle_size,
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
use super::super::types::{ParticleLimit, SimpleParticleType};
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors(exclude(friction, speed_up_when_y_motion_is_blocked,))]
#[pomme_particle_macros::quad_accessors]
pub struct SuspendedParticle {
    limit: Option<ParticleLimit>,
}

impl SuspendedParticle {
    fn new(position: DVec3, sprite: AtlasRegion) -> Self {
        Self::with_auxiliary(position, sprite, None)
    }

    fn with_velocity(position: DVec3, sprite: AtlasRegion, auxiliary: DVec3) -> Self {
        Self::with_auxiliary(position, sprite, Some(auxiliary))
    }

    fn with_auxiliary(position: DVec3, sprite: AtlasRegion, auxiliary: Option<DVec3>) -> Self {
        let position = position - DVec3::new(0.0, 0.125, 0.0);
        let mut random = fastrand::Rng::new();
        // Preserve base lifetime before optional motion and the three quad/life draws.
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let (velocity, quad_offset) = match auxiliary {
            Some(auxiliary) => (randomized_motion(&mut random, auxiliary), 0.6_f32),
            None => (DVec3::ZERO, 0.2_f32),
        };
        let mut quad_size = randomized_quad_size(&mut random);
        let bounds = resized_bounds(bounds_at(position, 0.2, 0.2), 0.01, 0.01);
        quad_size *= random.f32() * 0.6_f32 + quad_offset;
        let lifetime = (16.0_f64 / (f64::from(random.f32()) * 0.8_f64 + 0.2_f64)) as u32;
        Self {
            position,
            previous_position: position,
            velocity,
            bounds,
            width: 0.01,
            height: 0.01,
            removed: false,
            on_ground: false,
            stopped_by_collision: false,
            has_physics: false,
            age: 0,
            lifetime,
            gravity: 0.0,
            limit: None,
            quad_size,
            color: [1.0; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
        }
    }
}

impl Particle for SuspendedParticle {
    crate::particles::accessors::forward_particle_accessors!(SuspendedParticle);

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
    fn get_particle_limit(&self) -> Option<ParticleLimit> {
        self.limit
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

impl SingleQuadParticle for SuspendedParticle {
    crate::particles::accessors::forward_quad_accessors!(SuspendedParticle);

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

pub struct UnderwaterProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for UnderwaterProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        let mut particle = SuspendedParticle::new(position, sprite);
        particle.set_color([0.4, 0.4, 0.7]);
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct UnderwaterParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for UnderwaterParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(UnderwaterProvider {
            sprites: sprite_set,
        })
    }
}

pub struct SporeBlossomAirProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for SporeBlossomAirProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        let mut particle = SuspendedParticle::with_velocity(
            position,
            sprite,
            DVec3::new(0.0, f64::from(-0.8_f32), 0.0),
        );
        particle.lifetime = (random.next_int(501) + 500) as u32;
        particle.gravity = 0.01_f32;
        particle.set_color([0.32, 0.5, 0.22]);
        particle.limit = Some(ParticleLimit { limit: 1000 });
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct SporeBlossomAirParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for SporeBlossomAirParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(SporeBlossomAirProvider {
            sprites: sprite_set,
        })
    }
}

pub struct CrimsonSporeProvider {
    sprites: Rc<dyn SpriteSet>,
}

fn crimson_gaussian(random: &RandomSource) -> f64 {
    loop {
        let x = 2.0 * random.next_double() - 1.0;
        let y = 2.0 * random.next_double() - 1.0;
        let squared = x * x + y * y;
        if squared > 0.0 && squared < 1.0 {
            return x * (-2.0 * squared.ln() / squared).sqrt();
        }
    }
}

impl ParticleProvider<SimpleParticleType> for CrimsonSporeProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let x = crimson_gaussian(random) * f64::from(1.0e-6_f32);
        let y = crimson_gaussian(random) * f64::from(1.0e-4_f32);
        let z = crimson_gaussian(random) * f64::from(1.0e-6_f32);
        let sprite = self.sprites.get_random(random);
        let mut particle = SuspendedParticle::with_velocity(position, sprite, DVec3::new(x, y, z));
        particle.set_color([0.9, 0.4, 0.5]);
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct CrimsonSporeParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for CrimsonSporeParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(CrimsonSporeProvider {
            sprites: sprite_set,
        })
    }
}

pub struct WarpedSporeProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for WarpedSporeProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let y = f64::from(random.next_float()) * -1.9 * f64::from(random.next_float()) * 0.1;
        let sprite = self.sprites.get_random(random);
        let mut particle =
            SuspendedParticle::with_velocity(position, sprite, DVec3::new(0.0, y, 0.0));
        particle.set_color([0.1, 0.1, 0.3]);
        particle.set_size(0.001, 0.001);
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct WarpedSporeParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for WarpedSporeParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(WarpedSporeProvider {
            sprites: sprite_set,
        })
    }
}
