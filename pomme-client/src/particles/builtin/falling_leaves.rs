//! Source FallingLeavesParticle / FallingParticle, Cherry and Pale Oak
//! providers only.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{Particle, ParticleRef, bounds_at, move_without_collision, resized_bounds};
use super::super::particle_provider::ParticleProvider;
use super::super::render::{
    ParticleFacing, ParticleLayer, ParticleLight, ParticleRenderType, ParticleView,
    QuadParticleRenderState,
};
use super::super::resources::particle_resources::SpriteParticleRegistration;
use super::super::resources::sprite_set::SpriteSet;
use super::super::single_quad_particle::{
    SingleQuadParticle, extract_camera_quad, randomized_quad_size,
};
use super::super::types::{ParticleLimit, SimpleParticleType};
use crate::physics::aabb::Aabb;
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

pub struct FallingLeavesParticle {
    position: DVec3,
    previous_position: DVec3,
    velocity: DVec3,
    bounds: Aabb,
    width: f32,
    height: f32,
    removed: bool,
    on_ground: bool,
    stopped_by_collision: bool,
    has_physics: bool,
    remaining: i32,
    quad_size: f32,
    color: [f32; 3],
    alpha: f32,
    roll: f32,
    previous_roll: f32,
    sprite: AtlasRegion,
    gravity: f32,
    rot_speed: f32,
    spin_acceleration: f32,
    wind_big: f32,
    swirl: bool,
    flow_away: bool,
    xa_flow_scale: f64,
    za_flow_scale: f64,
    swirl_period: f64,
}

impl FallingLeavesParticle {
    fn new(
        position: DVec3,
        sprite: AtlasRegion,
        fall_acceleration: f32,
        side_acceleration: f32,
        swirl: bool,
        flow_away: bool,
        scale: f32,
        start_velocity: f32,
    ) -> Self {
        let mut random = fastrand::Rng::new();
        // Retain both overwritten parent draws before FallingParticle's fields.
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as i32;
        let _base_quad_size = randomized_quad_size(&mut random);
        let rot_speed = (if random.bool() { -30.0_f64 } else { 30.0_f64 }).to_radians() as f32;
        let spin_acceleration =
            (if random.bool() { -5.0_f64 } else { 5.0_f64 }).to_radians() as f32;
        let size = scale * if random.bool() { 0.05_f32 } else { 0.075_f32 };
        let particle_random = random.f32();
        let flow_angle = f64::from(particle_random * 60.0_f32).to_radians();
        Self {
            position,
            previous_position: position,
            velocity: DVec3::new(0.0, f64::from(-start_velocity), 0.0),
            bounds: resized_bounds(bounds_at(position, 0.2_f32, 0.2_f32), size, size),
            width: size,
            height: size,
            removed: false,
            on_ground: false,
            stopped_by_collision: false,
            has_physics: true,
            remaining: 300,
            quad_size: size,
            color: [1.0; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
            gravity: fall_acceleration * 1.2_f32 * 0.0025_f32,
            rot_speed,
            spin_acceleration,
            wind_big: side_acceleration,
            swirl,
            flow_away,
            xa_flow_scale: flow_angle.cos() * f64::from(side_acceleration),
            za_flow_scale: flow_angle.sin() * f64::from(side_acceleration),
            swirl_period: f64::from(1000.0_f32 + particle_random * 3000.0_f32).to_radians(),
        }
    }
}

impl Particle for FallingLeavesParticle {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn as_single_quad(&self) -> Option<&dyn SingleQuadParticle> {
        Some(self)
    }

    fn tick(&mut self, context: &mut LevelContext<'_>) {
        self.previous_position = self.position;
        let remaining = self.remaining;
        self.remaining = self.remaining.wrapping_sub(1);
        if remaining <= 0 {
            self.remove();
        }
        if self.removed {
            return;
        }
        let alive_ticks = (300 - self.remaining) as f32;
        let relative_age = (alive_ticks / 300.0_f32).min(1.0_f32);
        let mut xa = 0.0;
        let mut za = 0.0;
        if self.flow_away {
            xa += self.xa_flow_scale * f64::from(relative_age).powf(1.25);
            za += self.za_flow_scale * f64::from(relative_age).powf(1.25);
        }
        if self.swirl {
            xa += f64::from(relative_age)
                * (f64::from(relative_age) * self.swirl_period).cos()
                * f64::from(self.wind_big);
            za += f64::from(relative_age)
                * (f64::from(relative_age) * self.swirl_period).sin()
                * f64::from(self.wind_big);
        }
        self.velocity.x += xa * f64::from(0.0025_f32);
        self.velocity.z += za * f64::from(0.0025_f32);
        self.velocity.y -= f64::from(self.gravity);
        self.rot_speed += self.spin_acceleration / 20.0_f32;
        self.previous_roll = self.roll;
        self.roll += self.rot_speed / 20.0_f32;
        self.move_by(context, self.velocity);
        if self.on_ground
            || (self.remaining < 299 && (self.velocity.x == 0.0 || self.velocity.z == 0.0))
        {
            self.remove();
        }
        if !self.removed {
            self.velocity *= f64::from(1.0_f32);
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
    fn remove(&mut self) {
        self.removed = true;
    }
    fn is_alive(&self) -> bool {
        !self.removed
    }
    fn get_group(&self) -> ParticleRenderType {
        ParticleRenderType::SingleQuads
    }
    fn get_particle_limit(&self) -> Option<ParticleLimit> {
        None
    }
    fn get_pos(&self) -> DVec3 {
        self.position
    }
    fn set_pos(&mut self, position: DVec3) {
        self.position = position;
        self.bounds = bounds_at(position, self.width, self.height);
    }
    fn get_previous_pos(&self) -> DVec3 {
        self.previous_position
    }
    fn get_velocity(&self) -> DVec3 {
        self.velocity
    }
    fn set_velocity(&mut self, velocity: DVec3) {
        self.velocity = velocity;
    }
    fn get_age(&self) -> u32 {
        0
    }
    // The trait is unsigned; keep Source's signed countdown (including terminal -1)
    // locally.
    fn get_lifetime(&self) -> u32 {
        self.remaining.max(0) as u32
    }
    fn set_lifetime(&mut self, lifetime: u32) {
        self.remaining = lifetime.min(i32::MAX as u32) as i32;
    }
    fn get_bounding_box(&self) -> Aabb {
        self.bounds
    }
    fn set_bounding_box(&mut self, bounds: Aabb) {
        self.bounds = bounds;
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
        self.set_size(0.2_f32 * factor, 0.2_f32 * factor);
    }
    fn has_physics(&self) -> bool {
        self.has_physics
    }
    fn set_has_physics(&mut self, enabled: bool) {
        self.has_physics = enabled;
    }
    fn get_light_coords(&self, world: &LevelContext<'_>, _partial_tick: f32) -> ParticleLight {
        world.get_light_coords(self.position)
    }
}

impl SingleQuadParticle for FallingLeavesParticle {
    fn get_sprite(&self) -> AtlasRegion {
        self.sprite
    }
    fn set_sprite(&mut self, sprite: AtlasRegion) {
        self.sprite = sprite;
    }
    fn set_sprite_from_age(&mut self, sprites: &dyn SpriteSet) {
        if !self.removed {
            self.sprite = sprites.get(0, self.get_lifetime());
        }
    }
    fn get_uv(&self) -> [f32; 4] {
        [
            self.sprite.u_min,
            self.sprite.u_max,
            self.sprite.v_min,
            self.sprite.v_max,
        ]
    }
    fn get_color(&self) -> [f32; 3] {
        self.color
    }
    fn set_color(&mut self, color: [f32; 3]) {
        self.color = color;
    }
    fn get_alpha(&self) -> f32 {
        self.alpha
    }
    fn set_alpha(&mut self, alpha: f32) {
        self.alpha = alpha;
    }
    fn get_roll(&self) -> f32 {
        self.roll
    }
    fn set_roll(&mut self, roll: f32) {
        self.roll = roll;
    }
    fn get_facing(&self) -> ParticleFacing {
        ParticleFacing::Camera
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
        let index = output.len();
        extract_camera_quad(self, self.previous_roll, world, view, output);
        if let Some(quad) = output.get_mut(index) {
            quad.color = quad
                .color
                .map(|channel| (channel * 255.0_f32).floor() / 255.0_f32);
        }
    }
}

pub struct CherryProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for CherryProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(FallingLeavesParticle::new(
            position, sprite, 0.25, 2.0, false, true, 1.0, 0.0,
        ))))
    }
}

pub struct PaleOakProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for PaleOakProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(FallingLeavesParticle::new(
            position, sprite, 0.07, 10.0, true, false, 2.0, 0.021,
        ))))
    }
}

pub(in crate::particles) struct CherryParticleRegistration;
impl SpriteParticleRegistration<SimpleParticleType> for CherryParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(CherryProvider {
            sprites: sprite_set,
        })
    }
}

pub(in crate::particles) struct PaleOakParticleRegistration;
impl SpriteParticleRegistration<SimpleParticleType> for PaleOakParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(PaleOakProvider {
            sprites: sprite_set,
        })
    }
}
