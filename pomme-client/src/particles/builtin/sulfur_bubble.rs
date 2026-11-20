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
pub struct SulfurBubbleParticle {
    random: fastrand::Rng,
    y_start: f64,
    y_end: f64,
    y_prev: f64,
    size_start: f32,
}

impl SulfurBubbleParticle {
    fn new(position: DVec3, xa: f64, za: f64, sprite: AtlasRegion) -> Self {
        // COMPAT: Own persistent stream and source draw order, not Java seed parity.
        let mut random = fastrand::Rng::new();
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let _base_quad_size = randomized_quad_size(&mut random);
        let bounds = resized_bounds(bounds_at(position, 0.2, 0.2), 0.02, 0.02);
        let velocity = DVec3::new(
            xa * f64::from(0.2_f32) + f64::from((random.f32() * 2.0_f32 - 1.0_f32) * 0.02_f32),
            0.0,
            za * f64::from(0.2_f32) + f64::from((random.f32() * 2.0_f32 - 1.0_f32) * 0.02_f32),
        );
        let size_start = 0.02_f32 + 0.02_f32 * random.f32();
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
            lifetime: i32::MAX as u32,
            quad_size: size_start,
            color: [1.0; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprite,
            random,
            y_start: position.y,
            y_end: position.y + 4.0 - 1.0,
            y_prev: position.y,
            size_start,
        }
    }

    fn random_horizontal_wiggling(&mut self) -> f64 {
        let magnitude = self.random.f32() * 0.003_f32;
        let sign = if self.random.bool() {
            1.0_f32
        } else {
            -1.0_f32
        };
        f64::from(magnitude * sign) * 0.5_f64
    }
}

impl Particle for SulfurBubbleParticle {
    crate::particles::accessors::forward_particle_accessors!(SulfurBubbleParticle);

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
            self.velocity.y -= 0.04_f64 * f64::from(-0.04_f32);
            self.move_by(context, self.velocity);
            self.velocity *= f64::from(0.85_f32);
            if self.on_ground {
                self.velocity.x *= f64::from(0.7_f32);
                self.velocity.z *= f64::from(0.7_f32);
            }
        }
        if !self.removed {
            let fluid = context
                .block_state_table()
                .block_state_data(context.get_block_state(BlockPos::containing(self.position)))
                .fluid();
            if fluid.kind != FluidKind::Water || fluid.amount != 8 || fluid.falling {
                self.remove();
            }
        }
        if !self.removed && self.position.y >= self.y_end {
            self.remove();
        }
        if !self.removed && self.position.y <= self.y_prev {
            self.remove();
        }
        self.velocity.x += self.random_horizontal_wiggling();
        self.velocity.z += self.random_horizontal_wiggling();
        self.move_by(context, DVec3::new(self.velocity.x, 0.0, self.velocity.z));
        let travel_progress =
            ((self.position.y - self.y_start) / (self.y_end - self.y_start)) as f32;
        self.quad_size = self.size_start + travel_progress * (0.15_f32 - self.size_start);
        self.y_prev = self.position.y;
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

impl SingleQuadParticle for SulfurBubbleParticle {
    crate::particles::accessors::forward_quad_accessors!(SulfurBubbleParticle);

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

pub struct SulfurBubbleProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl ParticleProvider<SimpleParticleType> for SulfurBubbleProvider {
    fn create_particle(
        &self,
        _options: &SimpleParticleType,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef> {
        let sprite = self.sprites.get_random(random);
        // Vanilla passes yAux as za and ignores zAux.
        Some(Rc::new(RefCell::new(SulfurBubbleParticle::new(
            position,
            auxiliary.x,
            auxiliary.y,
            sprite,
        ))))
    }
}

pub(in crate::particles) struct SulfurBubbleParticleRegistration;

impl SpriteParticleRegistration<SimpleParticleType> for SulfurBubbleParticleRegistration {
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<SimpleParticleType>> {
        Rc::new(SulfurBubbleProvider {
            sprites: sprite_set,
        })
    }
}
