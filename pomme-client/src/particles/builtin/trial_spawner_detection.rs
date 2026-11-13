//! Concrete Vanilla TrialSpawnerDetectionParticle shared by both detection
//! sprite sets.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use glam::{DVec3, Quat};

use super::super::particle::{
    Particle, ParticleRef, bounds_at, move_without_collision, randomized_motion, resized_bounds,
};
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

pub struct TrialSpawnerDetectionParticle {
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
    age: u32,
    lifetime: u32,
    gravity: f32,
    friction: f32,
    quad_size: f32,
    color: [f32; 3],
    alpha: f32,
    roll: f32,
    previous_roll: f32,
    sprites: Rc<dyn SpriteSet>,
    sprite: AtlasRegion,
}

impl TrialSpawnerDetectionParticle {
    fn new(position: DVec3, auxiliary: DVec3, scale: f32, sprites: Rc<dyn SpriteSet>) -> Self {
        let sprite = sprites.first();
        // OWN8: full-motion parent7 (including overwritten lifetime/XYZ), then
        // lifetime.
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9 + 0.1)) as u32;
        let mut velocity = randomized_motion(&mut random, DVec3::ZERO);
        let mut quad_size = randomized_quad_size(&mut random);
        velocity.x *= 0.0;
        velocity.y *= 0.9;
        velocity.z *= 0.0;
        velocity += auxiliary;
        quad_size *= 0.75_f32 * scale;
        let lifetime = ((8.0_f32 / (random.f32() * 0.5 + 0.5) * scale) as u32).max(1);
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
            gravity: -0.1,
            friction: 0.96,
            quad_size,
            color: [1.0; 3],
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

// JOML 1.10.9 transform divides its quadratic rotation by N(q). Represent that
// geometry in the existing unit-quaternion quad field as q*sqrt(1/N), not a
// yaw reconstructed from Euler angles. Java/glam f32 rounding is not
// bit-identical.
fn detection_quad_rotation(view: &ParticleView<'_>, previous_roll: f32, roll: f32) -> Option<Quat> {
    let (mut x, mut y, mut z, mut w) = (0.0_f32, view.rotation.y, 0.0_f32, view.rotation.w);
    if roll != 0.0 {
        let angle = previous_roll + view.partial_tick * (roll - previous_roll);
        let half_angle = angle * 0.5;
        let sin = f64::from(half_angle).sin() as f32;
        // Source Math.cosFromSin, with its float arithmetic and Java int cast.
        let mut cos = f64::from(1.0_f32 - sin * sin).sqrt() as f32;
        let a = half_angle + std::f32::consts::FRAC_PI_2;
        let mut b = a - (a / std::f32::consts::TAU) as i32 as f32 * std::f32::consts::TAU;
        if b < 0.0 {
            b += std::f32::consts::TAU;
        }
        if b >= std::f32::consts::PI {
            cos = -cos;
        }
        (x, y, z, w) = (
            x * cos + y * sin,
            y * cos - x * sin,
            w * sin + z * cos,
            w * cos - z * sin,
        );
    }
    let norm_squared = ((x * x + y * y) + z * z) + w * w;
    if !norm_squared.is_finite() || norm_squared == 0.0 {
        // Source produces non-visible NaN vertices for singular/nonfinite q;
        // omit that quad rather than inventing an identity-facing fallback.
        return None;
    }
    let factor = (1.0_f32 / norm_squared).sqrt();
    Some(Quat::from_xyzw(
        x * factor,
        y * factor,
        z * factor,
        w * factor,
    ))
}

impl Particle for TrialSpawnerDetectionParticle {
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
        let age = self.age;
        self.age += 1;
        if age >= self.lifetime {
            self.remove();
        } else {
            self.velocity.y -= 0.04 * f64::from(self.gravity);
            self.move_by(context, self.velocity);
            if self.position.y == self.previous_position.y {
                self.velocity.x *= 1.1;
                self.velocity.z *= 1.1;
            }
            self.velocity *= f64::from(self.friction);
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
        self.age
    }

    fn get_lifetime(&self) -> u32 {
        self.lifetime
    }

    fn set_lifetime(&mut self, lifetime: u32) {
        self.lifetime = lifetime;
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
        self.set_size(0.2 * factor, 0.2 * factor);
    }

    fn has_physics(&self) -> bool {
        self.has_physics
    }

    fn set_has_physics(&mut self, enabled: bool) {
        self.has_physics = enabled;
    }

    fn get_light_coords(&self, world: &LevelContext<'_>, _partial_tick: f32) -> ParticleLight {
        let mut light = world.get_light_coords(self.position);
        light.block = 240;
        light
    }
}

impl SingleQuadParticle for TrialSpawnerDetectionParticle {
    fn get_sprite(&self) -> AtlasRegion {
        self.sprite
    }

    fn set_sprite(&mut self, sprite: AtlasRegion) {
        self.sprite = sprite;
    }

    fn set_sprite_from_age(&mut self, sprites: &dyn SpriteSet) {
        if !self.removed {
            self.sprite = sprites.get(self.age, self.lifetime);
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
        ParticleFacing::CameraY
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
        let Some(rotation) = detection_quad_rotation(view, self.previous_roll, self.roll) else {
            return;
        };
        let index = output.len();
        extract_camera_quad(self, self.previous_roll, world, view, output);
        if let Some(quad) = output.get_mut(index) {
            quad.rotation = rotation;
            quad.color = quad
                .color
                .map(|channel| (channel * 255.0_f32).floor() / 255.0_f32);
        }
    }
}

pub struct TrialSpawnerDetectionProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for TrialSpawnerDetectionProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        _random: &RandomSource,
    ) -> Option<ParticleRef> {
        Some(Rc::new(RefCell::new(TrialSpawnerDetectionParticle::new(
            position,
            auxiliary,
            1.5,
            Rc::clone(&self.sprites),
        ))))
    }
}

pub(in crate::particles) struct TrialSpawnerDetectionParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for TrialSpawnerDetectionParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(TrialSpawnerDetectionProvider {
            sprites: sprite_set,
        })
    }
}
