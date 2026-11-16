use std::cell::RefCell;
use std::rc::Rc;

use glam::{DVec3, Vec3};

use super::super::dust_color_transition_options::DustColorTransitionOptions;
use super::super::particle::{Particle, ParticleRef, bounds_at, randomized_motion, set_particle_size};
use super::super::particle_provider::ParticleProvider;
use super::super::render::{ParticleLayer, ParticleRenderType, ParticleView, QuadParticleRenderState};
use super::super::resources::particle_resources::SpriteParticleRegistration;
use super::super::resources::sprite_set::SpriteSet;
use super::super::single_quad_particle::{
    SingleQuadParticle, extract_camera_quad, randomized_quad_size,
};
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[pomme_particle_macros::particle_accessors(exclude(
    gravity,
    friction,
    speed_up_when_y_motion_is_blocked,
))]
#[pomme_particle_macros::quad_accessors]
pub(crate) struct DustColorTransitionParticle {
    sprites: Rc<dyn SpriteSet>,
    from_color: Vec3,
    to_color: Vec3,
}

impl DustColorTransitionParticle {
    fn new(
        options: &DustColorTransitionOptions,
        position: DVec3,
        auxiliary: DVec3,
        sprites: Rc<dyn SpriteSet>,
    ) -> Self {
        let _initial_sprite = sprites.first();
        let mut random = fastrand::Rng::new();
        // The overwritten base lifetime still consumes the first constructor draw.
        let _base_lifetime = (4.0_f32 / (random.f32() * 0.9_f32 + 0.1_f32)) as u32;
        let mut velocity = randomized_motion(&mut random, auxiliary);
        let mut quad_size = randomized_quad_size(&mut random);
        velocity *= f64::from(0.1_f32);
        quad_size *= 0.75_f32 * options.get_scale();
        let base_lifetime = (8.0_f64 / (random.f64() * 0.8_f64 + 0.2_f64)) as u32;
        let lifetime = (base_lifetime as f32 * options.get_scale()).max(1.0_f32) as u32;
        let sprite = sprites.get(0, lifetime);
        let common = random.f32() * 0.4_f32 + 0.6_f32;
        let from_color = Self::randomize_color(&mut random, options.get_from_color(), common);
        let to_color = Self::randomize_color(&mut random, options.get_to_color(), common);
        Self {
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
            quad_size,
            color: [1.0; 3],
            alpha: 1.0,
            roll: 0.0,
            previous_roll: 0.0,
            sprites,
            sprite,
            from_color,
            to_color,
        }
    }

    fn randomize_color(random: &mut fastrand::Rng, color: Vec3, common: f32) -> Vec3 {
        Vec3::new(
            (random.f32() * 0.2_f32 + 0.8_f32) * color.x * common,
            (random.f32() * 0.2_f32 + 0.8_f32) * color.y * common,
            (random.f32() * 0.2_f32 + 0.8_f32) * color.z * common,
        )
    }
}

impl Particle for DustColorTransitionParticle {
    crate::particles::accessors::forward_particle_accessors!();

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
        self.set_size(0.2 * factor, 0.2 * factor);
    }
}

impl SingleQuadParticle for DustColorTransitionParticle {
    crate::particles::accessors::forward_quad_accessors!();

    fn set_sprite_from_age(&mut self, sprites: &dyn SpriteSet) {
        if !self.removed {
            self.sprite = sprites.get(self.age, self.lifetime);
        }
    }

    fn get_render_color(&self, partial_tick: f32) -> [f32; 3] {
        let amount = (self.age as f32 + partial_tick) / (self.lifetime as f32 + 1.0_f32);
        // Vanilla mutates RGB during extraction; return the same frame color read-only.
        (self.from_color + (self.to_color - self.from_color) * amount).to_array()
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
        extract_camera_quad(self, self.previous_roll, world, view, output);
    }
}

pub(crate) struct DustColorTransitionProvider {
    sprites: Rc<dyn SpriteSet>,
}

impl DustColorTransitionProvider {
    fn new(sprites: Rc<dyn SpriteSet>) -> Self {
        Self { sprites }
    }
}

impl ParticleProvider<DustColorTransitionOptions> for DustColorTransitionProvider {
    fn create_particle(
        &self,
        options: &DustColorTransitionOptions,
        _world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        _random: &RandomSource,
    ) -> Option<ParticleRef> {
        Some(Rc::new(RefCell::new(DustColorTransitionParticle::new(
            options,
            position,
            auxiliary,
            Rc::clone(&self.sprites),
        ))))
    }
}

pub(in crate::particles) struct DustColorTransitionParticleRegistration;

impl SpriteParticleRegistration<DustColorTransitionOptions>
    for DustColorTransitionParticleRegistration
{
    fn create(
        &self,
        sprite_set: Rc<dyn SpriteSet>,
    ) -> Rc<dyn ParticleProvider<DustColorTransitionOptions>> {
        Rc::new(DustColorTransitionProvider::new(sprite_set))
    }
}
