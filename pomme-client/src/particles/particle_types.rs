use std::io;
use std::rc::Rc;

use super::block_particle_option::BlockParticleOption;
use super::dust_particle_options::DustParticleOptions;
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
        })
    }
}
