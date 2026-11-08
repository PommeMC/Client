//! Quad instance contracts and shared camera-facing extraction calculations.

use glam::Quat;

use super::particle::Particle;
use super::render::{ParticleFacing, ParticleLayer, ParticleView, QuadParticleRenderState};
use super::resources::sprite_set::SpriteSet;
use crate::renderer::chunk::atlas::AtlasRegion;
use crate::world::level::LevelContext;

/// Only quad instances implement this; non-quad groups are not forced through
/// sprite rendering. Base defaults and concrete tick/physics behavior are
/// deferred.
pub trait SingleQuadParticle: Particle {
    fn get_sprite(&self) -> AtlasRegion;
    fn set_sprite(&mut self, sprite: AtlasRegion);
    /// Explicit opt-in by concrete tick code; not automatic base ticking.
    fn set_sprite_from_age(&mut self, sprites: &dyn SpriteSet);
    fn get_uv(&self) -> [f32; 4];
    fn get_color(&self) -> [f32; 3];
    fn get_render_color(&self, _partial_tick: f32) -> [f32; 3] {
        self.get_color()
    }
    fn set_color(&mut self, color: [f32; 3]);
    fn get_alpha(&self) -> f32;
    fn set_alpha(&mut self, alpha: f32);
    fn get_roll(&self) -> f32;
    fn set_roll(&mut self, roll: f32);
    fn get_facing(&self) -> ParticleFacing;
    fn get_quad_size(&self, partial_tick: f32) -> f32;
    fn get_layer(&self) -> ParticleLayer;
    /// Read-only simulation access; append zero or more owned quad entries to
    /// the group's frame output, preserving existing entries. A delayed Shriek
    /// emits none; an active Shriek or Vibration emits two. This is extraction,
    /// not render work, and does not split the simulated particle's identity.
    fn extract(
        &self,
        world: &LevelContext<'_>,
        view: &ParticleView<'_>,
        output: &mut Vec<QuadParticleRenderState>,
    );
}

pub(super) fn randomized_quad_size(random: &mut fastrand::Rng) -> f32 {
    0.1 * (random.f32() * 0.5 + 0.5) * 2.0
}

pub(super) fn extract_camera_quad(
    particle: &dyn SingleQuadParticle,
    previous_roll: f32,
    world: &LevelContext<'_>,
    view: &ParticleView<'_>,
    output: &mut Vec<QuadParticleRenderState>,
) {
    let mut rotation = view.rotation;
    let roll = particle.get_roll();
    if roll != 0.0 {
        let roll = previous_roll + view.partial_tick * (roll - previous_roll);
        rotation *= Quat::from_rotation_z(roll);
    }
    let previous_position = particle.get_previous_pos();
    let position = previous_position
        + (particle.get_pos() - previous_position) * f64::from(view.partial_tick)
        - view.position;
    output.push(QuadParticleRenderState {
        position: position.as_vec3(),
        rotation,
        size: particle.get_quad_size(view.partial_tick),
        uv: particle.get_uv(),
        color: {
            let color = particle.get_render_color(view.partial_tick);
            [color[0], color[1], color[2], particle.get_alpha()]
        },
        light: particle.get_light_coords(world, view.partial_tick),
        layer: particle.get_layer(),
    });
}
