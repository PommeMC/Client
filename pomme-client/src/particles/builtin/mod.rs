//! Builtin particle simulations, providers and sprite-provider factories.

use std::rc::Rc;

use super::particle_types::ParticleTypes;
use super::resources::{MappedParticleResources, ParticleResources};

mod campfire_smoke;
mod drip;
mod dust;
mod end_rod;
mod flame;
mod lava;
mod smoke;
mod suspended;
mod terrain;
mod water_drop;

use self::campfire_smoke::{CosyParticleRegistration, SignalParticleRegistration};
use self::drip::{
    LavaFallParticleRegistration, LavaHangParticleRegistration, LavaLandParticleRegistration,
    ObsidianTearFallParticleRegistration, ObsidianTearHangParticleRegistration,
    ObsidianTearLandParticleRegistration, WaterFallParticleRegistration,
    WaterHangParticleRegistration,
};
use self::dust::DustParticleRegistration;
pub(super) use self::end_rod::EndRodParticleRegistration;
pub(super) use self::flame::FlameParticleRegistration;
use self::flame::SmallFlameParticleRegistration;
pub use self::flame::{FlameParticle, FlameProvider};
use self::lava::LavaParticleRegistration;
use self::smoke::LargeSmokeParticleRegistration;
pub(super) use self::smoke::SmokeParticleRegistration;
pub use self::smoke::{SmokeParticle, SmokeProvider};
use self::suspended::UnderwaterParticleRegistration;
pub(crate) use self::terrain::TerrainParticle;
use self::terrain::TerrainProvider;
use self::water_drop::SplashParticleRegistration;

pub(crate) fn register_builtin_providers(
    types: &ParticleTypes,
    resources: &mut MappedParticleResources,
) {
    resources.register_sprite_provider(Rc::clone(&types.dust), &DustParticleRegistration);
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
    resources.register_sprite_provider(
        Rc::clone(&types.small_flame),
        &SmallFlameParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.large_smoke),
        &LargeSmokeParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.campfire_cosy_smoke),
        &CosyParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.campfire_signal_smoke),
        &SignalParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.lava),
        &LavaParticleRegistration::new(Rc::clone(&types.smoke)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.underwater),
        &UnderwaterParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.dripping_water),
        &WaterHangParticleRegistration::new(Rc::clone(&types.falling_water)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.falling_water),
        &WaterFallParticleRegistration::new(Rc::clone(&types.splash)),
    );
    resources.register_sprite_provider(Rc::clone(&types.splash), &SplashParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.dripping_lava),
        &LavaHangParticleRegistration::new(Rc::clone(&types.falling_lava)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.falling_lava),
        &LavaFallParticleRegistration::new(Rc::clone(&types.landing_lava)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.landing_lava),
        &LavaLandParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.dripping_obsidian_tear),
        &ObsidianTearHangParticleRegistration::new(Rc::clone(&types.falling_obsidian_tear)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.falling_obsidian_tear),
        &ObsidianTearFallParticleRegistration::new(Rc::clone(&types.landing_obsidian_tear)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.landing_obsidian_tear),
        &ObsidianTearLandParticleRegistration,
    );
}
