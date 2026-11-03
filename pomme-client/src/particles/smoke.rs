//! Concrete Vanilla Smoke behavior, independent of engine/runtime adoption.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use glam::{DVec3, Quat};

use super::particle::{Particle, ParticleRef, ParticleTickContext, SingleQuadParticle};
use super::render::{
    ParticleFacing, ParticleLayer, ParticleLight, ParticleRenderType, ParticleView,
    QuadParticleRenderState,
};
use super::sprites::SpriteSet;
use super::types::{ParticleLimit, SimpleParticleType};
use super::world::ParticleWorld;
use super::{ParticleProvider, SpriteParticleRegistration};
use crate::physics::aabb::Aabb;
use crate::renderer::chunk::atlas::AtlasRegion;

pub struct SmokeParticle {
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
    quad_size: f32,
    color: [f32; 3],
    alpha: f32,
    roll: f32,
    previous_roll: f32,
    sprites: Rc<dyn SpriteSet>,
    sprite: AtlasRegion,
}

impl SmokeParticle {
    fn new(position: DVec3, auxiliary: DVec3, scale: f32, sprites: Rc<dyn SpriteSet>) -> Self {
        let sprite = sprites.first();
        // Vanilla Smoke owns its construction stream, ignoring the provider's
        // engine RNG. fastrand preserves draw purposes, not Java seed sequences.
        let mut random = fastrand::Rng::new();
        // The base lifetime is overwritten, but its draw still precedes motion.
        let _base_lifetime = (4.0 / (random.f32() * 0.9 + 0.1)) as u32;
        let mut velocity = DVec3::new(
            f64::from((random.f32() * 2.0 - 1.0) * 0.4),
            f64::from((random.f32() * 2.0 - 1.0) * 0.4),
            f64::from((random.f32() * 2.0 - 1.0) * 0.4),
        );
        let speed = f64::from((random.f32() + random.f32() + 1.0) * 0.15);
        // Java promotes these float constants after the f32 random arithmetic.
        velocity = velocity / velocity.length() * speed * f64::from(0.4_f32);
        velocity.y += f64::from(0.1_f32);
        let mut quad_size = 0.1 * (random.f32() * 0.5 + 0.5) * 2.0;
        velocity = velocity * f64::from(0.1_f32) + auxiliary;
        let color = [random.f32() * 0.3; 3];
        quad_size *= 0.75 * scale;
        // Accepted f32 adaptation: Java's final lifetime denominator uses doubles.
        let lifetime = ((8.0 / (random.f32() * 0.8 + 0.2) * scale) as u32).max(1);
        let mut particle = Self {
            position,
            previous_position: position,
            velocity,
            bounds: Self::bounds_at(position, 0.2, 0.2),
            width: 0.2,
            height: 0.2,
            removed: false,
            on_ground: false,
            stopped_by_collision: false,
            has_physics: true,
            age: 0,
            lifetime,
            quad_size,
            color,
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprites,
            sprite,
        };
        particle.sprite = particle.sprites.get(particle.age, particle.lifetime);
        particle
    }

    fn bounds_at(position: DVec3, width: f32, height: f32) -> Aabb {
        let half_width = f64::from(width / 2.0);
        Aabb::new(
            DVec3::new(position.x - half_width, position.y, position.z - half_width),
            DVec3::new(
                position.x + half_width,
                position.y + f64::from(height),
                position.z + half_width,
            ),
        )
    }
}

impl Particle for SmokeParticle {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_single_quad(&self) -> Option<&dyn SingleQuadParticle> {
        Some(self)
    }

    fn tick(&mut self, context: &mut dyn ParticleTickContext) {
        self.previous_position = self.position;
        let age = self.age;
        self.age += 1;
        if age >= self.lifetime {
            self.remove();
        } else {
            self.velocity.y -= 0.04 * f64::from(-0.1_f32);
            self.move_by(context, self.velocity);
            if self.position.y == self.previous_position.y {
                self.velocity.x *= 1.1;
                self.velocity.z *= 1.1;
            }
            self.velocity *= f64::from(0.96_f32);
            if self.on_ground {
                self.velocity.x *= f64::from(0.7_f32);
                self.velocity.z *= f64::from(0.7_f32);
            }
        }
        if !self.removed {
            self.sprite = self.sprites.get(self.age, self.lifetime);
        }
    }

    fn move_by(&mut self, world: &dyn ParticleWorld, displacement: DVec3) {
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
        self.bounds = Self::bounds_at(position, self.width, self.height);
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
            let min = DVec3::new(
                (self.bounds.min.x + self.bounds.max.x - f64::from(width)) / 2.0,
                self.bounds.min.y,
                (self.bounds.min.z + self.bounds.max.z - f64::from(width)) / 2.0,
            );
            self.bounds = Aabb::new(
                min,
                min + DVec3::new(f64::from(width), f64::from(height), f64::from(width)),
            );
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

    fn get_light_coords(&self, world: &dyn ParticleWorld, _partial_tick: f32) -> ParticleLight {
        world.get_light_coords(self.position)
    }
}

impl SingleQuadParticle for SmokeParticle {
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
        ParticleFacing::Camera
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
        world: &dyn ParticleWorld,
        view: &ParticleView<'_>,
        output: &mut Vec<QuadParticleRenderState>,
    ) {
        let mut rotation = view.rotation;
        if self.roll != 0.0 {
            let roll = self.previous_roll + view.partial_tick * (self.roll - self.previous_roll);
            rotation *= Quat::from_rotation_z(roll);
        }
        let position = self.previous_position
            + (self.position - self.previous_position) * f64::from(view.partial_tick)
            - view.position;
        output.push(QuadParticleRenderState {
            position: position.as_vec3(),
            rotation,
            size: self.get_quad_size(view.partial_tick),
            uv: self.get_uv(),
            color: [self.color[0], self.color[1], self.color[2], self.alpha],
            light: self.get_light_coords(world, view.partial_tick),
            layer: self.get_layer(),
        });
    }
}

/// Rust spelling of Vanilla `SmokeParticle.Provider`.
pub struct SmokeProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl SmokeProvider {
    pub fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<SimpleParticleType> for SmokeProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &dyn ParticleWorld,
        position: DVec3,
        auxiliary: DVec3,
        _random: &mut fastrand::Rng,
    ) -> Option<ParticleRef> {
        Some(Rc::new(RefCell::new(SmokeParticle::new(
            position,
            auxiliary,
            1.0,
            Rc::clone(&self.sprites),
        ))))
    }
}

pub(super) struct SmokeParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for SmokeParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(SmokeProvider::new(sprite_set))
    }
}
