//! Builtin particle simulations, providers and sprite-provider factories.

use std::rc::Rc;

use super::particle_types::ParticleTypes;
use super::resources::{MappedParticleResources, ParticleResources};

mod ash;
mod attack_sweep;
mod bubble_column;
mod bubble_pop;
mod campfire_smoke;
mod crit;
mod dragon_breath;
mod drip;
mod dripstone_fall;
mod dust;
mod dust_color_transition;
mod dust_plume;
mod end_rod;
mod explode;
mod falling_dust;
mod falling_leaves;
mod firefly;
mod firework_spark;
mod flame;
mod fly_straight_towards;
mod fly_towards_position;
mod glow;
mod gust;
mod gust_seed;
mod heart;
mod huge_explosion;
mod huge_explosion_seed;
mod lava;
mod nectar_fall;
mod note;
mod noxious_gas;
mod portal;
mod reverse_portal;
mod sculk_charge_pop;
mod simple_vertical;
mod smoke;
mod snowflake;
mod soul;
mod spit;
mod squid_ink;
mod sulfur_bubble;
mod suspended;
mod suspended_town;
mod terrain;
mod totem;
mod trial_spawner_detection;
mod vault_connection;
mod wake;
mod water_drop;
mod white_ash;

use self::ash::AshParticleRegistration;
use self::attack_sweep::AttackSweepParticleRegistration;
use self::bubble_column::{
    BubbleColumnUpParticleRegistration, WaterCurrentDownParticleRegistration,
};
use self::bubble_pop::BubblePopParticleRegistration;
use self::campfire_smoke::{CosyParticleRegistration, SignalParticleRegistration};
use self::crit::{
    CritParticleRegistration, DamageIndicatorParticleRegistration, EnchantedHitParticleRegistration,
};
use self::dragon_breath::DragonBreathParticleRegistration;
use self::drip::{
    DripFallParticleRegistration, DripHangParticleRegistration, DripLandParticleRegistration,
};
use self::dripstone_fall::DripstoneFallParticleRegistration;
use self::dust::DustParticleRegistration;
use self::dust_color_transition::DustColorTransitionParticleRegistration;
use self::dust_plume::DustPlumeParticleRegistration;
pub(super) use self::end_rod::EndRodParticleRegistration;
use self::explode::ExplodeParticleRegistration;
use self::falling_dust::FallingDustParticleRegistration;
use self::falling_leaves::{CherryParticleRegistration, PaleOakParticleRegistration};
use self::firefly::FireflyParticleRegistration;
use self::firework_spark::SparkParticleRegistration;
pub(super) use self::flame::FlameParticleRegistration;
use self::flame::SmallFlameParticleRegistration;
pub use self::flame::{FlameParticle, FlameProvider};
use self::fly_straight_towards::OminousSpawningParticleRegistration;
use self::fly_towards_position::{EnchantParticleRegistration, NautilusParticleRegistration};
use self::glow::{
    GlowParticleRegistration, ScrapeParticleRegistration, WaxOffParticleRegistration,
    WaxOnParticleRegistration,
};
use self::gust::{GustParticleRegistration, SmallGustParticleRegistration};
use self::gust_seed::GustSeedProvider;
use self::heart::{AngryVillagerParticleRegistration, HeartParticleRegistration};
use self::huge_explosion::HugeExplosionParticleRegistration;
use self::huge_explosion_seed::HugeExplosionSeedProvider;
use self::lava::LavaParticleRegistration;
use self::nectar_fall::NectarFallParticleRegistration;
use self::note::NoteParticleRegistration;
use self::noxious_gas::NoxiousGasParticleRegistration;
use self::portal::PortalParticleRegistration;
use self::reverse_portal::ReversePortalParticleRegistration;
use self::sculk_charge_pop::SculkChargePopParticleRegistration;
use self::simple_vertical::{
    PauseMobGrowthParticleRegistration, ResetMobGrowthParticleRegistration,
};
pub(super) use self::smoke::SmokeParticleRegistration;
use self::smoke::{LargeSmokeParticleRegistration, WhiteSmokeParticleRegistration};
pub use self::smoke::{SmokeParticle, SmokeProvider};
use self::spit::SpitParticleRegistration;
use self::squid_ink::{GlowSquidInkParticleRegistration, SquidInkParticleRegistration};
use self::sulfur_bubble::SulfurBubbleParticleRegistration;
use self::suspended::{
    CrimsonSporeParticleRegistration, SporeBlossomAirParticleRegistration,
    UnderwaterParticleRegistration, WarpedSporeParticleRegistration,
};
use self::suspended_town::{
    DolphinParticleRegistration, HappyVillagerParticleRegistration,
    SuspendedTownParticleRegistration,
};
pub(crate) use self::terrain::TerrainParticle;
use self::terrain::{DustPillarProvider, TerrainProvider};
use self::totem::TotemParticleRegistration;
use self::trial_spawner_detection::TrialSpawnerDetectionParticleRegistration;
use self::vault_connection::VaultConnectionParticleRegistration;
use self::wake::WakeParticleRegistration;
use self::water_drop::SplashParticleRegistration;
use self::white_ash::WhiteAshParticleRegistration;

pub(crate) fn register_builtin_providers(
    types: &ParticleTypes,
    resources: &mut MappedParticleResources,
) {
    resources
        .register_sprite_provider(Rc::clone(&types.cherry_leaves), &CherryParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.pale_oak_leaves),
        &PaleOakParticleRegistration,
    );
    resources.register_sprite_provider(Rc::clone(&types.firework), &SparkParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.sculk_charge_pop),
        &SculkChargePopParticleRegistration,
    );
    resources.register_sprite_provider(Rc::clone(&types.fishing), &WakeParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.totem_of_undying),
        &TotemParticleRegistration,
    );
    resources.register_sprite_provider(Rc::clone(&types.squid_ink), &SquidInkParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.glow_squid_ink),
        &GlowSquidInkParticleRegistration,
    );
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
    resources.register_sprite_provider(
        Rc::clone(&types.sulfur_bubbles),
        &SulfurBubbleParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.falling_dust),
        &FallingDustParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.composter),
        &suspended_town::ComposterFillParticleRegistration,
    );
    resources.register_sprite_provider(Rc::clone(&types.wax_on), &WaxOnParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.wax_off), &WaxOffParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.scrape), &ScrapeParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.electric_spark),
        &glow::ElectricSparkParticleRegistration,
    );
    resources.register(Rc::clone(&types.dust_pillar), Rc::new(DustPillarProvider));
    resources.register_sprite_provider(
        Rc::clone(&types.egg_crack),
        &suspended_town::EggCrackParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.explosion),
        &HugeExplosionParticleRegistration,
    );
    resources.register(
        Rc::clone(&types.explosion_emitter),
        Rc::new(HugeExplosionSeedProvider::new(Rc::clone(&types.explosion))),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.dragon_breath),
        &DragonBreathParticleRegistration,
    );
    resources.register_sprite_provider(Rc::clone(&types.heart), &HeartParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.note), &NoteParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.crit), &CritParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.enchanted_hit),
        &EnchantedHitParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.damage_indicator),
        &DamageIndicatorParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.sweep_attack),
        &AttackSweepParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.angry_villager),
        &AngryVillagerParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.sonic_boom),
        &huge_explosion::SonicBoomParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.snowflake),
        &snowflake::SnowflakeParticleRegistration,
    );
    resources.register_sprite_provider(Rc::clone(&types.soul), &soul::SoulParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.sculk_soul),
        &soul::SculkSoulParticleRegistration,
    );
    resources.register_sprite_provider(Rc::clone(&types.gust), &GustParticleRegistration);
    resources
        .register_sprite_provider(Rc::clone(&types.small_gust), &SmallGustParticleRegistration);
    resources.register(
        Rc::clone(&types.gust_emitter_large),
        Rc::new(GustSeedProvider::new(3.0, 7, 0, Rc::clone(&types.gust))),
    );
    resources.register(
        Rc::clone(&types.gust_emitter_small),
        Rc::new(GustSeedProvider::new(1.0, 3, 2, Rc::clone(&types.gust))),
    );
    resources.register_sprite_provider(Rc::clone(&types.poof), &ExplodeParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.rain),
        &water_drop::RainParticleRegistration,
    );
    resources.register_sprite_provider(Rc::clone(&types.ash), &AshParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.white_ash), &WhiteAshParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.spit), &SpitParticleRegistration);
    resources
        .register_sprite_provider(Rc::clone(&types.bubble_pop), &BubblePopParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.crimson_spore),
        &CrimsonSporeParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.warped_spore),
        &WarpedSporeParticleRegistration,
    );
    resources.register_sprite_provider(Rc::clone(&types.dolphin), &DolphinParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.glow), &GlowParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.firefly), &FireflyParticleRegistration);
    resources.register_sprite_provider(Rc::clone(&types.nautilus), &NautilusParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.vault_connection),
        &VaultConnectionParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.ominous_spawning),
        &OminousSpawningParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.dripping_dripstone_lava),
        &DripHangParticleRegistration::lava(Rc::clone(&types.falling_dripstone_lava)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.dripping_dripstone_water),
        &DripHangParticleRegistration::water(Rc::clone(&types.falling_dripstone_water)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.falling_dripstone_lava),
        &DripstoneFallParticleRegistration::lava(Rc::clone(&types.landing_lava)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.falling_dripstone_water),
        &DripstoneFallParticleRegistration::water(Rc::clone(&types.splash)),
    );
    resources.register_sprite_provider(
        Rc::clone(&types.falling_nectar),
        &NectarFallParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.pause_mob_growth),
        &PauseMobGrowthParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.reset_mob_growth),
        &ResetMobGrowthParticleRegistration,
    );
    resources
        .register_sprite_provider(Rc::clone(&types.dust_plume), &DustPlumeParticleRegistration);
    resources.register_sprite_provider(
        Rc::clone(&types.noxious_gas),
        &NoxiousGasParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.trial_spawner_detection),
        &TrialSpawnerDetectionParticleRegistration,
    );
    resources.register_sprite_provider(
        Rc::clone(&types.trial_spawner_detection_ominous),
        &TrialSpawnerDetectionParticleRegistration,
    );
}
