//! Isolated Vanilla 26.3 particle layer, not connected to `crate::particle`.
//! Type/provider storage and CPU description/sprite binding are implemented.
//! Scoped level queries reuse the existing collision and lighting services.
//! Concrete providers, engine, simulation, atlas sources and GPU/runtime
//! adoption remain deferred.
//! Leaf values, typed tokens and sealed options erasure are real data/bridges,
//! not placeholder behavior. The ordinary contract_exemplar needs a caller's
//! real provider and initializing engine factory.
//!
//! Foundation files hold identity, sprites, world queries and extraction
//! values; particle.rs keeps instance/spawn recursion together. This module
//! composes providers, resource ownership, groups and engine orchestration.
//! Lower layers import siblings directly, not this facade. No public plug-in
//! registration API.
#![allow(
    dead_code,
    reason = "Isolated particle contract sketch; runtime adoption is deferred"
)]

mod contract_exemplars;
mod level;
mod particle;
mod registry;
mod render;
mod resources;
mod sprites;
mod types;
mod world;

use std::io;
use std::rc::Rc;

use glam::DVec3;

#[allow(
    unused_imports,
    reason = "Contract exports are not yet consumed by the runtime"
)]
pub use self::{
    contract_exemplars::{
        ExemplarError, FireworkSpark, contract_exemplar_firework, contract_exemplar_smoke,
        contract_registry_exemplar,
    },
    particle::{ClientLevel, Particle, ParticleRef, ParticleTickContext, SingleQuadParticle},
    registry::{MappedParticleRegistry, ParticleRegistry, ParticleTypeRegistrationError},
    render::{
        ParticleFacing, ParticleFrustum, ParticleLayer, ParticleLight, ParticleRenderType,
        ParticleView, QuadParticleRenderState,
    },
    resources::MappedParticleResources,
    sprites::{ParticleAtlasSprites, ParticleDescription, PreparedParticleDescriptions, SpriteSet},
    types::{
        ParticleLimit, ParticleOptions, ParticleType, ParticleTypeInfo, SimpleParticleType,
        TypedParticleOptions,
    },
    world::ParticleWorld,
};
use crate::resources::ResourceManager;

/// Typed options, nullable construction, explicit engine RNG. Auxiliary values
/// are provider-defined doubles, not universally a velocity. Instances may
/// later own independent random streams; this argument is the creation stream.
pub trait ParticleProvider<O: TypedParticleOptions> {
    fn create_particle(
        &self,
        options: &O,
        world: &dyn ParticleWorld,
        position: DVec3,
        auxiliary: DVec3,
        random: &mut fastrand::Rng,
    ) -> Option<ParticleRef>;
}

/// Typed sprite-provider factory, matching Vanilla's private
/// SpriteParticleRegistration<T>. Called immediately during client provider
/// registration, not during spawning or reload. The provider retains the
/// supplied stable set; this crate-scoped sketch is not a public plug-in SPI.
pub trait SpriteParticleRegistration<O: TypedParticleOptions> {
    fn create(&self, sprite_set: Rc<dyn SpriteSet>) -> Rc<dyn ParticleProvider<O>>;
}

/// Private, object-safe registry/engine collaboration, not a plug-in API.
/// Storage and checked adapters are private to the resource owner.
mod provider_erasure {
    use super::{DVec3, ParticleOptions, ParticleRef, ParticleWorld};

    /// Binding/schema failures are invariants, never nullable construction.
    pub enum ParticleDispatchError {
        ResourceNameMismatch,
        OptionsSchemaMismatch,
    }

    /// Registration must build an adapter specialized for its O, retaining the
    /// exact supplied type token and Rc<dyn ParticleProvider<O>>. Before
    /// calling the typed provider, compare resource names and schema
    /// TypeIds, then use options.as_any().downcast_ref::<O>(); return the
    /// corresponding error if any check fails. Never unchecked-cast,
    /// dispatch by schema alone, or map an invariant failure to None. The
    /// sealed options blanket impl ties the reported schema to the actual
    /// payload; registry bindings still require these runtime checks,
    /// including when options come from another owner.
    pub trait ErasedParticleProvider {
        fn create_particle(
            &self,
            options: &dyn ParticleOptions,
            world: &dyn ParticleWorld,
            position: DVec3,
            auxiliary: DVec3,
            random: &mut fastrand::Rng,
        ) -> Result<Option<ParticleRef>, ParticleDispatchError>;
    }
}

/// Owns client provider bindings and stable sprite sets, not type creation.
/// ParticleType tokens are established by ParticleRegistry::register, so
/// packet/data/server identity does not depend on client provider registration.
/// Generic registration and typed lookup deliberately make this trait
/// non-object-safe; providers, options and engines remain usable as trait
/// objects. No codecs, wire registry, asset cache or external extension
/// framework is supplied.
pub trait ParticleResources {
    /// Associate an already established ParticleType<O> with its matching
    /// provider, retaining that token and specializing the checked erased
    /// adapter for O. Never mint a type or supply type metadata here.
    /// Replace by particle_type.info().name(), warning with both caller
    /// locations, regardless of O or registration form. Preserve any
    /// existing sprite set. Canonical type names belong to the independent
    /// ParticleRegistry owner, not this owner's provider replacement
    /// policy.
    #[track_caller]
    fn register<O: TypedParticleOptions>(
        &mut self,
        particle_type: Rc<ParticleType<O>>,
        provider: Rc<dyn ParticleProvider<O>>,
    );

    /// Rust spelling of Vanilla's second register overload. Create a new
    /// stable unbound SpriteSet indexed by particle_type.info().name(). Call
    /// registration.create immediately with that set, then bind the returned
    /// provider to the supplied token using the same checked erased adapter.
    /// Later apply calls rebind the SAME set without recreating the provider
    /// or engine. Replacement warns as register does. The factory must not
    /// select sprites: binding must precede particle creation, and an absent
    /// description does not bind a set. Type bootstrap is independent.
    #[track_caller]
    fn register_sprite_provider<O: TypedParticleOptions>(
        &mut self,
        particle_type: Rc<ParticleType<O>>,
        registration: &dyn SpriteParticleRegistration<O>,
    );

    /// Lookup by registered name AND schema, not just TypeId. None means an
    /// unknown name; a known name with the wrong schema is an invariant error.
    fn get_provider<O: TypedParticleOptions>(
        &self,
        particle_type: &ParticleType<O>,
    ) -> Result<Option<&dyn ParticleProvider<O>>, provider_erasure::ParticleDispatchError>;

    /// The engine can use this with erased options.get_type(), then invoke the
    /// checked adapter without knowing O. Validate name/schema at lookup too.
    fn get_provider_erased(
        &self,
        particle_type: &ParticleTypeInfo,
    ) -> Result<
        Option<&dyn provider_erasure::ErasedParticleProvider>,
        provider_erasure::ParticleDispatchError,
    >;

    fn get_sprite_set(&self, particle_type: &ParticleTypeInfo) -> Option<Rc<dyn SpriteSet>>;

    /// Discover/parse ordered descriptions using the selected resource manager.
    /// Ignore names without registered sprite sets. Return owned CPU data
    /// before atlas preparation; parse/I/O failures are not producer
    /// admission errors.
    fn prepare(&self, resources: &dyn ResourceManager) -> io::Result<PreparedParticleDescriptions>;

    /// At the caller's existing GPU-safe reload boundary: clear engine state
    /// through the callback, then privately rebind the SAME sprite sets against
    /// the completed atlas. Provider identity survives. Empty descriptions and
    /// missing referenced sprites use the missing region. Absent descriptions
    /// leave previous contents unchanged (including initially unbound sets).
    /// Consumers cannot extract old
    /// retained particles/requeue them without refreshing their cached sprites.
    /// This does not upload atlases or promise transactional all-resource
    /// reload. Prepare and atlas realization must both succeed first; do not
    /// change registrations between prepare and apply on this same owner.
    fn apply(
        &self,
        prepared: PreparedParticleDescriptions,
        atlas: &ParticleAtlasSprites,
        clear_particles: &mut dyn FnMut(),
    );
}

/// Engine-owned group contract, not one typed batch per provider. Associated
/// owned state permits quad collections versus distinct model-render state;
/// item-pickup/elder-guardian state schemas and submission remain deferred.
/// There is no universal empty model state or arbitrary render plug-in
/// registry.
pub trait ParticleGroup {
    type RenderState;

    fn get_render_type(&self) -> ParticleRenderType;
    /// Tick current members only. After detaching each dead member and dropping
    /// its borrow guard, call on_removed exactly once, before the next tick.
    /// The callback releases quota immediately so later child spawns see it.
    /// Context and callback share quota bookkeeping, never active storage.
    fn tick_particles(
        &mut self,
        context: &mut dyn ParticleTickContext,
        on_removed: &mut dyn FnMut(&ParticleRef),
    );
    /// Admission during the final pending drain: cap 16384, reservoir from
    /// 12288 with acceptance ((16384-len)/4096)^2. Reject new, never evict
    /// oldest. False makes the engine release that pending particle's
    /// reserved quota.
    fn add(&mut self, particle: ParticleRef, random: &mut fastrand::Rng) -> bool;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool;
    fn extract_render_state(
        &self,
        world: &dyn ParticleWorld,
        view: &ParticleView<'_>,
    ) -> Self::RenderState;
}

/// Top-level orchestration. Explicit scoped world parameters replace Vanilla's
/// stored level reference; no persistent borrow of GameState or LevelContext.
/// Direct create/add are repeated on ParticleTickContext only to expose the
/// borrow-split tick service, not a second admission policy or deferred
/// factory.
pub trait ParticleEngine {
    /// Owned frame snapshot with separate quad and special-group state. Its
    /// concrete schema is deferred alongside specialized renderer integration.
    type RenderState;

    /// Provider constructs immediately; add receives a clone and this returns
    /// the SAME instance even if quota rejects it. None means missing provider
    /// or provider decline, never group/quota rejection. Bypasses level policy.
    /// Erased registry/adapter invariant errors must be surfaced as fatal
    /// configuration errors, never converted into None by this Option API.
    fn create_particle(
        &mut self,
        world: &dyn ParticleWorld,
        options: &dyn ParticleOptions,
        position: DVec3,
        auxiliary: DVec3,
    ) -> Option<ParticleRef>;
    /// Reserve optional quota keyed by ParticleLimit value and queue the
    /// supplied ParticleRef allocation identity; void on rejection. Equal
    /// limits share a count. Pending accepted members count toward quota but
    /// not group size. Do not enqueue an already pending/live identity twice
    /// or mutate its routing.
    fn add(&mut self, particle: ParticleRef);
    /// Existing groups/deaths first, tracking emitters second, pending
    /// admission last. Children are constructed synchronously but do not
    /// tick in this pass. Tracking-emitter entity attachment/creation
    /// contracts are deferred.
    fn tick(&mut self, world: &dyn ParticleWorld);
    /// Clear groups, pending queue, tracking emitters and quota counts.
    /// Retained references still exist but are no longer simulated;
    /// providers survive.
    fn clear_particles(&mut self);
    /// Rust adaptation: notification of level replacement/unload; clears
    /// exactly as clear_particles. Stores NO level; subsequent calls supply
    /// the new one.
    fn set_level(&mut self);
    fn get_random(&mut self) -> &mut fastrand::Rng;
    /// Sum of active group sizes, excluding pending and tracking emitters.
    fn count_particles(&self) -> usize;
    /// Read-only simulation extraction in SingleQuads, ItemPickup,
    /// ElderGuardians order; NoRender omitted. Quad groups point-frustum-cull;
    /// special groups must supply their own concrete extraction contracts.
    fn extract(&self, world: &dyn ParticleWorld, view: &ParticleView<'_>) -> Self::RenderState;
}

/// Compile-checked, ordinary usage sketch, not a test or a runtime engine.
/// The supplied factory must build its engine from these SAME registered
/// resources, successfully preparing descriptions/atlas and applying bindings
/// before spawning. It must establish the binding of any sprite-backed type it
/// uses; absent descriptions do not initialize sets. Initialization errors are
/// propagated. This is a factory precondition, not an implemented engine.
/// The caller supplies particle_type from ParticleRegistry::register.
/// For the sprite-factory form, replace ParticleResources::register with
/// resources.register_sprite_provider(Rc::clone(&particle_type), registration):
/// it supplies the stable SpriteSet to the factory before engine construction.
pub fn contract_exemplar<R: ParticleResources, S>(
    mut resources: R,
    particle_type: Rc<ParticleType<SimpleParticleType>>,
    world: &dyn ParticleWorld,
    provider: Rc<dyn ParticleProvider<SimpleParticleType>>,
    create_engine: impl FnOnce(Rc<R>) -> io::Result<Box<dyn ParticleEngine<RenderState = S>>>,
) -> io::Result<Option<ParticleRef>> {
    // A provider for a different options schema cannot bind to this token.
    resources.register::<SimpleParticleType>(Rc::clone(&particle_type), provider);
    let options = SimpleParticleType::new(particle_type);
    let mut engine: Box<dyn ParticleEngine<RenderState = S>> = create_engine(Rc::new(resources))?;
    // &options implicitly coerces to &dyn ParticleOptions.
    // options.get_type().name() selects the registered provider.
    // register::<SimpleParticleType>() requires a
    // ParticleProvider<SimpleParticleType>.
    let created = engine.create_particle(world, &options, DVec3::ZERO, DVec3::ZERO);
    if let Some(particle) = created {
        let retained = Rc::clone(&particle);
        {
            let mut instance = retained.borrow_mut();
            instance.set_lifetime(40);
        } // Drop the guard before any engine re-entry.
        // Creation already called add; retaining identity must NOT enqueue it again.
        Ok(Some(retained))
    } else {
        Ok(None)
    }
}
