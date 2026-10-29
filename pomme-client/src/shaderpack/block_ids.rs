//! `block.properties`: the id a pack's `mc_Entity` reads for each block state,
//! as Iris's `IdMap` and `BlockMaterialMapping` assign them.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use regex::Regex;

use crate::world::block::{BlockStateTable, BlockTags};

/// One `block.<id>` entry: a block or, with `%`, a block tag, and the state
/// properties it must have.
#[derive(Debug, PartialEq)]
struct Entry {
    tag: bool,
    namespace: String,
    name: String,
    properties: Vec<(String, String)>,
}

impl Entry {
    /// Iris's `BlockEntry.parse`.
    fn parse(text: &str) -> Self {
        let tag = text.starts_with('%');
        let text = text.replace('%', "");
        let parts: Vec<&str> = text.split(':').collect();
        let (namespace, name, states) = match parts.as_slice() {
            [name] => ("minecraft", *name, &[][..]),
            [namespace, name] if !name.contains('=') => (*namespace, *name, &[][..]),
            [name, states @ ..] if states[0].contains('=') => ("minecraft", *name, states),
            [namespace, name, states @ ..] => (*namespace, *name, states),
            [] => unreachable!("split yields at least one part"),
        };
        let properties = states
            .iter()
            .filter_map(|state| match state.split('=').collect::<Vec<_>>()[..] {
                [key, value] => Some((key.to_owned(), value.to_owned())),
                _ => {
                    tracing::warn!("block.properties: {text}: {state} isn't key=value");
                    None
                }
            })
            .collect();
        Self {
            tag,
            namespace: namespace.to_owned(),
            name: name.to_owned(),
            properties,
        }
    }
}

/// A pack's `block.properties` entries, by id in file order.
#[derive(Debug, Default)]
pub struct BlockIds(Vec<(i32, Vec<Entry>)>);

impl BlockIds {
    /// The preprocessed file, after Iris's fix-ups: a `\` continuation across
    /// a blank line joins, and a `block.` key starts its own line.
    pub fn parse(text: &str) -> Self {
        static CONTINUED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\\n\s*\n").unwrap());
        static INLINE_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\S *block\.").unwrap());
        let text = CONTINUED.replace_all(text, " ");
        let text = INLINE_KEY.replace_all(&text, "\nblock.");
        // A repeated key keeps its first place and takes its last value.
        let mut ids: Vec<(i32, &str)> = Vec::new();
        let entries = super::properties::parse_ordered(&text);
        for (key, value) in &entries {
            let Some(id) = key.strip_prefix("block.") else {
                continue;
            };
            let Ok(id) = id.parse() else {
                tracing::warn!("block.properties: invalid key {key}");
                continue;
            };
            match ids.iter_mut().find(|(i, _)| *i == id) {
                Some(slot) => slot.1 = value,
                None => ids.push((id, value)),
            }
        }
        Self(
            ids.into_iter()
                .map(|(id, value)| (id, value.split_whitespace().map(Entry::parse).collect()))
                .collect(),
        )
    }

    /// Each state's id: the first block entry naming its block whose
    /// properties it has, else the first such tag entry, else -1. A property
    /// the block lacks is ignored.
    fn state_ids(&self, table: BlockStateTable, tags: &BlockTags) -> Vec<i32> {
        let mut blocks: HashMap<&str, Vec<(i32, &Entry)>> = HashMap::new();
        let mut tag_entries = Vec::new();
        for (id, entries) in &self.0 {
            for entry in entries {
                if entry.tag {
                    tag_entries.push((*id, entry));
                } else if entry.namespace == "minecraft" {
                    blocks.entry(&entry.name).or_default().push((*id, entry));
                }
            }
        }
        (0..table.state_count() as u32)
            .filter_map(|id| table.try_state(id))
            .map(|state| {
                let data = table.block_state_data(state);
                let properties = data.properties();
                let matches = |entry: &Entry| {
                    entry.properties.iter().all(|(key, value)| {
                        properties.get(key).is_none_or(|actual| actual == value)
                    })
                };
                let block = blocks
                    .get(data.id())
                    .into_iter()
                    .flatten()
                    .find(|(_, entry)| matches(entry));
                let tag = || {
                    tag_entries.iter().find(|(_, entry)| {
                        tags.contains(&format!("{}:{}", entry.namespace, entry.name), data.id())
                            && matches(entry)
                    })
                };
                block.or_else(tag).map_or(-1, |(id, _)| *id)
            })
            .collect()
    }
}

/// The packed `mc_Entity` of every block state, for the mesher: Iris's
/// `(id + 1) << 1`, with the low bit set for fluid quads.
pub struct PackBlockIds {
    /// The entries these came from, to tell when a pack changes.
    pub source: Arc<BlockIds>,
    states: Vec<u32>,
}

impl PackBlockIds {
    pub fn new(source: Arc<BlockIds>, table: BlockStateTable, tags: &BlockTags) -> Self {
        let states = source
            .state_ids(table, tags)
            .into_iter()
            .map(|id| ((id + 1) as u32) << 1)
            .collect();
        Self { source, states }
    }

    pub fn block(&self, state: azalea_block::BlockState) -> u32 {
        self.states.get(state.id() as usize).copied().unwrap_or(0)
    }

    /// A fluid quad's: the id of the fluid's legacy block.
    pub fn fluid(&self, legacy_block: azalea_block::BlockState) -> u32 {
        self.block(legacy_block) | 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_parse_like_iris() {
        let entry = |text| Entry::parse(text);
        assert_eq!(entry("stone").name, "stone");
        assert_eq!(entry("mod:ore").namespace, "mod");
        let lower = entry("tall_grass:half=lower");
        assert_eq!(
            (lower.namespace.as_str(), lower.name.as_str()),
            ("minecraft", "tall_grass")
        );
        assert_eq!(lower.properties, [("half".to_owned(), "lower".to_owned())]);
        let tagged = entry("%minecraft:leaves:persistent=true");
        assert!(tagged.tag);
        assert_eq!(tagged.name, "leaves");
        assert_eq!(tagged.properties.len(), 1);
    }

    #[test]
    fn states_take_the_first_matching_id() {
        let ids = BlockIds::parse(
            "block.10=grass_block:snowy=false\nblock.20=grass_block stone\nblock.30=%minecraft:mineable/pickaxe\n",
        );
        let table = BlockStateTable::native();
        let state_ids = ids.state_ids(table, &BlockTags::default());
        let id =
            |name, props: &[(&str, &str)]| state_ids[table.find_state(name, props).id() as usize];
        assert_eq!(id("grass_block", &[("snowy", "false")]), 10);
        assert_eq!(id("grass_block", &[("snowy", "true")]), 20);
        assert_eq!(id("stone", &[]), 20);
        assert_eq!(id("dirt", &[]), -1);
    }

    #[test]
    fn inline_keys_and_repeats_follow_iris() {
        let ids = BlockIds::parse("block.1=dirt\nblock.2=stone block.3=sand\nblock.1=gravel\n");
        let ids: Vec<_> = ids.0.iter().map(|(id, e)| (*id, e.len())).collect();
        // The character before an inline `block.` key goes with the newline.
        assert_eq!(ids, [(1, 1), (2, 1), (3, 1)]);
    }
}
