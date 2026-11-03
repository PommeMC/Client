//! Logical atlas inputs, CPU descriptions and stable resource-owned sprite
//! sets.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::renderer::chunk::atlas::AtlasRegion;
use crate::resources::ResourceId;

/// Read-only logical sprites successfully realized by the atlas owner, plus its
/// real missing tile. Description references do not determine atlas membership.
/// Full independent atlas-source interpretation and GPU upload belong to phase
/// 7.
/// COMPAT: Parsing defers to ResourceId, not Vanilla's stricter namespace/case
/// rules. Its rejection of Vanilla's legal empty sprite paths is accepted.
pub struct ParticleAtlasSprites {
    sprites: BTreeMap<ResourceId, AtlasRegion>,
    missing_region: AtlasRegion,
}

impl ParticleAtlasSprites {
    pub fn new(sprites: BTreeMap<ResourceId, AtlasRegion>, missing_region: AtlasRegion) -> Self {
        Self {
            sprites,
            missing_region,
        }
    }

    pub fn get(&self, id: &ResourceId) -> Option<AtlasRegion> {
        self.sprites.get(id).copied()
    }

    pub fn missing_region(&self) -> AtlasRegion {
        self.missing_region
    }
}

/// Only the ordered textures from `particles/<type>.json`, not behavior.
pub struct ParticleDescription {
    /// Logical sprite ResourceIds, not image filenames, in frame order.
    /// COMPAT: See ParticleAtlasSprites compat note
    pub textures: Vec<ResourceId>,
}

impl ParticleDescription {
    pub(super) fn read(reader: impl io::Read) -> io::Result<Self> {
        // GsonHelper reads one value, without a trailing-input end check.
        // RawValue preserves primitive number spelling, including exponent case.
        let mut deserializer = serde_json::Deserializer::from_reader(reader);
        let mut object = BTreeMap::<String, Box<RawValue>>::deserialize(&mut deserializer)
            .map_err(invalid_json)?;
        let Some(textures) = object.remove("textures") else {
            return Ok(Self {
                textures: Vec::new(),
            });
        };
        let values: Vec<Box<RawValue>> =
            serde_json::from_str(textures.get()).map_err(invalid_json)?;
        let textures = values
            .iter()
            .map(|value| {
                let raw = value.get();
                match raw.as_bytes().first() {
                    Some(b'"') => {
                        ResourceId::new(&serde_json::from_str::<String>(raw).map_err(invalid_json)?)
                    }
                    Some(b'-' | b'0'..=b'9' | b't' | b'f') => ResourceId::new(raw),
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "particle texture must be a string, number or boolean",
                        ));
                    }
                }
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
            })
            .collect::<io::Result<_>>()?;
        Ok(Self { textures })
    }
}

fn invalid_json(error: serde_json::Error) -> io::Error {
    io::Error::new(
        error.io_error_kind().unwrap_or(io::ErrorKind::InvalidData),
        error,
    )
}

/// Owned preparation for the same resource owner, with registrations unchanged
/// until apply. No GPU objects, atlas membership list or borrowed manager.
pub struct PreparedParticleDescriptions {
    pub descriptions: BTreeMap<ResourceId, ParticleDescription>,
}

/// Providers retain the same allocation across reloads. Selection before
/// binding violates initialization and panics with the particle name. Absent
/// descriptions do not bind sets. Copied regions, including retained
/// absent-file bindings, must not be assumed valid after atlas replacement.
pub trait SpriteSet {
    /// Requires lifetime > 0 and age <= lifetime. Selects age*(n-1)/lifetime.
    fn get(&self, age: u32, lifetime: u32) -> AtlasRegion;
    /// Uniform selection of a position, retaining duplicate-frame weights.
    fn get_random(&self, random: &mut fastrand::Rng) -> AtlasRegion;
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

    fn get_random(&self, random: &mut fastrand::Rng) -> AtlasRegion {
        self.select(|len| random.usize(..len))
    }

    fn first(&self) -> AtlasRegion {
        self.select(|_| 0)
    }
}
