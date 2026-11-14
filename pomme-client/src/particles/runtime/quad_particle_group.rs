//! Single-quad simulation group with current-world-position frustum culling.

use super::super::particle::ParticleRef;
use super::super::render::{ParticleRenderType, ParticleView, QuadParticleRenderState};
use super::particle_group::{self, ParticleGroup};
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[derive(Default)]
pub struct QuadParticleGroup {
    particles: Vec<ParticleRef>,
}

impl ParticleGroup for QuadParticleGroup {
    type RenderState = Vec<QuadParticleRenderState>;

    fn get_render_type(&self) -> ParticleRenderType {
        ParticleRenderType::SingleQuads
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
        world: &LevelContext<'_>,
        view: &ParticleView<'_>,
    ) -> Self::RenderState {
        let mut output = Vec::new();
        for particle in &self.particles {
            let instance = particle.borrow();
            if view.frustum.contains(instance.get_pos()) {
                let quad = instance
                    .as_single_quad()
                    .expect("SingleQuads particle must provide its SingleQuadParticle bridge");
                quad.extract(world, view, &mut output);
            }
        }
        output
    }
}
