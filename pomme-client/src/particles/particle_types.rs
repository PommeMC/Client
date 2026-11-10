use std::io;
use std::rc::Rc;

use super::block_particle_option::BlockParticleOption;
use super::dust_color_transition_options::DustColorTransitionOptions;
use super::dust_particle_options::DustParticleOptions;
use super::power_particle_option::PowerParticleOption;
use super::registry::{MappedParticleRegistry, ParticleRegistry, ParticleTypeRegistrationError};
use super::types::{ParticleType, SimpleParticleType};
use crate::resources::resource_id;

pub(crate) struct ParticleTypes {
    registry: MappedParticleRegistry,
    pub(crate) smoke: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) flame: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) soul_fire_flame: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) copper_fire_flame: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) end_rod: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) block: Rc<ParticleType<BlockParticleOption>>,
    pub(crate) dust: Rc<ParticleType<DustParticleOptions>>,
    pub(crate) small_flame: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) large_smoke: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) campfire_cosy_smoke: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) campfire_signal_smoke: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) lava: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) underwater: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) dripping_water: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) falling_water: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) splash: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) dripping_lava: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) falling_lava: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) landing_lava: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) dripping_obsidian_tear: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) falling_obsidian_tear: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) landing_obsidian_tear: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) dripping_honey: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) falling_honey: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) landing_honey: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) portal: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) enchant: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) reverse_portal: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) bubble_column_up: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) current_down: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) mycelium: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) falling_spore_blossom: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) spore_blossom_air: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) white_smoke: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) happy_villager: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) dust_color_transition: Rc<ParticleType<DustColorTransitionOptions>>,
    pub(crate) sulfur_bubbles: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) falling_dust: Rc<ParticleType<BlockParticleOption>>,
    pub(crate) composter: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) wax_on: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) wax_off: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) scrape: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) electric_spark: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) dust_pillar: Rc<ParticleType<BlockParticleOption>>,
    pub(crate) egg_crack: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) explosion: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) explosion_emitter: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) dragon_breath: Rc<ParticleType<PowerParticleOption>>,
    pub(crate) heart: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) angry_villager: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) note: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) sweep_attack: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) crit: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) enchanted_hit: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) damage_indicator: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) totem_of_undying: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) squid_ink: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) glow_squid_ink: Rc<ParticleType<SimpleParticleType>>,
    pub(crate) sonic_boom: Rc<ParticleType<SimpleParticleType>>,
}

impl ParticleTypes {
    pub(crate) fn new() -> io::Result<Self> {
        let mut registry = MappedParticleRegistry::new();
        let registration_error = |error| match error {
            ParticleTypeRegistrationError::DuplicateName(name) => {
                io::Error::other(format!("Duplicate builtin particle type: {name}"))
            }
            ParticleTypeRegistrationError::Frozen => {
                io::Error::other("Builtin particle registry was already frozen")
            }
        };
        let smoke = registry
            .register(resource_id!("minecraft:smoke"), false)
            .map_err(registration_error)?;
        let flame = registry
            .register(resource_id!("minecraft:flame"), false)
            .map_err(registration_error)?;
        let soul_fire_flame = registry
            .register(resource_id!("minecraft:soul_fire_flame"), false)
            .map_err(registration_error)?;
        let copper_fire_flame = registry
            .register(resource_id!("minecraft:copper_fire_flame"), false)
            .map_err(registration_error)?;
        let end_rod = registry
            .register(resource_id!("minecraft:end_rod"), false)
            .map_err(registration_error)?;
        let block = registry
            .register(resource_id!("minecraft:block"), false)
            .map_err(registration_error)?;
        let dust = registry
            .register(resource_id!("minecraft:dust"), false)
            .map_err(registration_error)?;
        let small_flame = registry
            .register(resource_id!("minecraft:small_flame"), false)
            .map_err(registration_error)?;
        let large_smoke = registry
            .register(resource_id!("minecraft:large_smoke"), false)
            .map_err(registration_error)?;
        let campfire_cosy_smoke = registry
            .register(resource_id!("minecraft:campfire_cosy_smoke"), true)
            .map_err(registration_error)?;
        let campfire_signal_smoke = registry
            .register(resource_id!("minecraft:campfire_signal_smoke"), true)
            .map_err(registration_error)?;
        let lava = registry
            .register(resource_id!("minecraft:lava"), false)
            .map_err(registration_error)?;
        let underwater = registry
            .register(resource_id!("minecraft:underwater"), false)
            .map_err(registration_error)?;
        let dripping_water = registry
            .register(resource_id!("minecraft:dripping_water"), false)
            .map_err(registration_error)?;
        let falling_water = registry
            .register(resource_id!("minecraft:falling_water"), false)
            .map_err(registration_error)?;
        let splash = registry
            .register(resource_id!("minecraft:splash"), false)
            .map_err(registration_error)?;
        let dripping_lava = registry
            .register(resource_id!("minecraft:dripping_lava"), false)
            .map_err(registration_error)?;
        let falling_lava = registry
            .register(resource_id!("minecraft:falling_lava"), false)
            .map_err(registration_error)?;
        let landing_lava = registry
            .register(resource_id!("minecraft:landing_lava"), false)
            .map_err(registration_error)?;
        let dripping_obsidian_tear = registry
            .register(resource_id!("minecraft:dripping_obsidian_tear"), false)
            .map_err(registration_error)?;
        let falling_obsidian_tear = registry
            .register(resource_id!("minecraft:falling_obsidian_tear"), false)
            .map_err(registration_error)?;
        let landing_obsidian_tear = registry
            .register(resource_id!("minecraft:landing_obsidian_tear"), false)
            .map_err(registration_error)?;
        let dripping_honey = registry
            .register(resource_id!("minecraft:dripping_honey"), false)
            .map_err(registration_error)?;
        let falling_honey = registry
            .register(resource_id!("minecraft:falling_honey"), false)
            .map_err(registration_error)?;
        let landing_honey = registry
            .register(resource_id!("minecraft:landing_honey"), false)
            .map_err(registration_error)?;
        let portal = registry
            .register(resource_id!("minecraft:portal"), false)
            .map_err(registration_error)?;
        let enchant = registry
            .register(resource_id!("minecraft:enchant"), false)
            .map_err(registration_error)?;
        let reverse_portal = registry
            .register(resource_id!("minecraft:reverse_portal"), false)
            .map_err(registration_error)?;
        let bubble_column_up = registry
            .register(resource_id!("minecraft:bubble_column_up"), false)
            .map_err(registration_error)?;
        let current_down = registry
            .register(resource_id!("minecraft:current_down"), false)
            .map_err(registration_error)?;
        let mycelium = registry
            .register(resource_id!("minecraft:mycelium"), false)
            .map_err(registration_error)?;
        let falling_spore_blossom = registry
            .register(resource_id!("minecraft:falling_spore_blossom"), false)
            .map_err(registration_error)?;
        let spore_blossom_air = registry
            .register(resource_id!("minecraft:spore_blossom_air"), false)
            .map_err(registration_error)?;
        let white_smoke = registry
            .register(resource_id!("minecraft:white_smoke"), false)
            .map_err(registration_error)?;
        let happy_villager = registry
            .register(resource_id!("minecraft:happy_villager"), false)
            .map_err(registration_error)?;
        let dust_color_transition = registry
            .register(resource_id!("minecraft:dust_color_transition"), false)
            .map_err(registration_error)?;
        let sulfur_bubbles = registry
            .register(resource_id!("minecraft:sulfur_bubbles"), false)
            .map_err(registration_error)?;
        let falling_dust = registry
            .register(resource_id!("minecraft:falling_dust"), false)
            .map_err(registration_error)?;
        let composter = registry
            .register(resource_id!("minecraft:composter"), false)
            .map_err(registration_error)?;
        let wax_on = registry
            .register(resource_id!("minecraft:wax_on"), true)
            .map_err(registration_error)?;
        let wax_off = registry
            .register(resource_id!("minecraft:wax_off"), true)
            .map_err(registration_error)?;
        let scrape = registry
            .register(resource_id!("minecraft:scrape"), true)
            .map_err(registration_error)?;
        let electric_spark = registry
            .register(resource_id!("minecraft:electric_spark"), true)
            .map_err(registration_error)?;
        let dust_pillar = registry
            .register(resource_id!("minecraft:dust_pillar"), false)
            .map_err(registration_error)?;
        let egg_crack = registry
            .register(resource_id!("minecraft:egg_crack"), false)
            .map_err(registration_error)?;
        let explosion = registry
            .register(resource_id!("minecraft:explosion"), true)
            .map_err(registration_error)?;
        let explosion_emitter = registry
            .register(resource_id!("minecraft:explosion_emitter"), true)
            .map_err(registration_error)?;
        let dragon_breath = registry
            .register(resource_id!("minecraft:dragon_breath"), false)
            .map_err(registration_error)?;
        let heart = registry
            .register(resource_id!("minecraft:heart"), false)
            .map_err(registration_error)?;
        let angry_villager = registry
            .register(resource_id!("minecraft:angry_villager"), false)
            .map_err(registration_error)?;
        let note = registry
            .register(resource_id!("minecraft:note"), false)
            .map_err(registration_error)?;
        let sweep_attack = registry
            .register(resource_id!("minecraft:sweep_attack"), true)
            .map_err(registration_error)?;
        let crit = registry
            .register(resource_id!("minecraft:crit"), false)
            .map_err(registration_error)?;
        let enchanted_hit = registry
            .register(resource_id!("minecraft:enchanted_hit"), false)
            .map_err(registration_error)?;
        let damage_indicator = registry
            .register(resource_id!("minecraft:damage_indicator"), true)
            .map_err(registration_error)?;
        let totem_of_undying = registry
            .register(resource_id!("minecraft:totem_of_undying"), false)
            .map_err(registration_error)?;
        let squid_ink = registry
            .register(resource_id!("minecraft:squid_ink"), true)
            .map_err(registration_error)?;
        let glow_squid_ink = registry
            .register(resource_id!("minecraft:glow_squid_ink"), true)
            .map_err(registration_error)?;
        let sonic_boom = registry
            .register(resource_id!("minecraft:sonic_boom"), true)
            .map_err(registration_error)?;
        registry.freeze();
        Ok(Self {
            registry,
            smoke,
            flame,
            soul_fire_flame,
            copper_fire_flame,
            end_rod,
            block,
            dust,
            small_flame,
            large_smoke,
            campfire_cosy_smoke,
            campfire_signal_smoke,
            lava,
            underwater,
            dripping_water,
            falling_water,
            splash,
            dripping_lava,
            falling_lava,
            landing_lava,
            dripping_obsidian_tear,
            falling_obsidian_tear,
            landing_obsidian_tear,
            dripping_honey,
            falling_honey,
            landing_honey,
            portal,
            enchant,
            reverse_portal,
            bubble_column_up,
            current_down,
            mycelium,
            falling_spore_blossom,
            spore_blossom_air,
            white_smoke,
            happy_villager,
            dust_color_transition,
            sulfur_bubbles,
            falling_dust,
            composter,
            wax_on,
            wax_off,
            scrape,
            electric_spark,
            dust_pillar,
            egg_crack,
            explosion,
            explosion_emitter,
            dragon_breath,
            heart,
            angry_villager,
            note,
            sweep_attack,
            crit,
            enchanted_hit,
            damage_indicator,
            totem_of_undying,
            squid_ink,
            glow_squid_ink,
            sonic_boom,
        })
    }
}
