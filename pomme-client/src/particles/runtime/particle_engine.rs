//! Split creation/admission and active groups permit synchronous child
//! spawning.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use glam::DVec3;

use super::super::particle::ParticleRef;
use super::super::particle_engine_access::ParticleEngineAccess;
use super::super::render::{
    ParticleRenderType, ParticleView, ParticlesRenderState, QuadParticleRenderState,
};
use super::super::resources::particle_resources::{ParticleDispatchError, ParticleResources};
use super::super::types::{ParticleLimit, ParticleOptions};
use super::no_render_particle_group::NoRenderParticleGroup;
use super::particle_group::ParticleGroup;
use super::quad_particle_group::QuadParticleGroup;
use crate::util::random_source::RandomSource;
use crate::world::level::LevelContext;

#[derive(Clone, Copy)]
struct Membership {
    render_type: ParticleRenderType,
    quota: Option<ParticleLimit>,
}

#[derive(Default)]
struct Admission {
    pending: VecDeque<ParticleRef>,
    identities: HashMap<*const (), Membership>,
    counts: HashMap<ParticleLimit, usize>,
}

struct ParticleEngineShared<R: ParticleResources> {
    resources: Rc<R>,
    random: RandomSource,
    admission: RefCell<Admission>,
}

fn identity(particle: &ParticleRef) -> *const () {
    Rc::as_ptr(particle).cast::<()>()
}

fn dispatch_error(options: &dyn ParticleOptions, error: ParticleDispatchError) -> ! {
    match error {
        ParticleDispatchError::ResourceNameMismatch => panic!(
            "particle provider resource name mismatch for {}",
            options.get_type().name()
        ),
        ParticleDispatchError::OptionsSchemaMismatch => panic!(
            "particle provider options schema mismatch for {} ({:?})",
            options.get_type().name(),
            options.get_type().options_schema()
        ),
    }
}

impl<R: ParticleResources> ParticleEngineShared<R> {
    fn check_context(&self, level: &LevelContext<'_>) {
        assert!(
            std::ptr::addr_eq(level.particle_engine(), self as &dyn ParticleEngineAccess),
            "LevelContext must reference this particle engine's creation services"
        );
    }

    fn release(&self, particle: &ParticleRef) {
        let mut admission = self.admission.borrow_mut();
        let membership = admission
            .identities
            .remove(&identity(particle))
            .expect("removed particle must have a reserved membership");
        if let Some(quota) = membership.quota {
            let count = admission
                .counts
                .get_mut(&quota)
                .expect("reserved particle quota must have a count");
            *count = count
                .checked_sub(1)
                .expect("reserved particle quota count must be positive");
            if *count == 0 {
                admission.counts.remove(&quota);
            }
        }
    }
}

impl<R: ParticleResources> ParticleEngineAccess for ParticleEngineShared<R> {
    fn create_particle(
        &self,
        level: &mut LevelContext<'_>,
        options: &dyn ParticleOptions,
        position: DVec3,
        auxiliary: DVec3,
    ) -> Option<ParticleRef> {
        self.check_context(level);
        let provider = self
            .resources
            .get_provider_erased(options.get_type())
            .unwrap_or_else(|error| dispatch_error(options, error))?;
        // Clone only the shared handle. Each draw releases its own RNG guard
        // before a provider can synchronously reenter this same stream.
        let random = self.random.clone();
        let particle = provider
            .create_particle(options, level, position, auxiliary, &random)
            .unwrap_or_else(|error| dispatch_error(options, error))?;
        self.add(Rc::clone(&particle));
        Some(particle)
    }

    fn add(&self, particle: ParticleRef) {
        let key = identity(&particle);
        if self.admission.borrow().identities.contains_key(&key) {
            return;
        }
        let membership = {
            let instance = particle.borrow();
            let render_type = instance.get_group();
            match render_type {
                ParticleRenderType::SingleQuads => assert!(
                    instance.as_single_quad().is_some(),
                    "SingleQuads particle must provide its SingleQuadParticle bridge"
                ),
                ParticleRenderType::NoRender => {}
                ParticleRenderType::ItemPickup | ParticleRenderType::ElderGuardians => {
                    panic!(
                        "unsupported particle group {render_type:?}: entity/model state is not implemented"
                    );
                }
            }
            Membership {
                render_type,
                quota: instance.get_particle_limit(),
            }
        };
        let mut admission = self.admission.borrow_mut();
        // Metadata callbacks above ran without a bookkeeping guard.
        if admission.identities.contains_key(&key) {
            return;
        }
        if let Some(quota) = membership.quota {
            let count = admission.counts.get(&quota).copied().unwrap_or(0);
            if count >= quota.limit {
                return;
            }
            admission.counts.insert(quota, count + 1);
        }
        admission.identities.insert(key, membership);
        admission.pending.push_back(particle);
    }

    fn get_random(&self) -> &RandomSource {
        &self.random
    }
}

type ActiveGroup = Box<dyn ParticleGroup<RenderState = Vec<QuadParticleRenderState>>>;

/// Top-level orchestration. Explicit scoped world parameters replace Vanilla's
/// stored level reference; no persistent borrow of GameState or LevelContext.
/// LevelContext's policy-aware add overloads delegate to split
/// creation/admission. Direct creation uses engine access, not a recursive
/// borrow of this whole engine. LevelContext requires camera/status/level RNG
/// and the canonical engine's shared services at construction. Provider/tick
/// callbacks receive that same context, without changing its engine
/// association.
///
/// CPU engine supporting SingleQuads and NoRender only. Special entity/model
/// groups fail at add, before reserving quota. AppCore owns the canonical
/// client-lifetime engine; rendering and tracking emitters remain deferred.
///
/// Contexts are caller-scoped, not stored levels. After reset, callers must not
/// requeue old-world references; after resource reload they must refresh cached
/// sprites before extracting/requeueing retained particles. Neither
/// precondition is enforced by a world/generation identity.
pub struct ParticleEngine<R: ParticleResources> {
    shared: Rc<ParticleEngineShared<R>>,
    groups: HashMap<ParticleRenderType, ActiveGroup>,
}

impl<R: ParticleResources> ParticleEngine<R> {
    /// Sprite resources must be bound before creating sprite-backed particles.
    pub fn new(resources: Rc<R>) -> Self {
        Self {
            shared: Rc::new(ParticleEngineShared {
                resources,
                random: RandomSource::new(),
                admission: RefCell::new(Admission::default()),
            }),
            groups: HashMap::new(),
        }
    }

    /// Clone only existing shared state, independently of the receiver
    /// borrow. Keep this handle frame/example-local: it does not own active
    /// groups. Do not replace or drop the engine while its service is in
    /// use.
    pub(crate) fn engine_access<'r>(&self) -> Rc<dyn ParticleEngineAccess + 'r>
    where
        R: 'r,
    {
        self.shared.clone()
    }

    /// Provider constructs immediately; add receives a clone and this returns
    /// the SAME instance even if quota rejects it. None means missing provider
    /// or provider decline, never group/quota rejection. Bypasses level policy.
    /// Erased registry/adapter invariant errors must be surfaced as fatal
    /// configuration errors, never converted into None by this Option API.
    pub fn create_particle(
        &mut self,
        world: &mut LevelContext<'_>,
        options: &dyn ParticleOptions,
        position: DVec3,
        auxiliary: DVec3,
    ) -> Option<ParticleRef> {
        self.shared
            .create_particle(world, options, position, auxiliary)
    }

    /// Reserve optional quota keyed by ParticleLimit value and queue the
    /// supplied ParticleRef allocation identity; void on rejection. Equal
    /// limits share a count. Pending accepted members count toward quota but
    /// not group size. Do not enqueue an already pending/live identity twice
    /// or mutate its routing.
    pub fn add(&mut self, particle: ParticleRef) {
        self.shared.add(particle);
    }

    /// Existing groups/deaths first, tracking emitters second, pending
    /// admission last. Children are constructed synchronously but do not
    /// tick in this pass. Tracking-emitter entity attachment/creation
    /// contracts are deferred.
    pub fn tick(&mut self, world: &mut LevelContext<'_>) {
        let shared = Rc::clone(&self.shared);
        shared.check_context(world);
        for group in self.groups.values_mut() {
            group.tick_particles(world, &mut |particle| shared.release(particle));
        }
        // Future order is groups -> tracking emitters -> pending. There is no
        // emitter stage until its real entity-attachment contract exists.
        loop {
            let pending = { shared.admission.borrow_mut().pending.pop_front() };
            let Some(particle) = pending else {
                break;
            };
            let membership = {
                *shared
                    .admission
                    .borrow()
                    .identities
                    .get(&identity(&particle))
                    .expect("pending particle must have a reserved membership")
            };
            let group =
                self.groups
                    .entry(membership.render_type)
                    .or_insert_with(|| match membership.render_type {
                        ParticleRenderType::SingleQuads => Box::new(QuadParticleGroup::default()),
                        ParticleRenderType::NoRender => Box::new(NoRenderParticleGroup::default()),
                        ParticleRenderType::ItemPickup | ParticleRenderType::ElderGuardians => {
                            panic!("unsupported particle group was reserved")
                        }
                    });
            // Keep the original alive through release; rejecting the argument
            // clone cannot destroy a particle under a bookkeeping guard.
            if !group.add(Rc::clone(&particle), &shared.random) {
                shared.release(&particle);
            }
        }
    }

    /// Clear groups, pending queue and quota counts.
    /// Retained references still exist but are no longer simulated;
    /// providers, resources and the engine random stream survive.
    pub fn clear_particles(&mut self) {
        let groups = std::mem::take(&mut self.groups);
        let admission = { std::mem::take(&mut *self.shared.admission.borrow_mut()) };
        // Detach everything before dropping any particle; destructors run
        // outside the Admission borrow, just like removal callbacks.
        drop(groups);
        drop(admission);
    }

    /// Rust adaptation: notification of level replacement/unload; clears
    /// exactly as clear_particles. Stores NO level; subsequent calls supply
    /// the new one.
    pub fn set_level(&mut self) {
        self.clear_particles();
    }

    /// The persistent engine stream, distinct from level and instance streams.
    pub fn get_random(&self) -> &RandomSource {
        self.shared.get_random()
    }

    /// Sum of active group sizes, excluding pending and tracking emitters.
    pub fn count_particles(&self) -> usize {
        self.groups.values().map(|group| group.len()).sum()
    }

    /// Read-only SingleQuads extraction; NoRender omitted. Quad groups
    /// point-frustum-cull. Future ItemPickup then ElderGuardians extraction
    /// requires actual specialized states; those routes are unsupported now.
    /// Returns an owned CPU frame snapshot; renderer integration remains
    /// deferred.
    pub fn extract(
        &self,
        world: &LevelContext<'_>,
        view: &ParticleView<'_>,
    ) -> ParticlesRenderState {
        // Future render order: SingleQuads, ItemPickup, ElderGuardians. The
        // latter two require real model states; NoRender is deliberately absent.
        ParticlesRenderState {
            quads: self
                .groups
                .get(&ParticleRenderType::SingleQuads)
                .map(|group| group.extract_render_state(world, view))
                .unwrap_or_default(),
        }
    }
}
