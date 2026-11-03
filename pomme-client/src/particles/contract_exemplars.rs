//! Ordinary usage examples of the contracts, not runtime implementations or
//! tests.

use std::any::type_name;
use std::rc::Rc;

use glam::DVec3;

use super::particle::{ParticleRef, ParticleTickContext, SingleQuadParticle};
use super::render::ParticleView;
use super::types::{ParticleRegistrationError, ParticleType, SimpleParticleType};
use super::world::ParticleWorld;
use super::{ParticleEngine, ParticleResources, SpriteParticleRegistration};

/// Register a smoke sprite provider, retain an instance across ticks, and
/// extract an owned snapshot before clearing and switching worlds.
///
/// Assumes the independently registered token's `info().name()` is
/// `minecraft:smoke`. The supplied factory must build the engine from these
/// SAME registered resources. Neither assumption is proved by the options
/// schema. The opaque provider supplies all SmokeParticle/BaseAshSmokeParticle
/// behavior; this example does not implement their math or demonstrate visual
/// fidelity. Sprite preparation/binding and concrete implementations remain
/// caller work. The returned snapshot describes the old world, not a proof of
/// world lifetime.
pub fn contract_exemplar_smoke<R: ParticleResources, S>(
    mut resources: R,
    particle_type: Rc<ParticleType<SimpleParticleType>>,
    registration: &dyn SpriteParticleRegistration<SimpleParticleType>,
    world: &dyn ParticleWorld,
    view: &ParticleView<'_>,
    replacement_world: &dyn ParticleWorld,
    create_engine: impl FnOnce(Rc<R>) -> Box<dyn ParticleEngine<RenderState = S>>,
) -> Result<(Option<ParticleRef>, S), ParticleRegistrationError> {
    resources.register_sprite_provider(Rc::clone(&particle_type), registration)?;
    let options = SimpleParticleType::new(particle_type);
    let mut engine = create_engine(Rc::new(resources));
    let created = engine.create_particle(world, &options, DVec3::ZERO, DVec3::ZERO);
    let retained = created.as_ref().map(Rc::clone);
    if let Some(particle) = &retained {
        let mut instance = particle.borrow_mut();
        instance.set_lifetime(40);
    } // Release the initializer guard before engine re-entry.

    // Creation already called add; never enqueue the retained identity again.
    // First tick drains pending into groups, subject to quota/group rejection.
    engine.tick(world);
    // Only an admitted, still-active particle can now receive an update.
    engine.tick(world);
    if let Some(particle) = &retained {
        let mut instance = particle.borrow_mut();
        instance.set_lifetime(60);
    } // Retained mutation beyond initialization; release before extraction.

    let old_world_snapshot = engine.extract(world, view);
    engine.clear_particles();
    // Detachment does not promise is_alive() == false on a retained instance.
    engine.set_level();
    // Notification stores no world: subsequent calls explicitly supply it.
    engine.tick(replacement_world);
    Ok((retained, old_world_snapshot))
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
/// expressible, but does not prove an engine implementation actually splits its
/// active storage from spawning services. SparkParticle's trail tick instead
/// constructs directly and calls add (lines 117-134); that is not this path.
/// A returned child may still be rejected by admission, and None means only a
/// missing provider or provider decline. Errors do not undo
/// creation/enqueueing.
pub fn contract_exemplar_firework<Spark: FireworkSpark>(
    context: &mut dyn ParticleTickContext,
    firework_options: &SimpleParticleType,
    parent: &ParticleRef,
    fade_color: Option<u32>,
) -> Result<Option<ParticleRef>, ExemplarError> {
    let parent_guard = parent.borrow_mut();
    let created = context.create_particle(
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
