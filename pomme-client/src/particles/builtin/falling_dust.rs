//! 26.3 FallingDustParticle, with its own position-only Particle simulation.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::block_particle_option::BlockParticleOption;
use super::super::particle::{Particle, ParticleRef, bounds_at, resized_bounds};
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
use super::super::types::ParticleLimit;
use crate::physics::aabb::Aabb;
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

pub struct FallingDustParticle {
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
    rot_speed: f32,
    sprite: AtlasRegion,
    sprites: Rc<dyn SpriteSet>,
}

impl FallingDustParticle {
    fn new(position: DVec3, color: [f32; 3], sprites: Rc<dyn SpriteSet>) -> Self {
        let sprite = sprites.first();
        // COMPAT: Instance fastrand preserves five OWN draw purposes/order,
        // not Java seeded sequences. No motion-constructor or ENGINE draws.
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let quad_size = randomized_quad_size(&mut random) * 0.67499995_f32;
        let base_lifetime = (32.0_f64 / (f64::from(random.f32()) * 0.8_f64 + 0.2_f64)) as u32;
        let lifetime = (base_lifetime as f32 * 0.9_f32).max(1.0_f32) as u32;
        let mut particle = Self {
            position,
            previous_position: position,
            velocity: DVec3::ZERO,
            bounds: bounds_at(position, 0.2, 0.2),
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
            rot_speed: 0.0,
            sprite,
            sprites: Rc::clone(&sprites),
        };
        particle.set_sprite_from_age(sprites.as_ref());
        particle.rot_speed = (random.f32() - 0.5_f32) * 0.1_f32;
        particle.roll = random.f32() * (std::f64::consts::PI * 2.0) as f32;
        particle
    }
}

impl Particle for FallingDustParticle {
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
            let sprites = Rc::clone(&self.sprites);
            self.set_sprite_from_age(sprites.as_ref());
            self.previous_roll = self.roll;
            self.roll += std::f32::consts::PI * self.rot_speed * 2.0_f32;
            if self.on_ground {
                self.previous_roll = 0.0;
                self.roll = 0.0;
            }
            self.move_by(context, self.velocity);
            self.velocity.y -= f64::from(0.003_f32);
            self.velocity.y = self.velocity.y.max(f64::from(-0.14_f32));
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

impl SingleQuadParticle for FallingDustParticle {
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
            * ((self.age as f32 + partial_tick) / self.lifetime as f32 * 32.0_f32).clamp(0.0, 1.0)
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

pub struct FallingDustProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<BlockParticleOption> for FallingDustProvider {
    fn create_particle(
        &self,
        options: &BlockParticleOption,
        world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        _random: &RandomSource,
    ) -> Option<ParticleRef> {
        let state = world
            .block_state_table()
            .block_state_data(options.get_state());
        if !state.is_air() && state.is_invisible() {
            return None;
        }
        // The source color boundary uses BlockPos::containing(position), flooring
        // even negative coordinates. This temporary palette is position-independent;
        // a real getDustColor/tint/map lookup must receive that floored position.
        let color = falling_dust_color(state.id());
        Some(Rc::new(RefCell::new(FallingDustParticle::new(
            position,
            color,
            Rc::clone(&self.sprites),
        ))))
    }
}

// COMPAT: Approved temporary 26.3 constants by selected-table name, not general
// tint/map-color parity or a verified all-version palette. Fixed getDustColor
// overrides win; powders use DyeColor -> MapColor, not dye texture RGB.
fn falling_dust_color(name: &str) -> [f32; 3] {
    let color: i32 = match name {
        "sand" => 14406560,
        "red_sand" => 11098145,
        "gravel" => -8356741,
        "dragon_egg" => -16777216,
        "anvil" | "chipped_anvil" | "damaged_anvil" => 10987431,
        "suspicious_sand" => 16247203,
        "suspicious_gravel" => 7368816,
        "white_concrete_powder" => 16777215,
        "orange_concrete_powder" => 14188339,
        "magenta_concrete_powder" => 11685080,
        "light_blue_concrete_powder" => 6724056,
        "yellow_concrete_powder" => 15066419,
        "lime_concrete_powder" => 8375321,
        "pink_concrete_powder" => 15892389,
        "gray_concrete_powder" => 5000268,
        "light_gray_concrete_powder" => 10066329,
        "cyan_concrete_powder" => 5013401,
        "purple_concrete_powder" => 8339378,
        "blue_concrete_powder" => 3361970,
        "brown_concrete_powder" => 6704179,
        "green_concrete_powder" => 6717235,
        "red_concrete_powder" => 10040115,
        "black_concrete_powder" => 1644825,
        _ => return [1.0; 3],
    };
    let rgb = color as u32;
    [
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
    ]
}

pub(in crate::particles) struct FallingDustParticleRegistration;

impl SpriteParticleRegistration<BlockParticleOption> for FallingDustParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<BlockParticleOption>> {
        Rc::new(FallingDustProvider {
            sprites: sprite_set,
        })
    }
}
