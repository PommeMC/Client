//! Typed particle construction contract.

use glam::DVec3;

use super::particle::ParticleRef;
use super::types::TypedParticleOptions;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

/// Typed options, nullable construction, explicit engine RNG. Auxiliary values
/// are provider-defined doubles, not universally a velocity. Instances may
/// own independent random streams; this argument is the engine creation stream.
/// The full context permits synchronous nested spawning. RandomSource borrows
/// only per draw, never across a provider callback.
pub trait ParticleProvider<O: TypedParticleOptions> {
    fn create_particle(
        &self,
        options: &O,
        world: &mut LevelContext<'_>,
        position: DVec3,
        auxiliary: DVec3,
        random: &RandomSource,
    ) -> Option<ParticleRef>;
}
