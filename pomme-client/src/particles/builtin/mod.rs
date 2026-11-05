//! Builtin particle simulations, providers and sprite-provider factories.

use std::rc::Rc;

use super::particle_types::ParticleTypes;
use super::resources::{MappedParticleResources, ParticleResources};

mod end_rod;
mod flame;
mod smoke;

pub(super) use self::end_rod::EndRodParticleRegistration;
pub(super) use self::flame::FlameParticleRegistration;
pub use self::flame::{FlameParticle, FlameProvider};
pub(super) use self::smoke::SmokeParticleRegistration;
pub use self::smoke::{SmokeParticle, SmokeProvider};

pub(crate) fn register_builtin_providers(
    types: &ParticleTypes,
    resources: &mut MappedParticleResources,
) {
    resources.register_sprite_provider(Rc::clone(&types.smoke), &SmokeParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.flame), &FlameParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.end_rod), &EndRodParticleRegistration);
}
