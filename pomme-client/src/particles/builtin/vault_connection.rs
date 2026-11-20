use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{
    Particle, ParticleRef, bounds_at, move_without_collision, set_particle_size,
};
use super::super::particle_provider::ParticleProvider;
use super::super::render::{
    ParticleLayer, ParticleLight, ParticleRenderType, ParticleView, QuadParticleRenderState,
};
use super::super::resources::particle_resources::SpriteParticleRegistration;
use super::super::resources::sprite_set::SpriteSet;
use super::super::single_quad_particle::{
    SingleQuadParticle, extract_camera_quad, randomized_quad_size,
};
use super::super::types::SimpleParticleType;
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
#[pomme_particle_macros::quad_accessors(exclude(alpha,))]
pub struct FlyTowardsPositionParticle {
    anchor: DVec3,
    alpha: Cell<f32>,
}

impl FlyTowardsPositionParticle {
    fn new(anchor: DVec3, auxiliary: DVec3, sprite: AtlasRegion) -> Self {
        let bounds = bounds_at(anchor, 0.2, 0.2);
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let _base_quad_size = randomized_quad_size(&mut random);
        // Source assigns coordinates directly; the box remains at the anchor.
        let position = anchor + auxiliary;
        let quad_size = 0.1_f32 * (random.f32() * 0.5_f32 + 0.2_f32);
        let brightness = random.f32() * 0.6_f32 + 0.4_f32;
        let lifetime = (random.f32() * 10.0_f32) as u32 + 30;
        Self {
            position,
            previous_position: position,
            anchor,
            velocity: auxiliary,
            bounds,
            width: 0.2,
            height: 0.2,
            removed: false,
            has_physics: false,
            age: 0,
            lifetime,
            quad_size,
            color: [0.9 * brightness, 0.9 * brightness, brightness],
            alpha: Cell::new(0.0),
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
        }
    }
}

impl Particle for FlyTowardsPositionParticle {
    crate::particles::accessors::forward_particle_accessors!(FlyTowardsPositionParticle);

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
            let pos = 1.0_f32 - self.age as f32 / self.lifetime as f32;
            let mut pp = 1.0_f32 - pos;
            pp *= pp;
            pp *= pp;
            self.position = DVec3::new(
                self.anchor.x + self.velocity.x * f64::from(pos),
                self.anchor.y + self.velocity.y * f64::from(pos) - f64::from(pp * 1.2_f32),
                self.anchor.z + self.velocity.z * f64::from(pos),
            );
        }
    }

    fn move_by(&mut self, _world: &LevelContext<'_>, displacement: DVec3) {
        move_without_collision(&mut self.bounds, &mut self.position, displacement);
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
    fn get_light_coords(&self, world: &LevelContext<'_>, _partial_tick: f32) -> ParticleLight {
        let base = world.get_light_coords(self.position);
        ParticleLight {
            block: 240,
            sky: base.sky,
        }
    }
}

impl SingleQuadParticle for FlyTowardsPositionParticle {
    crate::particles::accessors::forward_quad_accessors!(FlyTowardsPositionParticle);

    fn set_sprite_from_age(&mut self, sprites: &dyn SpriteSet) {
        if !self.removed {
            self.sprite = sprites.get(self.age, self.lifetime);
        }
    }
    fn get_alpha(&self) -> f32 {
        self.alpha.get()
    }
    fn set_alpha(&mut self, alpha: f32) {
        self.alpha.set(alpha);
    }
    fn get_quad_size(&self, _partial_tick: f32) -> f32 {
        self.quad_size
    }
    fn get_layer(&self) -> ParticleLayer {
        ParticleLayer::Translucent
    }
    fn extract(
        &self,
        world: &LevelContext<'_>,
        view: &ParticleView<'_>,
        output: &mut Vec<QuadParticleRenderState>,
    ) {
        let time = ((self.age as f32 + view.partial_tick) / self.lifetime as f32 - 0.25_f32)
            / (1.0_f32 - 0.25_f32);
        self.alpha.set(if time < 0.0 {
            0.0
        } else if time > 1.0 {
            0.6
        } else {
            0.0_f32 + time * (0.6_f32 - 0.0_f32)
        });
        let index = output.len();
        extract_camera_quad(self, self.previous_roll, world, view, output);
        if let Some(quad) = output.get_mut(index) {
            // Source ARGB floors channels; the existing backend rounds submitted floats.
            quad.color = quad
                .color
                .map(|channel| (channel * 255.0_f32).floor() / 255.0_f32);
        }
    }
}

pub struct VaultConnectionProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for VaultConnectionProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        let mut particle = FlyTowardsPositionParticle::new(position, auxiliary, sprite);
        particle.scale(1.5);
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct VaultConnectionParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for VaultConnectionParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(VaultConnectionProvider {
            sprites: sprite_set,
        })
    }
}
