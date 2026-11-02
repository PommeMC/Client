//! Recipe grouping, search, and craftability matching.
use super::*;
pub(super) fn build_collections(
    book: &RecipeBookState,
    spec: RecipeBookScreenSpec,
) -> Vec<Collection> {
    let mut grouped = Vec::<(usize, Vec<RecipeBookEntry>)>::new();
    let mut grouped_index = HashMap::<(u32, u32), usize>::new();
    for entry in book.known_in_vanilla_order() {
        let Some(rank) = category_rank(spec.kind, entry.category) else {
            continue;
        };
        match entry.group {
            Some(group) => match grouped_index.entry((entry.category, group)) {
                std::collections::hash_map::Entry::Occupied(slot) => {
                    grouped[*slot.get()].1.push(entry.clone());
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(grouped.len());
                    grouped.push((rank, vec![entry.clone()]));
                }
            },
            None => grouped.push((rank, vec![entry.clone()])),
        }
    }
    grouped.sort_by_key(|(rank, _)| *rank);
    grouped
        .into_iter()
        .map(|(_, entries)| Collection {
            fitting: entries
                .iter()
                .filter(|entry| can_display(spec, &entry.display))
                .map(|entry| entry.id)
                .collect(),
            craftable: HashSet::new(),
            entries,
        })
        .filter(|collection| !collection.fitting.is_empty())
        .collect()
}

/// Vanilla `selectMatchingRecipes`: a fitting recipe is craftable when the
/// available items cover its requirements.
pub(super) fn select_matching_recipes(
    collections: &mut [Collection],
    available: &HashMap<u32, u32>,
    tags: &ItemTags,
) {
    for collection in collections {
        collection.craftable = collection
            .entries
            .iter()
            .filter(|entry| collection.fitting.contains(&entry.id))
            .filter(|entry| can_craft(entry, available, tags))
            .map(|entry| entry.id)
            .collect();
    }
}

/// Position of `category` among `kind`'s tabs, which is also vanilla's
/// search-tab concatenation order (`SearchRecipeBookCategory`); `None` for
/// another book's category.
pub(super) fn category_rank(kind: RecipeBookType, category: u32) -> Option<usize> {
    tabs_for(kind)
        .iter()
        .filter_map(|tab| tab.category)
        .position(|candidate| candidate == category)
}

/// One recipe result's search text (`SessionSearchTrees`): its tooltip
/// lines and item id path, lowercased.
pub(super) struct SearchText {
    path: String,
    lines: Vec<String>,
}

impl SearchText {
    /// Vanilla `FullTextSearchTree.search`: plain text matches tooltip
    /// lines; `namespace:path` intersects the namespace with path-or-name.
    pub(super) fn matches(&self, needle: &str) -> bool {
        let contains = |needle: &str| self.lines.iter().any(|line| line.contains(needle));
        match needle.split_once(':') {
            Some((namespace, path)) => {
                let path = path.trim();
                "minecraft".contains(namespace.trim())
                    && (self.path.contains(path) || contains(path))
            }
            None => contains(needle),
        }
    }
}

/// Search text for every result of every recipe in `collection`, not just
/// the fitting ones (`collection.getRecipes()`).
pub(super) fn collection_search_text(
    collection: &Collection,
    book: &RecipeBookState,
) -> Vec<SearchText> {
    collection
        .entries
        .iter()
        .flat_map(|entry| display_result_stacks(&entry.display, book))
        .map(|stack| SearchText {
            path: crate::player::inventory::item_resource_name(stack.stack.kind).to_lowercase(),
            lines: stack_tooltip_lines(&stack)
                .into_iter()
                .map(|line| strip_formatting(&line_text(line)).trim().to_lowercase())
                .filter(|line| !line.is_empty())
                .collect(),
        })
        .collect()
}

pub(super) fn line_text(line: TooltipLine) -> String {
    line.spans.into_iter().map(|span| span.text).collect()
}

/// Vanilla `ChatFormatting.stripFormatting`: drops `§` codes.
pub(super) fn strip_formatting(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '§'
            && chars.peek().is_some_and(
                |code| matches!(code.to_ascii_lowercase(), '0'..='9' | 'a'..='f' | 'k'..='o' | 'r'),
            )
        {
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// Vanilla `RecipeDisplayEntry.canCraft`: needs requirements, each taking one
/// available item.
pub(super) fn can_craft(
    entry: &RecipeBookEntry,
    available: &HashMap<u32, u32>,
    tags: &ItemTags,
) -> bool {
    let Some(requirements) = entry.crafting_requirements.as_ref() else {
        return false;
    };
    let options = requirements
        .iter()
        .map(|ingredient| ingredient_items(ingredient, tags).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    can_match(&options, available)
}

/// Vanilla `StackedContents.RecipePicker.tryPick(1)`: whether every
/// ingredient gets one of its items, no item used beyond its count. Bipartite
/// matching by augmenting paths, polynomial where trying every assignment is
/// exponential.
pub(super) fn can_match(options: &[Vec<u32>], available: &HashMap<u32, u32>) -> bool {
    let mut holders = HashMap::<u32, Vec<usize>>::new();
    (0..options.len()).all(|ingredient| {
        augment(
            ingredient,
            options,
            available,
            &mut holders,
            &mut HashSet::new(),
        )
    })
}

/// Gives `ingredient` an item, moving earlier holders elsewhere if needed.
pub(super) fn augment(
    ingredient: usize,
    options: &[Vec<u32>],
    available: &HashMap<u32, u32>,
    holders: &mut HashMap<u32, Vec<usize>>,
    visited: &mut HashSet<u32>,
) -> bool {
    for &item in &options[ingredient] {
        let capacity = available.get(&item).copied().unwrap_or(0) as usize;
        if capacity == 0 || !visited.insert(item) {
            continue;
        }
        let held = holders.entry(item).or_default();
        if held.len() < capacity {
            held.push(ingredient);
            return true;
        }
        for slot in 0..held.len() {
            let other = holders[&item][slot];
            if augment(other, options, available, holders, visited) {
                holders.get_mut(&item).expect("item has holders")[slot] = ingredient;
                return true;
            }
        }
    }
    false
}

/// Vanilla `RecipeButton.allRecipesHaveSameResultDisplay`.
pub(super) fn all_recipes_have_same_result(
    entries: &[(RecipeDisplayId, Vec<ResolvedSlotStack>)],
) -> bool {
    let mut items = entries.iter().flat_map(|(_, items)| items);
    let Some(first) = items.next() else {
        return true;
    };
    items.all(|item| first.same_item_and_components(item))
}

/// Vanilla `fillGhostRecipe` for each screen: slot positions in main-panel
/// GUI units with their resolved alternatives. The fuel ghost only fills an
/// empty fuel slot.
pub(super) fn can_display(spec: RecipeBookScreenSpec, display: &RecipeDisplay) -> bool {
    match (spec.kind, display) {
        (RecipeBookType::Crafting, RecipeDisplay::Shaped { width, height, .. }) => {
            *width as usize <= spec.grid_width && *height as usize <= spec.grid_height
        }
        (RecipeBookType::Crafting, RecipeDisplay::Shapeless { ingredients, .. }) => {
            ingredients.len() <= spec.grid_width * spec.grid_height
        }
        (
            RecipeBookType::Furnace | RecipeBookType::BlastFurnace | RecipeBookType::Smoker,
            RecipeDisplay::Furnace { .. },
        ) => true,
        _ => false,
    }
}

pub(super) fn available_items(
    slots: &[ItemStack],
    spec: RecipeBookScreenSpec,
) -> HashMap<u32, u32> {
    let mut counts = HashMap::new();
    match spec.craftability_source {
        CraftabilitySource::PlayerInventory => {
            // InventoryMenu: 2x2 crafting inputs plus the player's 36 inventory
            // items. Armor, offhand, and the crafting result are excluded.
            for index in (1..5).chain(9..45) {
                account_simple_stack(slots.get(index), &mut counts);
            }
        }
        CraftabilitySource::CraftingTable => {
            // CraftingMenu: 3x3 crafting inputs 1..9 and player inventory 10..45.
            for index in 1..46 {
                account_simple_stack(slots.get(index), &mut counts);
            }
        }
        CraftabilitySource::Furnace => {
            // AbstractFurnaceMenu delegates its three-slot SimpleContainer to
            // accountStack (components do not disqualify these), then adds the
            // player's inventory with accountSimpleStack.
            for index in 0..3 {
                account_stack(slots.get(index), &mut counts);
            }
            for index in 3..39 {
                account_simple_stack(slots.get(index), &mut counts);
            }
        }
    }
    counts
}

pub(super) fn account_simple_stack(stack: Option<&ItemStack>, counts: &mut HashMap<u32, u32>) {
    let Some(ItemStack::Present(data)) = stack else {
        return;
    };
    if !is_usable_for_crafting(data) {
        return;
    }
    account_stack_data(data, counts);
}

pub(super) fn account_stack(stack: Option<&ItemStack>, counts: &mut HashMap<u32, u32>) {
    let Some(ItemStack::Present(data)) = stack else {
        return;
    };
    account_stack_data(data, counts);
}

pub(super) fn account_stack_data(data: &ItemStackData, counts: &mut HashMap<u32, u32>) {
    if data.count <= 0 {
        return;
    }
    let count = data.count.min(data.kind.max_stack_size()) as u32;
    *counts.entry(data.kind.to_u32()).or_default() += count;
}

pub(super) fn is_usable_for_crafting(data: &ItemStackData) -> bool {
    let damaged = data
        .get_component::<Damage>()
        .is_some_and(|damage| damage.amount > 0);
    let enchanted = data
        .get_component::<Enchantments>()
        .is_some_and(|enchantments| !enchantments.levels.is_empty());
    let named = data.get_component::<CustomName>().is_some();
    !damaged && !enchanted && !named
}

#[derive(Clone)]
pub(super) struct Collection {
    /// Every recipe of the group (`RecipeCollection.getRecipes`).
    pub(super) entries: Vec<RecipeBookEntry>,
    /// Recipes that fit this screen (vanilla's `selected`).
    pub(super) fitting: HashSet<RecipeDisplayId>,
    pub(super) craftable: HashSet<RecipeDisplayId>,
}

impl Collection {
    pub(super) fn category(&self) -> u32 {
        self.entries[0].category
    }

    pub(super) fn in_tab(&self, category: Option<u32>) -> bool {
        category.is_none_or(|category| self.category() == category)
    }

    /// Vanilla `getSelectedRecipes(CRAFTABLE)` when filtering, else `ANY`.
    pub(super) fn shown(&self, filtering: bool) -> Vec<&RecipeBookEntry> {
        self.entries
            .iter()
            .filter(|entry| self.fitting.contains(&entry.id))
            .filter(|entry| !filtering || self.craftable.contains(&entry.id))
            .collect()
    }

    pub(super) fn has_craftable(&self) -> bool {
        !self.craftable.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{Ingredient, RecipeBookEntry, RecipeDisplay, SlotDisplay};
    use crate::ui::recipe_book::test_support::recipe;
    #[test]
    fn craftability_respects_duplicate_ingredient_counts() {
        let entry = recipe(
            1,
            0,
            vec![Ingredient::Items(vec![5]), Ingredient::Items(vec![5])],
        )
        .contents;
        let tags = ItemTags::default();
        assert!(!can_craft(&entry, &HashMap::from([(5, 1)]), &tags));
        assert!(can_craft(&entry, &HashMap::from([(5, 2)]), &tags));
    }
    #[test]
    fn craftability_backtracks_across_alternatives() {
        let entry = recipe(
            1,
            0,
            vec![Ingredient::Items(vec![5, 6]), Ingredient::Items(vec![5])],
        )
        .contents;
        assert!(can_craft(
            &entry,
            &HashMap::from([(5, 1), (6, 1)]),
            &ItemTags::default()
        ));
    }
    #[test]
    fn missing_crafting_requirements_are_never_craftable() {
        let mut entry = recipe(1, 0, vec![Ingredient::Items(vec![5])]).contents;
        entry.crafting_requirements = None;
        assert!(!can_craft(
            &entry,
            &HashMap::from([(5, 64)]),
            &ItemTags::default()
        ));
    }
    #[test]
    fn player_craftability_excludes_armor_offhand_and_damaged_stacks() {
        use azalea_inventory::DataComponentPatch;
        use azalea_inventory::components::{Damage, DataComponentUnion};
        use azalea_registry::builtin::{DataComponentKind, ItemKind};

        let diamond = ItemKind::Diamond.to_u32();
        let mut slots = vec![ItemStack::Empty; 46];
        slots[5] = ItemStack::Present(ItemStackData::new(ItemKind::Diamond, 2));
        slots[45] = ItemStack::Present(ItemStackData::new(ItemKind::Diamond, 3));
        assert!(!available_items(&slots, RecipeBookScreenSpec::player(0)).contains_key(&diamond));

        let mut patch = DataComponentPatch::default();
        unsafe {
            patch.unchecked_insert_component(
                DataComponentKind::Damage,
                Some(DataComponentUnion::from(Damage { amount: 1 })),
            );
        }
        slots[9] = ItemStack::Present(ItemStackData {
            kind: ItemKind::Diamond,
            count: 4,
            component_patch: patch,
        });
        assert!(!available_items(&slots, RecipeBookScreenSpec::player(0)).contains_key(&diamond));

        slots[9] = ItemStack::Present(ItemStackData::new(ItemKind::Diamond, 4));
        assert_eq!(
            available_items(&slots, RecipeBookScreenSpec::player(0)).get(&diamond),
            Some(&4)
        );
    }
    #[test]
    fn furnace_craftability_counts_output_container_stack() {
        use azalea_registry::builtin::ItemKind;

        let diamond = ItemKind::Diamond.to_u32();
        let mut slots = vec![ItemStack::Empty; 39];
        slots[2] = ItemStack::Present(ItemStackData::new(ItemKind::Diamond, 7));
        assert_eq!(
            available_items(
                &slots,
                RecipeBookScreenSpec::furnace(1, RecipeBookType::Furnace)
            )
            .get(&diamond),
            Some(&7)
        );
    }
    #[test]
    fn player_grid_rejects_three_wide_recipe() {
        let spec = RecipeBookScreenSpec::player(0);
        let display = RecipeDisplay::Shaped {
            width: 3,
            height: 1,
            ingredients: vec![SlotDisplay::Item(1); 3],
            result: SlotDisplay::Item(2),
            crafting_station: SlotDisplay::Item(3),
        };
        assert!(!can_display(spec, &display));
        assert!(can_display(
            RecipeBookScreenSpec::crafting_table(1),
            &display
        ));
    }
    #[test]
    fn recipe_collections_group_only_matching_group_and_category() {
        let mut book = RecipeBookState::default();
        let mut a = recipe(1, 0, vec![Ingredient::Items(vec![5])]);
        let mut b = recipe(2, 0, vec![Ingredient::Items(vec![6])]);
        let mut c = recipe(3, 1, vec![Ingredient::Items(vec![7])]);
        a.contents.group = Some(42);
        b.contents.group = Some(42);
        c.contents.group = Some(42);
        book.apply_add(vec![a, b, c], false);
        let grouped = build_collections(&book, RecipeBookScreenSpec::crafting_table(1));
        let mut sizes = grouped.iter().map(|c| c.entries.len()).collect::<Vec<_>>();
        sizes.sort_unstable();
        assert_eq!(sizes, vec![1, 2]);
    }
    #[test]
    fn crafting_search_collections_follow_vanilla_category_order() {
        let mut book = RecipeBookState::default();
        book.apply_add(
            (1..=4)
                .map(|id| recipe(id, id - 1, vec![Ingredient::Items(vec![1])]))
                .collect(),
            false,
        );
        let grouped = build_collections(&book, RecipeBookScreenSpec::crafting_table(1));
        let categories = grouped.iter().map(Collection::category).collect::<Vec<_>>();
        assert_eq!(categories, vec![2, 0, 3, 1]);
    }
    #[test]
    fn collections_keep_recipes_that_do_not_fit_for_search() {
        let mut book = RecipeBookState::default();
        let mut small = recipe(1, 0, vec![Ingredient::Items(vec![1])]);
        let mut wide = recipe(2, 0, vec![Ingredient::Items(vec![1])]);
        small.contents.group = Some(7);
        wide.contents.group = Some(7);
        wide.contents.display = RecipeDisplay::Shaped {
            width: 3,
            height: 1,
            ingredients: vec![SlotDisplay::Item(1); 3],
            result: SlotDisplay::Item(2),
            crafting_station: SlotDisplay::Empty,
        };
        book.apply_add(vec![small, wide], false);
        let grouped = build_collections(&book, RecipeBookScreenSpec::player(0));
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].entries.len(), 2);
        assert_eq!(grouped[0].fitting, HashSet::from([1]));
        assert_eq!(grouped[0].shown(false).len(), 1);
    }
    #[test]
    fn search_matches_vanilla_plain_and_identifier_modes_with_components() {
        use azalea_chat::FormattedText;
        use azalea_inventory::DataComponentPatch;
        use azalea_inventory::components::{CustomName, DataComponentUnion, Lore};
        use azalea_registry::builtin::{DataComponentKind, ItemKind};

        let mut patch = DataComponentPatch::default();
        unsafe {
            patch.unchecked_insert_component(
                DataComponentKind::CustomName,
                Some(DataComponentUnion::from(CustomName {
                    name: FormattedText::from("Fancy Gem"),
                })),
            );
            patch.unchecked_insert_component(
                DataComponentKind::Lore,
                Some(DataComponentUnion::from(Lore {
                    lines: vec![FormattedText::from("Secret Recipe")],
                })),
            );
        }
        let entry = RecipeBookEntry {
            id: 9,
            display: RecipeDisplay::Shapeless {
                ingredients: Vec::new(),
                result: SlotDisplay::ItemStack(crate::recipe::ItemStackTemplate {
                    item: ItemKind::Diamond.to_u32(),
                    count: 1,
                    components: patch,
                }),
                crafting_station: SlotDisplay::Empty,
            },
            group: None,
            category: 0,
            crafting_requirements: None,
        };
        let book = RecipeBookState::default();
        let collection = Collection {
            fitting: HashSet::from([entry.id]),
            craftable: HashSet::new(),
            entries: vec![entry],
        };
        let texts = collection_search_text(&collection, &book);
        let matches = |needle: &str| texts.iter().any(|text| text.matches(needle));

        assert!(matches("fancy"));
        assert!(matches("secret"));
        assert!(!matches("diamond"));
        assert!(!matches(" fancy"));
        assert!(matches(" minecraft : diamond "));
    }
    /// The exhaustive assignment `can_match` replaces, as an oracle.
    fn brute_force_match(options: &[Vec<u32>], remaining: &mut HashMap<u32, u32>) -> bool {
        let Some((first, rest)) = options.split_first() else {
            return true;
        };
        first.iter().any(|item| {
            let Some(count) = remaining.get_mut(item).filter(|count| **count > 0) else {
                return false;
            };
            *count -= 1;
            let found = brute_force_match(rest, remaining);
            *remaining.get_mut(item).expect("item stays") += 1;
            found
        })
    }
    #[test]
    fn craftability_matching_agrees_with_exhaustive_assignment() {
        let mut rng = fastrand::Rng::with_seed(11);
        for _ in 0..2_000 {
            let options = (0..rng.usize(0..6))
                .map(|_| {
                    (0..rng.usize(0..4))
                        .map(|_| rng.u32(0..5))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let available = (0..5)
                .filter_map(|item| {
                    let count = rng.u32(0..3);
                    (count > 0).then_some((item, count))
                })
                .collect::<HashMap<_, _>>();
            assert_eq!(
                can_match(&options, &available),
                brute_force_match(&options, &mut available.clone()),
                "{options:?} with {available:?}"
            );
        }
    }
    #[test]
    fn a_chest_from_mixed_planks_needs_eight_planks() {
        let planks = (100..108).collect::<Vec<u32>>();
        let options = vec![planks.clone(); 8];
        let one_each = planks.iter().map(|&p| (p, 1)).collect::<HashMap<_, _>>();
        assert!(can_match(&options, &one_each));
        let seven = planks[..7]
            .iter()
            .map(|&p| (p, 1))
            .collect::<HashMap<_, _>>();
        assert!(!can_match(&options, &seven));
    }
    #[test]
    fn empty_requirements_are_craftable() {
        let entry = recipe(1, 0, Vec::new()).contents;
        assert!(can_craft(&entry, &HashMap::new(), &ItemTags::default()));
    }
}
