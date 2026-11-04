//! Narrow shared creation/admission access for level callbacks.

use glam::DVec3;

use super::particle::ParticleRef;
use super::types::ParticleOptions;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

/// Borrow support for creation/admission, never active groups or a whole
/// engine. Implemented by ParticleEngine's private ParticleEngineShared, not a
/// plug-in API.
pub(crate) trait ParticleEngineAccess {
    fn create_particle(
        &self,
        level: &mut LevelContext<'_>,
        options: &dyn ParticleOptions,
        position: DVec3,
        auxiliary: DVec3,
    ) -> Option<ParticleRef>;
    fn add(&self, particle: ParticleRef);
    fn get_random(&self) -> &RandomSource;
}
