//! Engine groups share admission and ordered ticking, not instance behavior.

use super::super::particle::ParticleRef;
use super::super::render::{ParticleRenderType, ParticleView};
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

// Vanilla's group cap and the threshold where gradual rejection begins.
const MAX_PARTICLES_PER_GROUP: usize = 16_384;
const SOFT_PARTICLE_LIMIT: usize = 12_288;

/// Engine-owned simulation group, not one typed batch per provider.
pub trait ParticleGroup {
    type RenderState;

    fn get_render_type(&self) -> ParticleRenderType;
    /// Release each dead member's bookkeeping after its particle guard drops,
    /// before ticking the next member. Callbacks never borrow active storage.
    fn tick_particles(
        &mut self,
        context: &mut LevelContext<'_>,
        on_removed: &mut dyn FnMut(&ParticleRef),
    );
    /// Reject new members, never evict old ones. The engine releases a rejected
    /// pending member's reserved quota and identity.
    fn add(&mut self, particle: ParticleRef, random: &RandomSource) -> bool;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn extract_render_state(
        &self,
        world: &LevelContext<'_>,
        view: &ParticleView<'_>,
    ) -> Self::RenderState;
}

pub(super) fn tick_particles(
    particles: &mut Vec<ParticleRef>,
    context: &mut LevelContext<'_>,
    on_removed: &mut dyn FnMut(&ParticleRef),
) {
    particles.retain(|particle| {
        let alive = {
            let mut instance = particle.borrow_mut();
            instance.tick(context);
            instance.is_alive()
        };
        if !alive {
            on_removed(particle);
        }
        alive
    });
}

pub(super) fn add(
    particles: &mut Vec<ParticleRef>,
    particle: ParticleRef,
    random: &RandomSource,
) -> bool {
    let size = particles.len();
    if size >= MAX_PARTICLES_PER_GROUP {
        return false;
    }
    if size >= SOFT_PARTICLE_LIMIT {
        let free_space = (MAX_PARTICLES_PER_GROUP - size) as f32
            / (MAX_PARTICLES_PER_GROUP - SOFT_PARTICLE_LIMIT) as f32;
        if random.next_float() >= free_space * free_space {
            return false;
        }
    }
    particles.push(particle);
    true
}
