//! The game's registries as Rust types.
//!
//! [`builtin`] holds the registries fixed per game version (items, entity
//! types, sounds), each an enum in protocol-id order. [`data`] holds the ones
//! a server sends on join (biomes, enchantments), each an id plus the keys the
//! game's own data pack names. [`tags`] holds the data pack's tags.
//!
//! The entries are generated from `reference/<version>` by `tools/typegen`
//! (`just typegen <version>`); everything else is hand-written.

#[macro_use]
mod macros;

#[cfg(feature = "azalea")]
mod azalea;
pub mod builtin;
pub mod data;
pub mod identifier;
pub mod tags;

use std::hash::Hash;

pub use identifier::Identifier;

/// A registry whose entries are known by numeric id on the wire.
pub trait Registry: Copy + Ord + Hash {
    fn from_u32(id: u32) -> Option<Self>;
    fn to_u32(&self) -> u32;
}

/// A registry a server sends on join, so an id means nothing until that
/// connection's entries are known.
pub trait DataRegistry: Copy + Ord + Hash {
    /// The registry's path, like `worldgen/biome`.
    const NAME: &'static str;
    type Key: DataRegistryKey;

    fn protocol_id(&self) -> u32;
    fn new_raw(id: u32) -> Self;
}

impl<T: DataRegistry> Registry for T {
    fn from_u32(id: u32) -> Option<Self> {
        Some(Self::new_raw(id))
    }

    fn to_u32(&self) -> u32 {
        self.protocol_id()
    }
}

/// A data registry's entry by name: one the game's data pack has, or any
/// other identifier.
pub trait DataRegistryKey {
    type Borrow<'a>: DataRegistryKeyRef<'a>;

    fn from_ident(ident: Identifier) -> Self;
    fn into_ident(self) -> Identifier;
}

/// A [`DataRegistryKey`] borrowing its identifier.
pub trait DataRegistryKeyRef<'a> {
    type Owned: DataRegistryKey;

    fn to_owned(self) -> Self::Owned;
    fn from_ident(ident: &'a Identifier) -> Self;
    fn into_ident(self) -> Identifier;
}

/// Vanilla `Holder`: a registry entry, or a value sent in its place.
#[derive(Clone, Debug, PartialEq)]
pub enum Holder<R, Direct> {
    Reference(R),
    Direct(Direct),
}

impl<R: Default, Direct> Default for Holder<R, Direct> {
    fn default() -> Self {
        Self::Reference(R::default())
    }
}

/// Vanilla `HolderSet`: a list of registry entries, or a tag naming them.
#[derive(Clone, Debug, PartialEq)]
pub enum HolderSet<R, Key> {
    Direct {
        contents: Vec<R>,
    },
    /// `contents` is empty until the tag is resolved.
    Named {
        key: Key,
        contents: Vec<Key>,
    },
}

impl<R, Key> Default for HolderSet<R, Key> {
    fn default() -> Self {
        Self::Direct {
            contents: Vec::new(),
        }
    }
}

impl<R, Key> From<Vec<R>> for HolderSet<R, Key> {
    fn from(contents: Vec<R>) -> Self {
        Self::Direct { contents }
    }
}

impl<R: PartialEq, Key: From<R> + PartialEq> HolderSet<R, Key> {
    pub fn contains(&self, value: R) -> bool {
        match self {
            Self::Direct { contents } => contents.contains(&value),
            Self::Named { contents, .. } => contents.contains(&Key::from(value)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::builtin::{EntityKind, ItemKind};
    use super::data::{Biome, BiomeKey};
    use super::*;

    #[test]
    fn a_builtin_entry_is_its_protocol_id_and_its_name() {
        assert_eq!(ItemKind::Air.to_u32(), 0);
        assert_eq!(ItemKind::from_u32(1), Some(ItemKind::Stone));
        assert_eq!(ItemKind::Stone.to_str(), "minecraft:stone");
        assert_eq!(ItemKind::Stone.to_string(), "minecraft:stone");
        assert_eq!("stone".parse(), Ok(ItemKind::Stone));
        assert_eq!("minecraft:stone".parse(), Ok(ItemKind::Stone));
        assert_eq!("other:stone".parse::<ItemKind>(), Err(()));
        assert_eq!("not_an_item".parse::<ItemKind>(), Err(()));
    }

    #[test]
    fn every_id_round_trips_and_the_one_past_the_end_is_none() {
        for (id, &kind) in EntityKind::ALL.iter().enumerate() {
            assert_eq!(kind.to_u32(), id as u32);
            assert_eq!(EntityKind::from_u32(id as u32), Some(kind));
            assert_eq!(kind.to_str().parse(), Ok(kind));
        }
        assert_eq!(EntityKind::from_u32(EntityKind::ALL.len() as u32), None);
        assert_eq!(EntityKind::try_from(u32::MAX), Err(()));
    }

    #[test]
    fn a_data_registry_is_an_opaque_id_with_named_keys() {
        assert_eq!(Biome::NAME, "worldgen/biome");
        assert_eq!(Biome::from_u32(7).map(|b| b.to_u32()), Some(7));
        let plains = Identifier::new("minecraft:plains");
        assert_eq!(BiomeKey::from(plains.clone()), BiomeKey::Plains);
        assert_eq!(Identifier::from(<BiomeKey>::Plains), plains);
        let custom = Identifier::new("mod:swamp");
        assert_eq!(
            BiomeKey::from(custom.clone()),
            BiomeKey::Other(custom.clone())
        );
        assert_eq!(Identifier::from(BiomeKey::Other(custom.clone())), custom);
        assert_eq!(<BiomeKey<&Identifier>>::from(&plains), BiomeKey::Plains);
    }

    #[test]
    fn a_holder_set_holds_its_entries_or_a_tags() {
        let direct: HolderSet<ItemKind, Identifier> = vec![ItemKind::Stone].into();
        assert!(direct.contains(ItemKind::Stone));
        assert!(!direct.contains(ItemKind::Air));
        let named = HolderSet::<ItemKind, Identifier>::Named {
            key: Identifier::new("planks"),
            contents: vec![Identifier::new("oak_planks")],
        };
        assert!(named.contains(ItemKind::OakPlanks));
        assert!(!named.contains(ItemKind::Stone));
    }
}
