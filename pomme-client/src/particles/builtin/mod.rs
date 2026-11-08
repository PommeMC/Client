//! Builtin particle simulations, providers and sprite-provider factories.

use std::rc::Rc;

use super::particle_types::ParticleTypes;
use super::resources::{MappedParticleResources, ParticleResources};

mod bubble_column;
mod campfire_smoke;
mod drip;
mod dust;
mod dust_color_transition;
mod end_rod;
mod flame;
mod fly_towards_position;
mod lava;
mod portal;
mod reverse_portal;
mod smoke;
mod suspended;
mod suspended_town;
mod terrain;
mod water_drop;

use self::bubble_column::{
    BubbleColumnUpParticleRegistration, WaterCurrentDownParticleRegistration,
};
use self::campfire_smoke::{CosyParticleRegistration, SignalParticleRegistration};
use self::drip::{
    DripFallParticleRegistration, DripHangParticleRegistration, DripLandParticleRegistration,
};
use self::dust::DustParticleRegistration;
use self::dust_color_transition::DustColorTransitionParticleRegistration;
pub(super) use self::end_rod::EndRodParticleRegistration;
pub(super) use self::flame::FlameParticleRegistration;
use self::flame::SmallFlameParticleRegistration;
pub use self::flame::{FlameParticle, FlameProvider};
use self::fly_towards_position::EnchantParticleRegistration;
use self::lava::LavaParticleRegistration;
use self::portal::PortalParticleRegistration;
use self::reverse_portal::ReversePortalParticleRegistration;
pub(super) use self::smoke::SmokeParticleRegistration;
use self::smoke::{LargeSmokeParticleRegistration, WhiteSmokeParticleRegistration};
pub use self::smoke::{SmokeParticle, SmokeProvider};
use self::suspended::{SporeBlossomAirParticleRegistration, UnderwaterParticleRegistration};
use self::suspended_town::{HappyVillagerParticleRegistration, SuspendedTownParticleRegistration};
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
        &DripHangParticleRegistration::water(Rc::clone(&types.falling_water)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.falling_water),
        &DripFallParticleRegistration::water(Rc::clone(&types.splash)),
    );
    resources.register_sprite_provider(Rc::clone(&types.splash), &SplashParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.dripping_lava),
        &DripHangParticleRegistration::lava(Rc::clone(&types.falling_lava)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.falling_lava),
        &DripFallParticleRegistration::lava(Rc::clone(&types.landing_lava)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.landing_lava),
        &DripLandParticleRegistration::lava(),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.dripping_obsidian_tear),
        &DripHangParticleRegistration::obsidian_tear(Rc::clone(&types.falling_obsidian_tear)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.falling_obsidian_tear),
        &DripFallParticleRegistration::obsidian_tear(Rc::clone(&types.landing_obsidian_tear)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.landing_obsidian_tear),
        &DripLandParticleRegistration::obsidian_tear(),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.dripping_honey),
        &DripHangParticleRegistration::honey(Rc::clone(&types.falling_honey)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.falling_honey),
        &DripFallParticleRegistration::honey(Rc::clone(&types.landing_honey)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.landing_honey),
        &DripLandParticleRegistration::honey(),
    );
    resources.register_sprite_provider(Rc::clone(&types.portal), &PortalParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.enchant), &EnchantParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.reverse_portal),
        &ReversePortalParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.bubble_column_up),
        &BubbleColumnUpParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.current_down),
        &WaterCurrentDownParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.mycelium),
        &SuspendedTownParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.falling_spore_blossom),
        &DripFallParticleRegistration::spore_blossom(),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.spore_blossom_air),
        &SporeBlossomAirParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.white_smoke),
        &WhiteSmokeParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.happy_villager),
        &HappyVillagerParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.dust_color_transition),
        &DustColorTransitionParticleRegistration,
    );
}
