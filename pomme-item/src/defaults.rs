//! Each item's default components (vanilla `Item.components()`), as the
//! bytes the game's own `STREAM_CODEC`s wrote for them.
//!
//! `data/default-components.json` is generated from the reference server jar
//! by `tools/codec-oracle/DefaultComponents.java` (`just typegen <version>`).
//! A value holding an entry of a registry a server sends (a trim material, a
//! jukebox song) carries the id that entry has in the game's own data pack;
//! the file's `server_registries` lists the components that applies to.
// TODO: resolve those by name against the connection's registries, so a
// server with other entries reads the right ones.

use std::sync::OnceLock;

use pomme_registry::Registry;
use pomme_registry::builtin::{DataComponentKind, ItemKind};
use serde_json::Value;

use crate::components::{Component, MaxStackSize};

const DATA: &str = include_str!("data/default-components.json");

/// Every item's defaults, by item id, each in component order.
type Table = Vec<Vec<(DataComponentKind, Box<[u8]>)>>;

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| parse(DATA).expect("the generated default components parse"))
}

fn parse(json: &str) -> Option<Table> {
    let root: Value = serde_json::from_str(json).ok()?;
    let values = root["values"].as_array()?;
    let values: Vec<Box<[u8]>> = values
        .iter()
        .map(|v| unhex(v.as_str()?))
        .collect::<Option<_>>()?;
    let items = root["items"].as_object()?;
    let mut table = vec![Vec::new(); ItemKind::ALL.len()];
    for (item, components) in items {
        let item: ItemKind = item.parse().ok()?;
        let entry = &mut table[item.to_u32() as usize];
        for (kind, index) in components.as_object()? {
            let value = values.get(usize::try_from(index.as_u64()?).ok()?)?;
            entry.push((kind.parse().ok()?, value.clone()));
        }
        entry.sort_by_key(|(kind, _)| *kind);
    }
    Some(table)
}

fn unhex(hex: &str) -> Option<Box<[u8]>> {
    let (pairs, rest) = hex.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

/// Parses the data now rather than on the first lookup, which would stall
/// whatever frame asked.
pub fn prewarm() {
    table();
}

/// `item`'s default components with their wire bytes, in component order.
pub fn of(item: ItemKind) -> &'static [(DataComponentKind, Box<[u8]>)] {
    &table()[item.to_u32() as usize]
}

/// The wire bytes of `item`'s default `kind`, if it has one.
pub fn get(item: ItemKind, kind: DataComponentKind) -> Option<&'static [u8]> {
    let defaults = of(item);
    let at = defaults.binary_search_by_key(&kind, |(k, _)| *k).ok()?;
    Some(&defaults[at].1)
}

/// What an item's defaults say about it, as methods of the item.
pub trait ItemKindExt {
    /// `Item.getDefaultMaxStackSize`: one for an item without the component.
    fn max_stack_size(&self) -> i32;
}

impl ItemKindExt for ItemKind {
    fn max_stack_size(&self) -> i32 {
        get(*self, MaxStackSize::KIND)
            .and_then(MaxStackSize::from_bytes)
            .map_or(1, |size| size.count)
    }
}

#[cfg(test)]
mod tests {
    use pomme_protocol::version::NATIVE;

    use super::*;

    #[test]
    fn the_data_is_the_native_versions_and_covers_every_item() {
        let root: Value = serde_json::from_str(DATA).unwrap();
        assert_eq!(root["version"], NATIVE.name);
        assert_eq!(
            root["items"].as_object().unwrap().len(),
            ItemKind::ALL.len()
        );
        // Every item has at least the components all items share.
        for &item in ItemKind::ALL {
            assert!(
                get(item, DataComponentKind::MaxStackSize).is_some(),
                "{item}"
            );
        }
    }

    #[test]
    fn values_are_the_games_own_bytes() {
        use DataComponentKind::{MaxDamage, MaxStackSize, ProvidesTrimMaterial};
        assert_eq!(get(ItemKind::Stone, MaxStackSize), Some(&[64][..]));
        assert_eq!(get(ItemKind::EnderPearl, MaxStackSize), Some(&[16][..]));
        // 250 as a varint.
        assert_eq!(
            get(ItemKind::IronPickaxe, MaxDamage),
            Some(&[0xfa, 0x01][..])
        );
        assert_eq!(get(ItemKind::Stone, MaxDamage), None);
        // A holder: the entry's id plus one, diamond third of the materials.
        assert_eq!(get(ItemKind::Diamond, ProvidesTrimMaterial), Some(&[3][..]));
    }

    #[test]
    fn an_items_stack_size_is_its_default_component() {
        assert_eq!(ItemKind::Stone.max_stack_size(), 64);
        assert_eq!(ItemKind::EnderPearl.max_stack_size(), 16);
        assert_eq!(ItemKind::IronPickaxe.max_stack_size(), 1);
    }

    #[test]
    fn only_these_components_lean_on_server_registries() {
        let root: Value = serde_json::from_str(DATA).unwrap();
        let bound: Vec<_> = root["server_registries"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(
            bound,
            [
                "chicken/variant",
                "damage_type",
                "instrument",
                "jukebox_playable",
                "provides_trim_material"
            ]
        );
    }
}
