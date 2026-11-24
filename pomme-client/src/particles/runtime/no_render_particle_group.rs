//! Ordinary simulation and admission without render extraction.

use super::super::particle::ParticleRef;
use super::super::render::{ParticleRenderType, ParticleView, QuadParticleRenderState};
use super::particle_group::{self, ParticleGroup};
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[derive(Default)]
pub struct NoRenderParticleGroup {
    particles: Vec<ParticleRef>,
}

impl ParticleGroup for NoRenderParticleGroup {
    type RenderState = Vec<QuadParticleRenderState>;

    fn get_render_type(&self) -> ParticleRenderType {
        ParticleRenderType::NoRender
    }

    fn tick_particles(
        &mut self,
        context: &mut LevelContext<'_>,
        on_removed: &mut dyn FnMut(&ParticleRef),
    ) {
        particle_group::tick_particles(&mut self.particles, context, on_removed);
    }

    fn add(&mut self, particle: ParticleRef, random: &RandomSource) -> bool {
        particle_group::add(&mut self.particles, particle, random)
    }

    fn len(&self) -> usize {
        self.particles.len()
    }

    fn extract_render_state(
        &self,
        _world: &LevelContext<'_>,
        _view: &ParticleView<'_>,
    ) -> Self::RenderState {
        Vec::new()
    }
}
