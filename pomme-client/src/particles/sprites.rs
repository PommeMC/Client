//! CPU description data and stable, resource-owned sprite-set contracts.

use std::collections::BTreeMap;

use crate::renderer::chunk::atlas::AtlasRegion;
use crate::resources::ResourceId;

/// Only the ordered `textures` list from `particles/<type>.json`; not behavior.
/// Default particle-atlas IDs map to `textures/particle/<sprite>.png`. Full
/// `atlases/particles.json` source interpretation is deferred, not implied
/// here.
pub struct ParticleDescription {
    pub textures: Vec<ResourceId>,
}

/// Owned CPU preparation, keyed by registered particle name. No GPU objects or
/// borrowed resource manager. The caller can collect texture names before
/// stitch.
pub struct PreparedParticleDescriptions {
    pub descriptions: BTreeMap<ResourceId, ParticleDescription>,
}

/// Providers retain the same set allocation across reloads; its variable-length
/// contents are rebound privately by ParticleResources. No provider mutator.
/// Reads copy regions from the current binding, never from a fixed frame array.
/// `None` explicitly represents an unbound set (a Rust adaptation of Vanilla's
/// uninitialized list). It is not a lazy load or a missing-sprite fallback.
/// Copied regions must not be used after atlas replacement, including copies
/// held in externally retained particles removed by the reload clear.
pub trait SpriteSet {
    /// Requires lifetime > 0 and age <= lifetime. Selects age*(n-1)/lifetime.
    fn get(&self, age: u32, lifetime: u32) -> Option<AtlasRegion>;
    /// Uniform selection from the current list, independent of age animation.
    fn get_random(&self, random: &mut fastrand::Rng) -> Option<AtlasRegion>;
    fn first(&self) -> Option<AtlasRegion>;
}
