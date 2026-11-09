use crate::recipe::{Ingredient, RecipeBookAddEntry, RecipeBookEntry, RecipeDisplay, SlotDisplay};
/// Builds a minimal recipe add event for collection and screen tests.
pub(super) fn recipe(id: u32, category: u32, requirements: Vec<Ingredient>) -> RecipeBookAddEntry {
    RecipeBookAddEntry {
        contents: RecipeBookEntry {
            id,
            display: RecipeDisplay::Shapeless {
                ingredients: vec![SlotDisplay::Item(1)],
                result: SlotDisplay::Item(2),
                crafting_station: SlotDisplay::Item(3),
            },
            group: None,
            category,
            crafting_requirements: Some(requirements),
        },
        highlight: false,
        notification: false,
    }
}
