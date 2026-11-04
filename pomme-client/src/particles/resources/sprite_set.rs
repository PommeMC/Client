//! Stable resource-owned sprite sets and frame selection.

use std::cell::RefCell;

use crate::renderer::chunk::atlas::AtlasRegion;
use crate::resources::ResourceId;
use crate::util::random_source::RandomSource;

/// Providers retain the same allocation across reloads. Selection before
/// binding violates initialization and panics with the particle name. Absent
/// descriptions do not bind sets. Copied regions, including retained
/// absent-file bindings, must not be assumed valid after atlas replacement.
pub trait SpriteSet {
    /// Requires lifetime > 0 and age <= lifetime. Selects age*(n-1)/lifetime.
    fn get(&self, age: u32, lifetime: u32) -> AtlasRegion;
    /// Uniform selection of a position, retaining duplicate-frame weights.
    fn get_random(&self, random: &RandomSource) -> AtlasRegion;
    fn first(&self) -> AtlasRegion;
}

pub(super) struct MutableSpriteSet {
    name: ResourceId,
    sprites: RefCell<Option<Vec<AtlasRegion>>>,
}

impl MutableSpriteSet {
    pub(super) fn new(name: ResourceId) -> Self {
        Self {
            name,
            sprites: RefCell::new(None),
        }
    }

    pub(super) fn bind(&self, sprites: Vec<AtlasRegion>) {
        assert!(
            !sprites.is_empty(),
            "empty sprite binding for particle {}",
            self.name
        );
        *self.sprites.borrow_mut() = Some(sprites);
    }

    fn select(&self, index: impl FnOnce(usize) -> usize) -> AtlasRegion {
        let sprites = self.sprites.borrow();
        let sprites = sprites.as_ref().unwrap_or_else(|| {
            panic!(
                "sprite set for particle {} selected before binding",
                self.name
            )
        });
        sprites[index(sprites.len())]
    }
}

impl SpriteSet for MutableSpriteSet {
    fn get(&self, age: u32, lifetime: u32) -> AtlasRegion {
        assert!(
            lifetime > 0 && age <= lifetime,
            "invalid sprite age {age}/lifetime {lifetime} for particle {}",
            self.name
        );
        self.select(|len| (u128::from(age) * (len - 1) as u128 / u128::from(lifetime)) as usize)
    }

    fn get_random(&self, random: &RandomSource) -> AtlasRegion {
        self.select(|len| random.next_int(len))
    }

    fn first(&self) -> AtlasRegion {
        self.select(|_| 0)
    }
}
