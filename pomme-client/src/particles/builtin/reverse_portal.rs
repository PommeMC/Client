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
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors(exclude(
    on_ground,
    stopped_by_collision,
    gravity,
    friction,
    speed_up_when_y_motion_is_blocked,
))]
#[pomme_particle_macros::quad_accessors]
pub struct ReversePortalParticle {}

impl ReversePortalParticle {
    fn new(position: DVec3, auxiliary: DVec3, sprite: AtlasRegion) -> Self {
        // Separate from sprite selection: preserve draw purposes, not Java seeds.
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let _base_quad_size = randomized_quad_size(&mut random);
        let mut quad_size = 0.1_f32 * (random.f32() * 0.2_f32 + 0.5_f32);
        let brightness = random.f32() * 0.6_f32 + 0.4_f32;
        let _portal_lifetime = (random.f32() * 10.0_f32) as u32 + 40;
        quad_size *= 1.5_f32;
        let lifetime = (random.f32() * 2.0_f32) as u32 + 60;
        Self {
            position,
            previous_position: position,
            velocity: auxiliary,
            bounds: bounds_at(position, 0.2, 0.2),
            width: 0.2,
            height: 0.2,
            removed: false,
            has_physics: true,
            age: 0,
            lifetime,
            quad_size,
            color: [brightness * 0.9, brightness * 0.3, brightness],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
        }
    }
}

impl Particle for ReversePortalParticle {
    crate::particles::accessors::forward_particle_accessors!(ReversePortalParticle);

    fn as_single_quad(&self) -> Option<&dyn SingleQuadParticle> {
        Some(self)
    }

    fn tick(&mut self, _context: &mut LevelContext<'_>) {
        self.previous_position = self.position;
        let age = self.age;
        self.age += 1;
        if age >= self.lifetime {
            self.remove();
        } else {
            let speed = self.age as f32 / self.lifetime as f32;
            // Vanilla assigns coordinates directly, leaving bounds unchanged.
            self.position += self.velocity * f64::from(speed);
        }
    }

    fn move_by(&mut self, _world: &LevelContext<'_>, displacement: DVec3) {
        move_without_collision(&mut self.bounds, &mut self.position, displacement);
    }

    fn get_group(&self) -> ParticleRenderType {
        ParticleRenderType::SingleQuads
    }

    fn get_particle_limit(&self) -> Option<ParticleLimit> {
        None
    }

    fn set_pos(&mut self, position: DVec3) {
        self.position = position;
        self.bounds = bounds_at(position, self.width, self.height);
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

    fn get_light_coords(&self, world: &LevelContext<'_>, _partial_tick: f32) -> ParticleLight {
        let base = world.get_light_coords(self.position);
        let mut brightness = self.age as f32 / self.lifetime as f32;
        brightness *= brightness;
        brightness *= brightness;
        let emitted = (brightness.clamp(0.0, 1.0) * 240.0) as u16;
        ParticleLight {
            block: (u16::from(base.block) + emitted).min(240) as u8,
            sky: base.sky,
        }
    }
}

impl SingleQuadParticle for ReversePortalParticle {
    crate::particles::accessors::forward_quad_accessors!(ReversePortalParticle);

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

    fn get_facing(&self) -> ParticleFacing {
        ParticleFacing::Camera
    }

    fn get_quad_size(&self, partial_tick: f32) -> f32 {
        let s = 1.0_f32 - (self.age as f32 + partial_tick) / (self.lifetime as f32 * 1.5_f32);
        self.quad_size * s
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

pub struct ReversePortalProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ReversePortalProvider {
    pub fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<SimpleParticleType> for ReversePortalProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        Some(Rc::new(RefCell::new(ReversePortalParticle::new(
            position, auxiliary, sprite,
        ))))
    }
}

pub(in crate::particles) struct ReversePortalParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for ReversePortalParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(ReversePortalProvider::new(sprite_set))
    }
}
