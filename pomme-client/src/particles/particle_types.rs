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
        })
    }
}
