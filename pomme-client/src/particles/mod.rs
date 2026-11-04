//! Vanilla 26.3 particle layer, separate from legacy `crate::particle`.
//! Type/provider storage and CPU description/sprite binding are implemented.
//! World LevelContext owns queries/policy and narrow engine access. Concrete
//! Smoke and Flame simulations and sprite providers accompany a CPU engine
//! for SingleQuads and NoRender. AppCore owns and ticks the canonical engine
//! with a fully bound context.
//! Provider bootstrap, atlas sources, producers and GPU adoption remain
//! deferred; the legacy store still produces and renders existing effects.
//! Leaf values, typed tokens and sealed options erasure are real data/bridges,
//! not placeholder behavior. The ordinary contract_exemplar needs a caller's
//! real provider and resource/atlas initializer.
//!
//! Root contracts describe identity, instances and extraction values. Resources
//! own provider bindings, descriptions and sprites; runtime owns engine/groups;
//! builtin owns concrete simulations and providers. Lower layers import owners
//! directly, not this facade. No public plug-in registration API.
#![allow(
    dead_code,
    reason = "Particle contracts beyond root ownership/ticking await producer and renderer adoption"
)]

mod builtin;
mod contract_exemplars;
mod particle;
pub(crate) mod particle_engine_access;
mod particle_provider;
mod registry;
pub(crate) mod render;
mod resources;
mod runtime;
mod single_quad_particle;
pub(crate) mod types;

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
        ParticleFacing, ParticleFrustum, ParticleLayer, ParticleLight, ParticleRenderType,
        ParticleView, ParticlesRenderState, QuadParticleRenderState,
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
