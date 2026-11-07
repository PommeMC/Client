use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

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
use super::super::types::{ParticleLimit, ParticleType, SimpleParticleType};
use crate::physics::aabb::Aabb;
use crate::position::BlockPos;
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::util::random_source::RandomSource;
use crate::world::block::FluidKind;
use crate::world::level::LevelContext;

pub struct DripHangParticle {
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
    sprite: AtlasRegion,
    falling_particle: SimpleParticleType,
    fluid_kind: FluidKind,
    cooling: bool,
    gravity: f32,
    is_glowing: bool,
}

impl DripHangParticle {
    fn new(
        position: DVec3,
        sprite: AtlasRegion,
        falling_particle: SimpleParticleType,
        fluid_kind: FluidKind,
        color: [f32; 3],
        cooling: bool,
    ) -> Self {
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let quad_size = randomized_quad_size(&mut random);
        Self {
            position,
            previous_position: position,
            velocity: DVec3::ZERO,
            bounds: resized_bounds(bounds_at(position, 0.2_f32, 0.2_f32), 0.01_f32, 0.01_f32),
            width: 0.01,
            height: 0.01,
            removed: false,
            on_ground: false,
            stopped_by_collision: false,
            has_physics: true,
            age: 0,
            lifetime: 40,
            quad_size,
            color,
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
            falling_particle,
            fluid_kind,
            cooling,
            gravity: 0.06_f32 * 0.02_f32,
            is_glowing: false,
        }
    }
}

impl Particle for DripHangParticle {
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
        if self.removed {
            return;
        }
        self.previous_position = self.position;
        if self.cooling {
            let elapsed = 40_i64 - i64::from(self.lifetime);
            self.color = [
                1.0,
                16.0_f32 / (elapsed + 16) as f32,
                4.0_f32 / (elapsed + 8) as f32,
            ];
        }
        if self.lifetime == 0 {
            self.remove();
            context.add_particle(&self.falling_particle, self.position, self.velocity);
            return;
        }
        self.lifetime -= 1;
        self.velocity.y -= f64::from(self.gravity);
        self.move_by(context, self.velocity);
        self.velocity *= 0.02_f64;
        self.velocity *= f64::from(0.98_f32);
        if self.fluid_kind != FluidKind::Empty
            && touches_source_fluid(context, self.position, self.fluid_kind)
        {
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
        self.set_size(0.2_f32 * factor, 0.2_f32 * factor);
    }
    fn has_physics(&self) -> bool {
        self.has_physics
    }
    fn set_has_physics(&mut self, enabled: bool) {
        self.has_physics = enabled;
    }
    fn get_light_coords(&self, world: &LevelContext<'_>, _partial_tick: f32) -> ParticleLight {
        let mut light = world.get_light_coords(self.position);
        if self.is_glowing {
            light.block = 240;
        }
        light
    }
}

impl SingleQuadParticle for DripHangParticle {
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

pub struct FallAndLandParticle {
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
    sprite: AtlasRegion,
    land_particle: SimpleParticleType,
    fluid_kind: FluidKind,
    gravity: f32,
    is_glowing: bool,
}

impl FallAndLandParticle {
    fn new(
        position: DVec3,
        sprite: AtlasRegion,
        land_particle: SimpleParticleType,
        fluid_kind: FluidKind,
        color: [f32; 3],
    ) -> Self {
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let quad_size = randomized_quad_size(&mut random);
        let lifetime = (64.0_f64 / (f64::from(random.f32()) * 0.8_f64 + 0.2_f64)) as u32;
        Self {
            position,
            previous_position: position,
            velocity: DVec3::ZERO,
            bounds: resized_bounds(bounds_at(position, 0.2_f32, 0.2_f32), 0.01_f32, 0.01_f32),
            width: 0.01,
            height: 0.01,
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
            sprite,
            land_particle,
            fluid_kind,
            gravity: 0.06_f32,
            is_glowing: false,
        }
    }
}

impl Particle for FallAndLandParticle {
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
        if self.on_ground {
            self.remove();
            context.add_particle(&self.land_particle, self.position, DVec3::ZERO);
            return;
        }
        self.velocity *= f64::from(0.98_f32);
        if self.fluid_kind != FluidKind::Empty
            && touches_source_fluid(context, self.position, self.fluid_kind)
        {
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
        self.set_size(0.2_f32 * factor, 0.2_f32 * factor);
    }
    fn has_physics(&self) -> bool {
        self.has_physics
    }
    fn set_has_physics(&mut self, enabled: bool) {
        self.has_physics = enabled;
    }
    fn get_light_coords(&self, world: &LevelContext<'_>, _partial_tick: f32) -> ParticleLight {
        let mut light = world.get_light_coords(self.position);
        if self.is_glowing {
            light.block = 240;
        }
        light
    }
}

impl SingleQuadParticle for FallAndLandParticle {
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

pub struct DripLandParticle {
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
    sprite: AtlasRegion,
    fluid_kind: FluidKind,
    is_glowing: bool,
}

impl DripLandParticle {
    fn new(position: DVec3, sprite: AtlasRegion) -> Self {
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let quad_size = randomized_quad_size(&mut random);
        let lifetime = (16.0_f64 / (f64::from(random.f32()) * 0.8_f64 + 0.2_f64)) as u32;
        Self {
            position,
            previous_position: position,
            velocity: DVec3::ZERO,
            bounds: resized_bounds(bounds_at(position, 0.2_f32, 0.2_f32), 0.01_f32, 0.01_f32),
            width: 0.01,
            height: 0.01,
            removed: false,
            on_ground: false,
            stopped_by_collision: false,
            has_physics: true,
            age: 0,
            lifetime,
            quad_size,
            color: [1.0, 0.2857143, 0.083333336],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
            fluid_kind: FluidKind::Lava,
            is_glowing: false,
        }
    }
}

impl Particle for DripLandParticle {
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
        if self.removed {
            return;
        }
        self.previous_position = self.position;
        if self.lifetime == 0 {
            self.remove();
            return;
        }
        self.lifetime -= 1;
        self.velocity.y -= f64::from(0.06_f32);
        self.move_by(context, self.velocity);
        self.velocity *= f64::from(0.98_f32);
        if self.fluid_kind != FluidKind::Empty
            && touches_source_fluid(context, self.position, self.fluid_kind)
        {
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
        self.set_size(0.2_f32 * factor, 0.2_f32 * factor);
    }
    fn has_physics(&self) -> bool {
        self.has_physics
    }
    fn set_has_physics(&mut self, enabled: bool) {
        self.has_physics = enabled;
    }
    fn get_light_coords(&self, world: &LevelContext<'_>, _partial_tick: f32) -> ParticleLight {
        let mut light = world.get_light_coords(self.position);
        if self.is_glowing {
            light.block = 240;
        }
        light
    }
}

impl SingleQuadParticle for DripLandParticle {
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

fn touches_source_fluid(
    context: &LevelContext<'_>,
    position: DVec3,
    fluid_kind: FluidKind,
) -> bool {
    let pos = BlockPos::containing(position);
    let fluid = context
        .block_state_table()
        .block_state_data(context.get_block_state(pos))
        .fluid();
    fluid.kind == fluid_kind
        && fluid.amount == 8
        && !fluid.falling
        && position.y < f64::from(pos.y) + context.get_fluid_height(pos)
}

pub struct WaterHangProvider {
    sprites: Rc<dyn SpriteSet>,
    falling_water: Rc<ParticleType<SimpleParticleType>>,
}

impl ParticleProvider<SimpleParticleType> for WaterHangProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(DripHangParticle::new(
            position,
            sprite,
            SimpleParticleType::new(Rc::clone(&self.falling_water)),
            FluidKind::Water,
            [0.2, 0.3, 1.0],
            false,
        ))))
    }
}

pub(in crate::particles) struct WaterHangParticleRegistration {
    falling_water: Rc<ParticleType<SimpleParticleType>>,
}

impl WaterHangParticleRegistration {
    pub(in crate::particles) fn new(falling_water: Rc<ParticleType<SimpleParticleType>>) -> Self {
        Self { falling_water }
    }
}

impl SpriteParticleRegistration<SimpleParticleType> for WaterHangParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(WaterHangProvider {
            sprites: sprite_set,
            falling_water: Rc::clone(&self.falling_water),
        })
    }
}

pub struct WaterFallProvider {
    sprites: Rc<dyn SpriteSet>,
    splash: Rc<ParticleType<SimpleParticleType>>,
}

impl ParticleProvider<SimpleParticleType> for WaterFallProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(FallAndLandParticle::new(
            position,
            sprite,
            SimpleParticleType::new(Rc::clone(&self.splash)),
            FluidKind::Water,
            [0.2, 0.3, 1.0],
        ))))
    }
}

pub(in crate::particles) struct WaterFallParticleRegistration {
    splash: Rc<ParticleType<SimpleParticleType>>,
}

impl WaterFallParticleRegistration {
    pub(in crate::particles) fn new(splash: Rc<ParticleType<SimpleParticleType>>) -> Self {
        Self { splash }
    }
}

impl SpriteParticleRegistration<SimpleParticleType> for WaterFallParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(WaterFallProvider {
            sprites: sprite_set,
            splash: Rc::clone(&self.splash),
        })
    }
}

pub struct LavaHangProvider {
    sprites: Rc<dyn SpriteSet>,
    falling_lava: Rc<ParticleType<SimpleParticleType>>,
}

impl ParticleProvider<SimpleParticleType> for LavaHangProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(DripHangParticle::new(
            position,
            sprite,
            SimpleParticleType::new(Rc::clone(&self.falling_lava)),
            FluidKind::Lava,
            [1.0, 1.0, 1.0],
            true,
        ))))
    }
}

pub(in crate::particles) struct LavaHangParticleRegistration {
    falling_lava: Rc<ParticleType<SimpleParticleType>>,
}

impl LavaHangParticleRegistration {
    pub(in crate::particles) fn new(falling_lava: Rc<ParticleType<SimpleParticleType>>) -> Self {
        Self { falling_lava }
    }
}

impl SpriteParticleRegistration<SimpleParticleType> for LavaHangParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(LavaHangProvider {
            sprites: sprite_set,
            falling_lava: Rc::clone(&self.falling_lava),
        })
    }
}

pub struct LavaFallProvider {
    sprites: Rc<dyn SpriteSet>,
    landing_lava: Rc<ParticleType<SimpleParticleType>>,
}

impl ParticleProvider<SimpleParticleType> for LavaFallProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(FallAndLandParticle::new(
            position,
            sprite,
            SimpleParticleType::new(Rc::clone(&self.landing_lava)),
            FluidKind::Lava,
            [1.0, 0.2857143, 0.083333336],
        ))))
    }
}

pub(in crate::particles) struct LavaFallParticleRegistration {
    landing_lava: Rc<ParticleType<SimpleParticleType>>,
}

impl LavaFallParticleRegistration {
    pub(in crate::particles) fn new(landing_lava: Rc<ParticleType<SimpleParticleType>>) -> Self {
        Self { landing_lava }
    }
}

impl SpriteParticleRegistration<SimpleParticleType> for LavaFallParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(LavaFallProvider {
            sprites: sprite_set,
            landing_lava: Rc::clone(&self.landing_lava),
        })
    }
}

pub struct LavaLandProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for LavaLandProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(DripLandParticle::new(
            position, sprite,
        ))))
    }
}

pub(in crate::particles) struct LavaLandParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for LavaLandParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(LavaLandProvider {
            sprites: sprite_set,
        })
    }
}

pub struct ObsidianTearHangProvider {
    sprites: Rc<dyn SpriteSet>,
    falling_obsidian_tear: Rc<ParticleType<SimpleParticleType>>,
}

impl ParticleProvider<SimpleParticleType> for ObsidianTearHangProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        let mut particle = DripHangParticle::new(
            position,
            sprite,
            SimpleParticleType::new(Rc::clone(&self.falling_obsidian_tear)),
            FluidKind::Empty,
            [0.51171875, 0.03125, 0.890625],
            false,
        );
        particle.is_glowing = true;
        particle.gravity *= 0.01_f32;
        particle.lifetime = 100;
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct ObsidianTearHangParticleRegistration {
    falling_obsidian_tear: Rc<ParticleType<SimpleParticleType>>,
}

impl ObsidianTearHangParticleRegistration {
    pub(in crate::particles) fn new(
        falling_obsidian_tear: Rc<ParticleType<SimpleParticleType>>,
    ) -> Self {
        Self {
            falling_obsidian_tear,
        }
    }
}

impl SpriteParticleRegistration<SimpleParticleType> for ObsidianTearHangParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(ObsidianTearHangProvider {
            sprites: sprite_set,
            falling_obsidian_tear: Rc::clone(&self.falling_obsidian_tear),
        })
    }
}

pub struct ObsidianTearFallProvider {
    sprites: Rc<dyn SpriteSet>,
    landing_obsidian_tear: Rc<ParticleType<SimpleParticleType>>,
}

impl ParticleProvider<SimpleParticleType> for ObsidianTearFallProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        let mut particle = FallAndLandParticle::new(
            position,
            sprite,
            SimpleParticleType::new(Rc::clone(&self.landing_obsidian_tear)),
            FluidKind::Empty,
            [0.51171875, 0.03125, 0.890625],
        );
        particle.is_glowing = true;
        particle.gravity = 0.01_f32;
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct ObsidianTearFallParticleRegistration {
    landing_obsidian_tear: Rc<ParticleType<SimpleParticleType>>,
}

impl ObsidianTearFallParticleRegistration {
    pub(in crate::particles) fn new(
        landing_obsidian_tear: Rc<ParticleType<SimpleParticleType>>,
    ) -> Self {
        Self {
            landing_obsidian_tear,
        }
    }
}

impl SpriteParticleRegistration<SimpleParticleType> for ObsidianTearFallParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(ObsidianTearFallProvider {
            sprites: sprite_set,
            landing_obsidian_tear: Rc::clone(&self.landing_obsidian_tear),
        })
    }
}

pub struct ObsidianTearLandProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for ObsidianTearLandProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        let mut particle = DripLandParticle::new(position, sprite);
        particle.fluid_kind = FluidKind::Empty;
        particle.is_glowing = true;
        particle.lifetime =
            (28.0_f64 / (f64::from(random.next_float()) * 0.8_f64 + 0.2_f64)) as u32;
        particle.color = [0.51171875, 0.03125, 0.890625];
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct ObsidianTearLandParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for ObsidianTearLandParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(ObsidianTearLandProvider {
            sprites: sprite_set,
        })
    }
}
