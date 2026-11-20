use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{
    Particle, ParticleRef, bounds_at, move_without_collision, resized_bounds, set_particle_size,
};
use super::super::particle_provider::ParticleProvider;
use super::super::render::{ParticleLayer, ParticleRenderType, ParticleView, QuadParticleRenderState};
use super::super::resources::particle_resources::SpriteParticleRegistration;
use super::super::resources::sprite_set::SpriteSet;
use super::super::single_quad_particle::{
    SingleQuadParticle, extract_camera_quad, randomized_quad_size,
};
use super::super::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::util::random_source::RandomSource;
use crate::world::block::FluidKind;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors(exclude(
    gravity,
    friction,
    speed_up_when_y_motion_is_blocked
))]
#[pomme_particle_macros::quad_accessors]
pub struct BubbleColumnUpParticle {}

impl BubbleColumnUpParticle {
    fn new(position: DVec3, auxiliary: DVec3, sprite: AtlasRegion) -> Self {
        // COMPAT: Own constructor stream and source draw order, not Java seed parity.
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let mut quad_size = randomized_quad_size(&mut random);
        let bounds = resized_bounds(bounds_at(position, 0.2, 0.2), 0.02, 0.02);
        quad_size *= random.f32() * 0.6_f32 + 0.2_f32;
        let velocity = DVec3::new(
            auxiliary.x * f64::from(0.2_f32)
                + f64::from((random.f32() * 2.0_f32 - 1.0_f32) * 0.02_f32),
            auxiliary.y * f64::from(0.2_f32)
                + f64::from((random.f32() * 2.0_f32 - 1.0_f32) * 0.02_f32),
            auxiliary.z * f64::from(0.2_f32)
                + f64::from((random.f32() * 2.0_f32 - 1.0_f32) * 0.02_f32),
        );
        let lifetime = (40.0_f64 / (f64::from(random.f32()) * 0.8_f64 + 0.2_f64)) as u32;
        Self {
            position,
            previous_position: position,
            velocity,
            bounds,
            width: 0.02,
            height: 0.02,
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
        }
    }
}

impl Particle for BubbleColumnUpParticle {
    crate::particles::accessors::forward_particle_accessors!(BubbleColumnUpParticle);

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
            self.velocity.y -= 0.04_f64 * f64::from(-0.125_f32);
            self.move_by(context, self.velocity);
            self.velocity *= f64::from(0.85_f32);
            if self.on_ground {
                self.velocity.x *= f64::from(0.7_f32);
                self.velocity.z *= f64::from(0.7_f32);
            }
        }
        if self.is_alive() && !is_water(context, self.position) {
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

impl SingleQuadParticle for BubbleColumnUpParticle {
    crate::particles::accessors::forward_quad_accessors!(BubbleColumnUpParticle);

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

#[pomme_particle_macros::particle_accessors(exclude(
    gravity,
    friction,
    speed_up_when_y_motion_is_blocked
))]
#[pomme_particle_macros::quad_accessors]
pub struct WaterCurrentDownParticle {
    angle: f32,
}

impl WaterCurrentDownParticle {
    fn new(position: DVec3, sprite: AtlasRegion) -> Self {
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let mut quad_size = randomized_quad_size(&mut random);
        let lifetime = (random.f32() * 60.0_f32) as u32 + 30;
        let bounds = resized_bounds(bounds_at(position, 0.2, 0.2), 0.02, 0.02);
        quad_size *= random.f32() * 0.6_f32 + 0.2_f32;
        Self {
            position,
            previous_position: position,
            velocity: DVec3::new(0.0, -0.05, 0.0),
            bounds,
            width: 0.02,
            height: 0.02,
            removed: false,
            on_ground: false,
            stopped_by_collision: false,
            has_physics: false,
            age: 0,
            lifetime,
            quad_size,
            color: [1.0; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
            angle: 0.0,
        }
    }
}

impl Particle for WaterCurrentDownParticle {
    crate::particles::accessors::forward_particle_accessors!(WaterCurrentDownParticle);

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
            let scaled_angle = f64::from(self.angle) * 10430.378350470453_f64;
            self.velocity.x += f64::from(0.6_f32 * mth_sample(scaled_angle + 16384.0_f64));
            self.velocity.z += f64::from(0.6_f32 * mth_sample(scaled_angle));
            self.velocity.x *= 0.07_f64;
            self.velocity.z *= 0.07_f64;
            self.move_by(context, self.velocity);
            if !is_water(context, self.position) || self.on_ground {
                self.remove();
            }
            self.angle += 0.08_f32;
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

impl SingleQuadParticle for WaterCurrentDownParticle {
    crate::particles::accessors::forward_quad_accessors!(WaterCurrentDownParticle);

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

fn is_water(world: &LevelContext<'_>, position: DVec3) -> bool {
    // COMPAT: Vanilla FluidTags.WATER equivalence; custom fluid tags are
    // unsupported.
    let pos = BlockPos::containing(position);
    world
        .block_state_table()
        .block_state_data(world.get_block_state(pos))
        .fluid()
        .kind
        == FluidKind::Water
}

fn mth_sample(scaled_index: f64) -> f32 {
    // COMPAT: 26.3 double/long index and divided table samples, without a cache.
    // Java Math.sin versus Rust platform sine has no bit-parity guarantee.
    let index = (scaled_index as i64) & 65535;
    (index as f64 / 10430.378350470453_f64).sin() as f32
}

pub struct BubbleColumnUpProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for BubbleColumnUpProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(BubbleColumnUpParticle::new(
            position, auxiliary, sprite,
        ))))
    }
}

pub struct WaterCurrentDownProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for WaterCurrentDownProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(WaterCurrentDownParticle::new(
            position, sprite,
        ))))
    }
}

pub(in crate::particles) struct BubbleColumnUpParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for BubbleColumnUpParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(BubbleColumnUpProvider {
            sprites: sprite_set,
        })
    }
}

pub(in crate::particles) struct WaterCurrentDownParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for WaterCurrentDownParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(WaterCurrentDownProvider {
            sprites: sprite_set,
        })
    }
}
