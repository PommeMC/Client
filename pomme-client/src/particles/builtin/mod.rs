//! Builtin particle simulations, providers and sprite-provider factories.

use std::rc::Rc;

use super::particle_types::ParticleTypes;
use super::resources::{MappedParticleResources, ParticleResources};

mod end_rod;
mod flame;
mod smoke;
mod terrain;

pub(super) use self::end_rod::EndRodParticleRegistration;
pub(super) use self::flame::FlameParticleRegistration;
pub use self::flame::{FlameParticle, FlameProvider};
pub(super) use self::smoke::SmokeParticleRegistration;
pub use self::smoke::{SmokeParticle, SmokeProvider};
pub(crate) use self::terrain::TerrainParticle;
use self::terrain::TerrainProvider;

pub(crate) fn register_builtin_providers(
    types: &ParticleTypes,
    resources: &mut MappedParticleResources,
) {
    resources.register_sprite_provider(Rc::clone(&types.smoke), &SmokeParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.flame), &FlameParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.soul_fire_flame),
        &FlameParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.copper_fire_flame),
        &FlameParticleRegistration,
    );
    resources.register_sprite_provider(Rc::clone(&types.end_rod), &EndRodParticleRegistration);
    resources.register(Rc::clone(&types.block), Rc::new(TerrainProvider));
}
