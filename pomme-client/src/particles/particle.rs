//! Instance contracts and their mutually recursive, scoped spawn/world bridge.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use glam::DVec3;

use super::render::{ParticleLight, ParticleRenderType};
use super::single_quad_particle::SingleQuadParticle;
use super::types::ParticleLimit;
use crate::physics::aabb::Aabb;
use crate::world::level::LevelContext;

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
    fn tick(&mut self, context: &mut LevelContext<'_>);
    /// Rust spelling of `move`; applies this instance's collision/physics
    /// rules.
    fn move_by(&mut self, world: &LevelContext<'_>, displacement: DVec3);
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
    fn get_light_coords(&self, world: &LevelContext<'_>, partial_tick: f32) -> ParticleLight;
}

pub(super) fn bounds_at(position: DVec3, width: f32, height: f32) -> Aabb {
    let half_width = f64::from(width / 2.0);
    Aabb::new(
        DVec3::new(position.x - half_width, position.y, position.z - half_width),
        DVec3::new(
            position.x + half_width,
            position.y + f64::from(height),
            position.z + half_width,
        ),
    )
}

pub(super) fn resized_bounds(bounds: Aabb, width: f32, height: f32) -> Aabb {
    let min = DVec3::new(
        (bounds.min.x + bounds.max.x - f64::from(width)) / 2.0,
        bounds.min.y,
        (bounds.min.z + bounds.max.z - f64::from(width)) / 2.0,
    );
    Aabb::new(
        min,
        min + DVec3::new(f64::from(width), f64::from(height), f64::from(width)),
    )
}

pub(super) fn randomized_motion(random: &mut fastrand::Rng, auxiliary: DVec3) -> DVec3 {
    let mut velocity = DVec3::new(
        auxiliary.x + f64::from((random.f32() * 2.0 - 1.0) * 0.4),
        auxiliary.y + f64::from((random.f32() * 2.0 - 1.0) * 0.4),
        auxiliary.z + f64::from((random.f32() * 2.0 - 1.0) * 0.4),
    );
    let speed = f64::from((random.f32() + random.f32() + 1.0) * 0.15);
    // Java promotes these float constants after the f32 random arithmetic.
    velocity = velocity / velocity.length() * speed * f64::from(0.4_f32);
    velocity.y += f64::from(0.1_f32);
    velocity
}
