use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::{Particle, ParticleRef, bounds_at, resized_bounds, set_particle_size};
use super::super::particle_provider::ParticleProvider;
use super::super::render::{ParticleLayer, ParticleRenderType, ParticleView, QuadParticleRenderState};
use super::super::resources::particle_resources::SpriteParticleRegistration;
use super::super::resources::sprite_set::SpriteSet;
use super::super::single_quad_particle::{
    SingleQuadParticle, extract_quantized_camera_quad, randomized_quad_size,
};
use super::super::types::{ParticleType, SimpleParticleType};
use crate::position::BlockPos;
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::util::random_source::RandomSource;
use crate::world::block::FluidKind;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors(exclude(friction, speed_up_when_y_motion_is_blocked))]
#[pomme_particle_macros::quad_accessors]
pub struct DripstoneFallAndLandParticle {
    land_particle: SimpleParticleType,
    fluid_kind: FluidKind,
    random: fastrand::Rng,
}

impl DripstoneFallAndLandParticle {
    fn new(
        position: DVec3,
        sprite: AtlasRegion,
        land_particle: SimpleParticleType,
        fluid_kind: FluidKind,
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
            color: [1.0; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
            land_particle,
            fluid_kind,
            gravity: 0.06_f32,
            random,
        }
    }
}

impl Particle for DripstoneFallAndLandParticle {
    crate::particles::accessors::forward_particle_accessors!(DripstoneFallAndLandParticle);

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
            // COMPAT: playback is deferred, but Source's post-child OWN volume and
            // LEVEL sound seed draws remain. BLOCKS, pitch=1f, distanceDelay=false.
            let _sound = if self.fluid_kind == FluidKind::Lava {
                "minecraft:block.pointed_dripstone.drip_lava"
            } else {
                "minecraft:block.pointed_dripstone.drip_water"
            };
            let _volume = self.random.f32() * (1.0_f32 - 0.3_f32) + 0.3_f32;
            let _sound_seed = context.get_random().next_long();
            return;
        }
        if !self.removed {
            self.velocity *= f64::from(0.98_f32);
            let pos = BlockPos::containing(self.position);
            let fluid = context
                .block_state_table()
                .block_state_data(context.get_block_state(pos))
                .fluid();
            // FluidState.is compares WATER/LAVA identity, excluding FLOWING_*.
            if fluid.kind == self.fluid_kind && fluid.amount == 8 && !fluid.falling {
                let height = context.get_fluid_height(pos) as f32;
                let surface = f64::from(pos.y as f32 + height);
                if height > 0.0 && self.position.y < surface {
                    self.remove();
                }
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
        self.set_size(0.2_f32 * factor, 0.2_f32 * factor);
    }
}

impl SingleQuadParticle for DripstoneFallAndLandParticle {
    crate::particles::accessors::forward_quad_accessors!(DripstoneFallAndLandParticle);

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
        extract_quantized_camera_quad(self, self.previous_roll, world, view, output);
    }
}

pub struct DripstoneFallProvider {
    sprites: Rc<dyn SpriteSet>,
    land_particle: Rc<ParticleType<SimpleParticleType>>,
    fluid_kind: FluidKind,
}

impl ParticleProvider<SimpleParticleType> for DripstoneFallProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        _auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        let mut particle = DripstoneFallAndLandParticle::new(
            position,
            sprite,
            SimpleParticleType::new(Rc::clone(&self.land_particle)),
            self.fluid_kind,
        );
        particle.set_color(if self.fluid_kind == FluidKind::Lava {
            [1.0, 0.2857143, 0.083333336]
        } else {
            [0.2, 0.3, 1.0]
        });
        Some(Rc::new(RefCell::new(particle)))
    }
}

pub(in crate::particles) struct DripstoneFallParticleRegistration {
    land_particle: Rc<ParticleType<SimpleParticleType>>,
    fluid_kind: FluidKind,
}

impl DripstoneFallParticleRegistration {
    pub(in crate::particles) fn lava(land_particle: Rc<ParticleType<SimpleParticleType>>) -> Self {
        Self {
            land_particle,
            fluid_kind: FluidKind::Lava,
        }
    }

    pub(in crate::particles) fn water(land_particle: Rc<ParticleType<SimpleParticleType>>) -> Self {
        Self {
            land_particle,
            fluid_kind: FluidKind::Water,
        }
    }
}

impl SpriteParticleRegistration<SimpleParticleType> for DripstoneFallParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(DripstoneFallProvider {
            sprites: sprite_set,
            land_particle: Rc::clone(&self.land_particle),
            fluid_kind: self.fluid_kind,
        })
    }
}
