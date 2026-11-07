use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::block_particle_option::BlockParticleOption;
use super::super::particle::{Particle, ParticleRef, bounds_at, randomized_motion, resized_bounds};
use super::super::particle_provider::ParticleProvider;
use super::super::render::{
    ParticleFacing, ParticleLayer, ParticleLight, ParticleRenderType, ParticleView,
    QuadParticleRenderState,
};
use super::super::resources::sprite_set::SpriteSet;
use super::super::single_quad_particle::{
    SingleQuadParticle, extract_camera_quad, randomized_quad_size,
};
use super::super::types::ParticleLimit;
use crate::physics::aabb::Aabb;
use crate::position::BlockPos;
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

pub(crate) struct TerrainParticle {
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
    sprite: AtlasRegion,
    uo: f32,
    vo: f32,
    layer: ParticleLayer,
}

impl TerrainParticle {
    pub(crate) fn new(
        position: DVec3,
        auxiliary: DVec3,
        sprite: AtlasRegion,
        tint: [f32; 3],
    ) -> Self {
        let mut random = fastrand::Rng::new();
        let bounds = bounds_at(position, 0.2, 0.2);
        let lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let velocity = randomized_motion(&mut random, auxiliary);
        let mut quad_size = randomized_quad_size(&mut random);
        let mut color = [0.6_f32; 3];
        for (c, t) in color.iter_mut().zip(tint) {
            *c *= t;
        }
        quad_size *= 0.5;
        let uo = random.f32() * 3.0;
        let vo = random.f32() * 3.0;
        let layer = if sprite.translucent {
            ParticleLayer::BlockTranslucent
        } else {
            ParticleLayer::BlockOpaque
        };
        Self {
            position,
            previous_position: position,
            velocity,
            bounds,
            width: 0.2,
            height: 0.2,
            removed: false,
            on_ground: false,
            stopped_by_collision: false,
            has_physics: true,
            age: 0,
            lifetime,
            gravity: 1.0,
            friction: 0.98,
            quad_size,
            color,
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
            uo,
            vo,
            layer,
        }
    }
}

impl Particle for TerrainParticle {
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
            self.velocity *= f64::from(self.friction);
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
        world.get_light_coords(self.position)
    }
}

impl SingleQuadParticle for TerrainParticle {
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
        let sprite_u = |f: f32| self.sprite.u_min + f * (self.sprite.u_max - self.sprite.u_min);
        let sprite_v = |f: f32| self.sprite.v_min + f * (self.sprite.v_max - self.sprite.v_min);
        [
            sprite_u((self.uo + 1.0) / 4.0),
            sprite_u(self.uo / 4.0),
            sprite_v(self.vo / 4.0),
            sprite_v((self.vo + 1.0) / 4.0),
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
        self.layer
    }

    fn extract(
        &self,
        world: &LevelContext<'_>,
        view: &ParticleView<'_>,
        output: &mut Vec<QuadParticleRenderState>,
    ) {
        if !self.removed {
            extract_camera_quad(self, self.previous_roll, world, view, output);
        }
    }
}

pub(super) struct TerrainProvider;

impl ParticleProvider<BlockParticleOption> for TerrainProvider {
    fn create_particle(
        &self,
        options: &BlockParticleOption,
        world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        _random: &RandomSource,
    ) -> Option<ParticleRef> {
        let state = options.get_state();
        let data = world.block_state_table().block_state_data(state);
        if data.is_air() || matches!(data.id(), "moving_piston" | "barrier" | "structure_void") {
            return None;
        }
        let appearance = world.block_particle_appearance();
        let sprite = appearance.sprite(state);
        let tint = appearance.tint(state, BlockPos::containing(position), world.chunks());
        Some(Rc::new(RefCell::new(TerrainParticle::new(
            position, auxiliary, sprite, tint,
        ))))
    }
}
