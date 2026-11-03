//! Instance contracts and their mutually recursive, scoped spawn/world bridge.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::render::{
    ParticleFacing, ParticleLayer, ParticleLight, ParticleRenderType, ParticleView,
    QuadParticleRenderState,
};
use super::sprites::SpriteSet;
use super::types::{ParticleLimit, ParticleOptions};
use super::world::ParticleWorld;
use crate::physics::aabb::Aabb;
use crate::renderer::chunk::atlas::AtlasRegion;

/// Proposed Java-reference bridge, not a final storage/performance framework.
/// Clones retain the SAME mutable allocation after enqueue, quota rejection or
/// removal. Removal detaches simulation; it does not invalidate caller
/// identity. Downcast through a held Ref/RefMut guard using as_any/as_any_mut;
/// references must not escape that guard. No Send/Sync requirement or permanent
/// arena key. Callers must release guards before invoking engine operations
/// that may borrow the instance. Resource reload invalidates cached atlas
/// regions even here.
pub type ParticleRef = Rc<RefCell<dyn Particle>>;

/// Construction belongs to a provider, not a Sized-only trait constructor.
/// Routing (group and optional quota key value) must stay fixed while
/// enqueued/live; other instance state remains customizable through the
/// retained reference.
pub trait Particle: Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Rust trait-object bridge for the quad group; Some for SingleQuads,
    /// None for other groups. Does not require every Particle to be a quad.
    fn as_single_quad(&self) -> Option<&dyn SingleQuadParticle>;
    fn tick(&mut self, context: &mut dyn ParticleTickContext);
    /// Rust spelling of `move`; applies this instance's collision/physics
    /// rules.
    fn move_by(&mut self, world: &dyn ParticleWorld, displacement: DVec3);
    fn remove(&mut self);
    fn is_alive(&self) -> bool;
    fn get_group(&self) -> ParticleRenderType;
    fn get_particle_limit(&self) -> Option<ParticleLimit>;
    fn get_pos(&self) -> DVec3;
    fn set_pos(&mut self, position: DVec3);
    fn get_previous_pos(&self) -> DVec3;
    fn get_velocity(&self) -> DVec3;
    fn set_velocity(&mut self, velocity: DVec3);
    fn get_age(&self) -> u32;
    fn get_lifetime(&self) -> u32;
    fn set_lifetime(&mut self, lifetime: u32);
    fn get_bounding_box(&self) -> Aabb;
    fn set_bounding_box(&mut self, bounds: Aabb);
    fn set_size(&mut self, width: f32, height: f32);
    fn scale(&mut self, factor: f32);
    fn has_physics(&self) -> bool;
    fn set_has_physics(&mut self, enabled: bool);
    fn get_light_coords(&self, world: &dyn ParticleWorld, partial_tick: f32) -> ParticleLight;
}

/// Only quad instances implement this; non-quad groups are not forced through
/// sprite rendering. Base defaults and concrete tick/physics behavior are
/// deferred.
pub trait SingleQuadParticle: Particle {
    fn get_sprite(&self) -> AtlasRegion;
    fn set_sprite(&mut self, sprite: AtlasRegion);
    /// Explicit opt-in by concrete tick code; not automatic base ticking.
    fn set_sprite_from_age(&mut self, sprites: &dyn SpriteSet);
    fn get_uv(&self) -> [f32; 4];
    fn get_color(&self) -> [f32; 3];
    fn set_color(&mut self, color: [f32; 3]);
    fn get_alpha(&self) -> f32;
    fn set_alpha(&mut self, alpha: f32);
    fn get_roll(&self) -> f32;
    fn set_roll(&mut self, roll: f32);
    fn get_facing(&self) -> ParticleFacing;
    fn get_quad_size(&self, partial_tick: f32) -> f32;
    fn get_layer(&self) -> ParticleLayer;
    /// Read-only simulation access; append zero or more owned quad entries to
    /// the group's frame output, preserving existing entries. A delayed Shriek
    /// emits none; an active Shriek or Vibration emits two. This is extraction,
    /// not render work, and does not split the simulated particle's identity.
    fn extract(
        &self,
        world: &dyn ParticleWorld,
        view: &ParticleView<'_>,
        output: &mut Vec<QuadParticleRenderState>,
    );
}

/// Proposed ClientLevel particle service, NOT an implementation on the existing
/// LevelContext. Block callbacks would request options here rather than retain
/// emitter handles; no block/runtime integration is part of this sketch.
pub trait ClientLevel: ParticleWorld {
    /// Settings/distance policy precedes construction. Effective override is
    /// caller flag OR type flag. Otherwise distance <=32 and calculated
    /// settings decide; always_show improves MINIMAL odds, not guaranteed
    /// visibility. Return is void, unlike direct engine creation. Policy
    /// RNG is level-owned.
    fn add_particle(
        &mut self,
        options: &dyn ParticleOptions,
        override_limiter: bool,
        always_show: bool,
        position: DVec3,
        auxiliary: DVec3,
    );
}

/// Borrow bridge for ticking: scoped world queries + level policy + direct
/// engine spawning. Implementations split provider/RNG/pending/quota access
/// from active-group storage before borrowing a ticking ParticleRef. They must
/// never reborrow that active instance or expose the active groups here.
/// Level-policy services must use this split spawn path too, not recursively
/// borrow the whole engine. Settings/camera policy wiring is deferred.
pub trait ParticleTickContext: ClientLevel {
    /// Construct NOW through the provider, add, and return that same
    /// allocation, including on quota rejection. A child can be
    /// downcast/mutated immediately; only group activation waits until the
    /// engine's final pending drain. Missing provider or provider decline
    /// yields None, not admission failure.
    fn create_particle(
        &mut self,
        options: &dyn ParticleOptions,
        position: DVec3,
        auxiliary: DVec3,
    ) -> Option<ParticleRef>;
    /// Direct engine add: bypass level policy, enforce quota and queue
    /// identity.
    fn add(&mut self, particle: ParticleRef);
}
