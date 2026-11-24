//! Ordinary usage examples of the contracts, not runtime implementations or
//! tests.

use std::any::type_name;
use std::io;
use std::rc::Rc;

use glam::DVec3;

use super::builtin::{FlameParticleRegistration, SmokeParticleRegistration};
use super::particle::ParticleRef;
use super::particle_provider::ParticleProvider;
use super::registry::{MappedParticleRegistry, ParticleRegistry, ParticleTypeRegistrationError};
use super::render::{ParticleLight, ParticleView, ParticlesRenderState};
use super::resources::ParticleResources;
use super::runtime::ParticleEngine;
use super::single_quad_particle::SingleQuadParticle;
use super::types::{ParticleType, SimpleParticleType};
use crate::world::level::LevelContext;

/// Compile-checked, ordinary usage sketch, not a test or a runtime engine.
/// The supplied initializer receives these SAME registered resources and must
/// successfully prepare descriptions/atlas and apply bindings
/// before spawning. It must establish the binding of any sprite-backed type it
/// uses; absent descriptions do not initialize sets. Initialization errors are
/// propagated. Initialization precedes engine construction, so apply may use a
/// no-op clear callback here: there are not yet any engine-owned particles.
/// The caller supplies particle_type from ParticleRegistry::register.
/// For the sprite-factory form, replace ParticleResources::register with
/// resources.register_sprite_provider(Rc::clone(&particle_type), registration):
/// it supplies the stable SpriteSet to the factory before engine construction.
/// LevelContext::new requires chunks, block particle appearance, real camera
/// position, particle status, a persistent level RNG and the local engine's
/// creation services. Callbacks receive that same full context.
pub fn contract_exemplar<R: ParticleResources>(
    mut resources: R,
    particle_type: Rc<ParticleType<SimpleParticleType>>,
    world: &mut LevelContext<'_>,
    provider: Rc<dyn ParticleProvider<SimpleParticleType>>,
    initialize_resources: impl FnOnce(&R) -> io::Result<()>,
) -> io::Result<Option<ParticleRef>> {
    // A provider for a different options schema cannot bind to this token.
    resources.register::<SimpleParticleType>(Rc::clone(&particle_type), provider);
    let options = SimpleParticleType::new(particle_type);
    let resources = Rc::new(resources);
    initialize_resources(resources.as_ref())?;
    let mut engine = ParticleEngine::new(resources);
    let engine_access = engine.engine_access();
    let mut level = LevelContext::new(
        world.chunks(),
        world.block_particle_appearance(),
        world.block_tags,
        world.camera_render_position,
        world.particle_status,
        world.get_random(),
        engine_access.as_ref(),
    );
    // &options implicitly coerces to &dyn ParticleOptions.
    // options.get_type().name() selects the registered provider.
    // register::<SimpleParticleType>() requires a
    // ParticleProvider<SimpleParticleType>.
    let created = engine.create_particle(&mut level, &options, DVec3::ZERO, DVec3::ZERO);
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

/// Bootstrap a smoke token for the existing provider-registration exemplars.
/// Provider resources and the engine remain the caller's responsibility.
pub fn contract_registry_exemplar() -> Result<
    (MappedParticleRegistry, Rc<ParticleType<SimpleParticleType>>),
    ParticleTypeRegistrationError,
> {
    let mut registry = MappedParticleRegistry::new();
    let smoke = registry.register(crate::resources::resource_id!("minecraft:smoke"), false)?;
    registry.freeze();
    Ok((registry, smoke))
}

/// Register a smoke sprite provider, retain an instance across ticks, and
/// extract an owned snapshot before clearing and switching worlds.
///
/// Assumes the independently registered token's `info().name()` is
/// `minecraft:smoke`. The initializer receives these SAME registered resources,
/// successfully preparing descriptions/atlas and applying bindings before
/// engine construction. It must establish smoke's binding;
/// absent JSON leaves the set unbound. Initialization errors propagate.
/// Neither assumption is proved by the options schema. Registration supplies
/// the concrete Smoke provider; the engine must dispatch through it. Engine
/// construction uses ParticleEngine; sprite preparation/binding remains
/// caller work. Since initialization precedes the engine, apply may use a no-op
/// clear callback. This example does not demonstrate runtime or visual
/// fidelity. The returned snapshot describes the old world, not a proof of
/// world lifetime. LevelContext::new requires chunks, block particle
/// appearance, real camera position, particle status, a persistent level RNG
/// and the local engine's creation services for both contexts. Input contexts
/// supply only world/client inputs; callbacks receive the newly constructed
/// full context.
pub fn contract_exemplar_smoke<R: ParticleResources>(
    mut resources: R,
    particle_type: Rc<ParticleType<SimpleParticleType>>,
    world: &mut LevelContext<'_>,
    view: &ParticleView<'_>,
    replacement_world: &mut LevelContext<'_>,
    initialize_resources: impl FnOnce(&R) -> io::Result<()>,
) -> io::Result<(Option<ParticleRef>, ParticlesRenderState)> {
    resources.register_sprite_provider(Rc::clone(&particle_type), &SmokeParticleRegistration);
    let options = SimpleParticleType::new(particle_type);
    let resources = Rc::new(resources);
    initialize_resources(resources.as_ref())?;
    let mut engine = ParticleEngine::new(resources);
    let engine_access = engine.engine_access();
    let mut level = LevelContext::new(
        world.chunks(),
        world.block_particle_appearance(),
        world.block_tags,
        world.camera_render_position,
        world.particle_status,
        world.get_random(),
        engine_access.as_ref(),
    );
    let created = engine.create_particle(&mut level, &options, DVec3::ZERO, DVec3::ZERO);
    let retained = created.as_ref().map(Rc::clone);
    if let Some(particle) = &retained {
        let mut instance = particle.borrow_mut();
        instance.set_lifetime(40);
    } // Release the initializer guard before engine re-entry.

    // Creation already called add; never enqueue the retained identity again.
    // First tick drains pending into groups, subject to quota/group rejection.
    engine.tick(&mut level);
    // Only an admitted, still-active particle can now receive an update.
    engine.tick(&mut level);
    if let Some(particle) = &retained {
        let mut instance = particle.borrow_mut();
        instance.set_lifetime(60);
    } // Retained mutation beyond initialization; release before extraction.

    let old_world_snapshot = engine.extract(&level, view);
    engine.clear_particles();
    // Detachment does not promise is_alive() == false on a retained instance.
    engine.set_level();
    // Notification stores no world: subsequent calls explicitly supply it.
    let mut replacement_level = LevelContext::new(
        replacement_world.chunks(),
        replacement_world.block_particle_appearance(),
        replacement_world.block_tags,
        replacement_world.camera_render_position,
        replacement_world.particle_status,
        replacement_world.get_random(),
        engine_access.as_ref(),
    );
    engine.tick(&mut replacement_level);
    Ok((retained, old_world_snapshot))
}

/// Register the ordinary Flame provider, mutate a retained instance, query its
/// light, and extract an owned frame snapshot.
///
/// Assumes the caller-established token names `minecraft:flame`; the options
/// schema alone does not prove that identity. The initializer receives these
/// SAME registered resources, successfully preparing descriptions/atlas and
/// applying Flame's binding before engine construction. Absent JSON
/// does not bind the set. Initialization errors propagate, and the engine must
/// dispatch through the concrete registered provider. This is a readiness
/// precondition, not runtime adoption or a demonstration of visual fidelity.
/// Initialization precedes the engine, so apply may use a no-op clear callback.
/// Retained identity/world association and cached sprites after reload remain
/// deferred lifecycle concerns.
/// LevelContext::new requires chunks, block particle appearance, real camera
/// position, particle status, a persistent level RNG and the local engine's
/// creation services. Callbacks receive that same full context.
pub fn contract_exemplar_flame<R: ParticleResources>(
    mut resources: R,
    particle_type: Rc<ParticleType<SimpleParticleType>>,
    world: &mut LevelContext<'_>,
    view: &ParticleView<'_>,
    initialize_resources: impl FnOnce(&R) -> io::Result<()>,
) -> io::Result<(
    Option<ParticleRef>,
    Option<ParticleLight>,
    ParticlesRenderState,
)> {
    resources.register_sprite_provider(Rc::clone(&particle_type), &FlameParticleRegistration);
    let options = SimpleParticleType::new(particle_type);
    let resources = Rc::new(resources);
    initialize_resources(resources.as_ref())?;
    let mut engine = ParticleEngine::new(resources);
    let engine_access = engine.engine_access();
    let mut level = LevelContext::new(
        world.chunks(),
        world.block_particle_appearance(),
        world.block_tags,
        world.camera_render_position,
        world.particle_status,
        world.get_random(),
        engine_access.as_ref(),
    );
    let created = engine.create_particle(&mut level, &options, DVec3::ZERO, DVec3::ZERO);
    let retained = created.as_ref().map(Rc::clone);
    if let Some(particle) = &retained {
        let mut instance = particle.borrow_mut();
        instance.set_lifetime(24);
        instance.set_velocity(DVec3::new(0.0, 0.01, 0.0));
    } // Release the guard before engine re-entry; creation already enqueued it.

    // First tick drains pending; only admitted active particles update next.
    engine.tick(&mut level);
    engine.tick(&mut level);
    let light = retained.as_ref().map(|particle| {
        particle
            .borrow()
            .get_light_coords(&level, view.partial_tick)
    });
    let snapshot = engine.extract(&level, view);
    Ok((retained, light, snapshot))
}

/// Exemplar-only capability for the caller's concrete SparkParticle equivalent.
/// No implementation is supplied, and this is not a universal particle API.
/// These setters model Vanilla's spark-specific trail, twinkle and fade
/// controls; alpha and RGB color already belong to SingleQuadParticle.
pub trait FireworkSpark: SingleQuadParticle {
    fn set_trail(&mut self, trail: bool);
    fn set_twinkle(&mut self, twinkle: bool);
    /// Packed `0xRRGGBB`, matching Vanilla's setFadeColor input.
    fn set_fade_color(&mut self, color: u32);
}

/// Invalid child identity/type is distinct from missing or declining providers.
#[derive(Debug, thiserror::Error)]
pub enum ExemplarError {
    #[error("firework creation returned the already-borrowed parent, not a distinct child")]
    ChildIsParent,
    #[error(
        "firework provider returned a particle other than the expected concrete spark {expected}"
    )]
    SparkTypeMismatch { expected: &'static str },
}

/// Model Starter.createParticle (Vanilla 26.3 FireworkParticles.java:292-312):
/// synchronously create, cast to a concrete spark, then customize the same
/// child. Assumes `firework_options.name()` is the canonical
/// `minecraft:firework`. SimpleParticleType registration constrains options,
/// NOT the output class.
///
/// This body models logic executed while a parent is mutably borrowed; it does
/// not invoke or implement Particle::tick. The scoped context makes that call
/// expressible. The caller clones `engine.engine_access()` before constructing
/// the full context with `LevelContext::new`; that same context can be used
/// while mutably ticking the engine. Keep the access handle local and the
/// engine alive. ParticleEngine splits active storage from spawning
/// services. SparkParticle's trail tick instead
/// constructs directly and calls add (lines 117-134); that is not this path.
/// A returned child may still be rejected by admission, and None means only a
/// missing provider or provider decline. Errors do not undo
/// creation/enqueueing.
pub fn contract_exemplar_firework<Spark: FireworkSpark>(
    context: &mut LevelContext<'_>,
    firework_options: &SimpleParticleType,
    parent: &ParticleRef,
    fade_color: Option<u32>,
) -> Result<Option<ParticleRef>, ExemplarError> {
    let parent_guard = parent.borrow_mut();
    let engine = context.particle_engine();
    let created = engine.create_particle(
        context,
        firework_options,
        parent_guard.get_pos(),
        DVec3::new(0.0, 0.1, 0.0),
    );
    if let Some(child) = &created {
        // Check before borrowing: otherwise a parent alias would panic here.
        if Rc::ptr_eq(parent, child) {
            return Err(ExemplarError::ChildIsParent);
        }
        let mut child_guard = child.borrow_mut();
        let spark = child_guard.as_any_mut().downcast_mut::<Spark>().ok_or(
            ExemplarError::SparkTypeMismatch {
                expected: type_name::<Spark>(),
            },
        )?;
        spark.set_trail(true);
        spark.set_twinkle(true);
        spark.set_alpha(0.99);
        spark.set_color([1.0, 0.0, 0.0]);
        if let Some(color) = fade_color {
            spark.set_fade_color(color);
        }
    } // Release child guard before any subsequent context/engine operation.
    // Explicitly hold the parent borrow THROUGH synchronous child configuration.
    drop(parent_guard);
    // create_particle already added it; return the same retained allocation.
    Ok(created)
}
