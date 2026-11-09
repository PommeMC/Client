//! Vanilla 26.3 particle layer, separate from legacy `crate::particle`.
//! Type/provider storage and CPU description/sprite binding are implemented.
//! World LevelContext owns queries/policy and narrow engine access. Concrete
//! Smoke, Flame, EndRod and Terrain simulations and providers accompany a CPU
//! engine for SingleQuads and NoRender. AppCore owns and ticks the canonical
//! engine with a fully bound context.
//! Sprite providers share the independent particle atlas lifecycle; Terrain
//! resolves its appearance from the block atlas and block colors.
//! Server EndRod packets use the canonical engine and dedicated atlas draws;
//! ordinary Sniffer terrain uses typed BLOCK requests. World-owned block
//! destruction directly adds Terrain instances; item effects use the legacy
//! store. Leaf values, typed
//! tokens and sealed options erasure are real data/bridges, not placeholder
//! behavior. The ordinary contract_exemplar
//! needs a caller's real provider and resource/atlas initializer.
//!
//! Root contracts describe identity, instances and extraction values. Resources
//! own provider bindings, descriptions and sprites; runtime owns engine/groups;
//! builtin owns concrete simulations and providers. Lower layers import owners
//! directly, not this facade. No public plug-in registration API.
#![allow(
    dead_code,
    reason = "Particle contracts beyond root ownership/ticking await producer and renderer adoption"
)]

pub(crate) mod block_particle_option;
mod builtin;
mod contract_exemplars;
pub(crate) mod dust_color_transition_options;
pub(crate) mod dust_particle_options;
mod particle;
pub(crate) mod particle_engine_access;
mod particle_provider;
mod particle_types;
pub(crate) mod power_particle_option;
mod registry;
pub(crate) mod render;
mod resources;
mod runtime;
mod single_quad_particle;
pub(crate) mod types;

pub(crate) use self::builtin::{TerrainParticle, register_builtin_providers};
pub(crate) use self::particle_types::ParticleTypes;
#[allow(
    unused_imports,
    reason = "Some contract exports await producer and renderer adoption"
)]
pub use self::{
    builtin::{FlameParticle, FlameProvider, SmokeParticle, SmokeProvider},
    contract_exemplars::{
        ExemplarError, FireworkSpark, contract_exemplar, contract_exemplar_firework,
        contract_exemplar_flame, contract_exemplar_smoke, contract_registry_exemplar,
    },
    particle::{Particle, ParticleRef},
    particle_provider::ParticleProvider,
    registry::{MappedParticleRegistry, ParticleRegistry, ParticleTypeRegistrationError},
    render::{
        ParticleFacing, ParticleLayer, ParticleLight, ParticleRenderType, ParticleView,
        ParticlesRenderState, QuadParticleRenderState,
    },
    resources::{
        MappedParticleResources, ParticleAtlasSprites, ParticleDescription, ParticleResources,
        PreparedParticleDescriptions, SpriteParticleRegistration, SpriteSet,
    },
    runtime::{NoRenderParticleGroup, ParticleEngine, ParticleGroup, QuadParticleGroup},
    single_quad_particle::SingleQuadParticle,
    types::{
        ParticleLimit, ParticleOptions, ParticleType, ParticleTypeInfo, SimpleParticleType,
        TypedParticleOptions,
    },
};
