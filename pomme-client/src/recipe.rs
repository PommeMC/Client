use std::collections::{HashMap, HashSet};

/// A recipe display id from the post-1.21.2 recipe protocol.
pub type RecipeDisplayId = u32;

/// Pomme-owned view of a recipe ingredient holder set.
#[derive(Clone, Debug, PartialEq)]
pub enum Ingredient {
    Items(Vec<u32>),
    Tag(String),
}

/// Pomme-owned recipe display tree. Item stacks remain the client's existing
/// inventory leaf type until the generic item-component codec migration lands.
#[derive(Clone, Debug, PartialEq)]
pub enum RecipeDisplay {
    Shapeless {
        ingredients: Vec<SlotDisplay>,
        result: SlotDisplay,
        crafting_station: SlotDisplay,
    },
    Shaped {
        width: u32,
        height: u32,
        ingredients: Vec<SlotDisplay>,
        result: SlotDisplay,
        crafting_station: SlotDisplay,
    },
    Furnace {
        ingredient: SlotDisplay,
        fuel: SlotDisplay,
        result: SlotDisplay,
        crafting_station: SlotDisplay,
        duration: u32,
        experience: f32,
    },
    Stonecutter {
        input: SlotDisplay,
        result: SlotDisplay,
        crafting_station: SlotDisplay,
    },
    Smithing {
        template: SlotDisplay,
        base: SlotDisplay,
        addition: SlotDisplay,
        result: SlotDisplay,
        crafting_station: SlotDisplay,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemStackTemplate {
    pub item: u32,
    pub count: i32,
    pub components: azalea_inventory::DataComponentPatch,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TrimPatternHolder {
    Reference(u32),
    Direct {
        asset_id: String,
        description: azalea_chat::FormattedText,
        decal: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum SlotDisplay {
    Empty,
    AnyFuel,
    WithAnyPotion(Box<SlotDisplay>),
    OnlyWithComponent {
        contents: Box<SlotDisplay>,
        component: u32,
    },
    Item(u32),
    ItemStack(ItemStackTemplate),
    Tag(String),
    Dyed {
        dye: Box<SlotDisplay>,
        target: Box<SlotDisplay>,
    },
    SmithingTrim {
        base: Box<SlotDisplay>,
        material: Box<SlotDisplay>,
        trim_pattern: TrimPatternHolder,
    },
    WithRemainder {
        input: Box<SlotDisplay>,
        remainder: Box<SlotDisplay>,
    },
    Composite(Vec<SlotDisplay>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecipeBookEntry {
    pub id: RecipeDisplayId,
    pub display: RecipeDisplay,
    /// `None` is vanilla's OptionalInt.empty; otherwise this is the decoded
    /// group id, not the wire's +1 sentinel representation.
    pub group: Option<u32>,
    /// Built-in `recipe_book_category` registry id.
    pub category: u32,
    pub crafting_requirements: Option<Vec<Ingredient>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecipeBookAddEntry {
    pub contents: RecipeBookEntry,
    pub highlight: bool,
    pub notification: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecipeBookTypeSettings {
    pub open: bool,
    pub filtering: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecipeBookSettings {
    pub crafting: RecipeBookTypeSettings,
    pub furnace: RecipeBookTypeSettings,
    pub blast_furnace: RecipeBookTypeSettings,
    pub smoker: RecipeBookTypeSettings,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeBookType {
    Crafting,
    Furnace,
    BlastFurnace,
    Smoker,
}

#[allow(dead_code)]
impl RecipeBookSettings {
    pub fn get(self, kind: RecipeBookType) -> RecipeBookTypeSettings {
        match kind {
            RecipeBookType::Crafting => self.crafting,
            RecipeBookType::Furnace => self.furnace,
            RecipeBookType::BlastFurnace => self.blast_furnace,
            RecipeBookType::Smoker => self.smoker,
        }
    }

    pub fn set(&mut self, kind: RecipeBookType, settings: RecipeBookTypeSettings) {
        *match kind {
            RecipeBookType::Crafting => &mut self.crafting,
            RecipeBookType::Furnace => &mut self.furnace,
            RecipeBookType::BlastFurnace => &mut self.blast_furnace,
            RecipeBookType::Smoker => &mut self.smoker,
        } = settings;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StonecutterRecipe {
    pub input: Ingredient,
    pub option_display: SlotDisplay,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecipeData {
    /// `RecipePropertySet` values keyed by their resource location.
    pub item_sets: HashMap<String, Vec<u32>>,
    pub stonecutter_recipes: Vec<StonecutterRecipe>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GhostRecipe {
    pub container_id: i32,
    pub recipe: RecipeDisplay,
}

/// Item tags synchronized by the server, normalized to native item ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemTags {
    tags: HashMap<String, HashSet<u32>>,
    ordered: HashMap<String, Vec<u32>>,
}

#[allow(dead_code)]
impl ItemTags {
    pub fn from_entries(entries: impl IntoIterator<Item = (String, Vec<u32>)>) -> Self {
        let mut tags = HashMap::new();
        let mut ordered = HashMap::new();
        for (name, items) in entries {
            let mut seen = HashSet::new();
            let mut sequence = Vec::new();
            for item in items {
                if seen.insert(item) {
                    sequence.push(item);
                }
            }
            tags.insert(name.clone(), seen);
            ordered.insert(name, sequence);
        }
        Self { tags, ordered }
    }

    pub fn contains(&self, tag: &str, item: u32) -> bool {
        self.tags
            .get(tag)
            .is_some_and(|items| items.contains(&item))
    }

    pub fn get(&self, tag: &str) -> Option<&HashSet<u32>> {
        self.tags.get(tag)
    }

    pub fn ordered(&self, tag: &str) -> Option<&[u32]> {
        self.ordered.get(tag).map(Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.tags.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }
}

/// Per-session recipe state matching vanilla's `ClientRecipeBook` plus the
/// recipe container and last ghost placement delivered by the server.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecipeBookState {
    pub known: HashMap<RecipeDisplayId, RecipeBookEntry>,
    /// Insertion order of keys in `known`. Vanilla stores recipes in a Java
    /// `HashMap`; its iteration order is bucket order, with insertion order
    /// preserved within each bucket. We keep key insertion order so the UI can
    /// reproduce that iteration exactly when rebuilding collections.
    known_insertion_order: Vec<RecipeDisplayId>,
    /// Emulated backing-table capacity of Vanilla's Java `HashMap`.
    /// Java grows this at 0.75 load and never shrinks on remove/clear, which
    /// affects `values()` bucket iteration after a grow-then-reduce history.
    known_table_capacity: usize,
    pub highlight: HashSet<RecipeDisplayId>,
    pub settings: RecipeBookSettings,
    pub data: RecipeData,
    pub item_tags: ItemTags,
    pub ghost_recipe: Option<GhostRecipe>,
}

impl RecipeBookState {
    pub fn apply_add(&mut self, entries: Vec<RecipeBookAddEntry>, replace: bool) {
        if replace {
            self.known.clear();
            self.known_insertion_order.clear();
            self.highlight.clear();
        }
        for entry in entries {
            let id = entry.contents.id;
            if !self.known.contains_key(&id) {
                if self.known_table_capacity == 0 {
                    self.known_table_capacity = 16;
                }
                self.known_insertion_order.push(id);
                self.known.insert(id, entry.contents);
                while self.known.len() > self.known_table_capacity * 3 / 4 {
                    self.known_table_capacity *= 2;
                }
            } else {
                self.known.insert(id, entry.contents);
            }
            if entry.highlight {
                self.highlight.insert(id);
            }
        }
    }

    pub fn remove(&mut self, ids: impl IntoIterator<Item = RecipeDisplayId>) {
        for id in ids {
            self.known.remove(&id);
            self.highlight.remove(&id);
            self.known_insertion_order.retain(|known| *known != id);
        }
    }

    /// Entries in the order vanilla's `HashMap<RecipeDisplayId, ...>.values()`
    /// iterates them. Java's `HashMap` walks buckets from low to high and keeps
    /// insertion order within each bucket. `RecipeDisplayId` hashes as its
    /// integer id, then `HashMap` applies `h ^ (h >>> 16)`.
    pub fn known_in_vanilla_order(&self) -> Vec<&RecipeBookEntry> {
        if self.known.is_empty() {
            return Vec::new();
        }

        let capacity = self.known_table_capacity.max(16);
        let mask = (capacity - 1) as u32;
        let mut buckets = vec![Vec::new(); capacity];
        for id in &self.known_insertion_order {
            let Some(entry) = self.known.get(id) else {
                continue;
            };
            let hash = *id ^ (*id >> 16);
            buckets[(hash & mask) as usize].push(entry);
        }
        buckets.into_iter().flatten().collect()
    }

    #[allow(dead_code)]
    pub fn mark_seen(&mut self, id: RecipeDisplayId) {
        self.highlight.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u32) -> RecipeBookAddEntry {
        RecipeBookAddEntry {
            contents: RecipeBookEntry {
                id,
                display: RecipeDisplay::Shapeless {
                    ingredients: Vec::new(),
                    result: SlotDisplay::Empty,
                    crafting_station: SlotDisplay::Empty,
                },
                group: None,
                category: 0,
                crafting_requirements: None,
            },
            highlight: true,
            notification: false,
        }
    }

    #[test]
    fn replace_and_remove_match_vanilla_recipe_book_semantics() {
        let mut state = RecipeBookState::default();
        state.apply_add(vec![entry(1), entry(2)], false);
        assert_eq!(state.known.len(), 2);
        assert_eq!(state.highlight.len(), 2);

        state.apply_add(vec![entry(3)], true);
        assert_eq!(state.known.keys().copied().collect::<Vec<_>>(), [3]);
        assert_eq!(state.highlight.iter().copied().collect::<Vec<_>>(), [3]);

        state.remove([3]);
        assert!(state.known.is_empty());
        assert!(state.highlight.is_empty());
    }

    #[test]
    fn marking_seen_only_clears_highlight() {
        let mut state = RecipeBookState::default();
        state.apply_add(vec![entry(7)], false);
        state.mark_seen(7);
        assert!(state.known.contains_key(&7));
        assert!(!state.highlight.contains(&7));
    }

    #[test]
    fn known_order_matches_java_hash_map_bucket_iteration() {
        let mut state = RecipeBookState::default();
        state.apply_add(vec![entry(17), entry(1), entry(16), entry(2)], false);
        assert_eq!(
            state
                .known_in_vanilla_order()
                .into_iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            vec![16, 17, 1, 2]
        );

        state.remove([17]);
        state.apply_add(vec![entry(17)], false);
        assert_eq!(
            state
                .known_in_vanilla_order()
                .into_iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            vec![16, 1, 17, 2]
        );
    }

    #[test]
    fn java_hash_map_capacity_history_survives_remove_and_replace() {
        let mut state = RecipeBookState::default();
        let mut entries = (0..13).map(entry).collect::<Vec<_>>();
        entries[0] = entry(17);
        entries[1] = entry(1);
        state.apply_add(entries, false);
        assert_eq!(state.known_table_capacity, 32);

        let remove = state
            .known
            .keys()
            .copied()
            .filter(|id| *id != 17 && *id != 1)
            .collect::<Vec<_>>();
        state.remove(remove);
        assert_eq!(
            state
                .known_in_vanilla_order()
                .into_iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            vec![1, 17]
        );

        state.apply_add(vec![entry(17), entry(1)], true);
        assert_eq!(state.known_table_capacity, 32);
        assert_eq!(
            state
                .known_in_vanilla_order()
                .into_iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            vec![1, 17]
        );
    }

    #[test]
    fn item_tags_are_resolved_sets() {
        let tags = ItemTags::from_entries([("minecraft:planks".into(), vec![2, 1, 2])]);
        assert_eq!(tags.len(), 1);
        assert!(tags.contains("minecraft:planks", 1));
        assert!(tags.contains("minecraft:planks", 2));
        assert!(!tags.contains("minecraft:planks", 3));
        assert_eq!(tags.ordered("minecraft:planks"), Some(&[2, 1][..]));
    }
}
