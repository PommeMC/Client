use std::cell::RefCell;
use std::collections::HashMap;

pub(crate) use crate::world::block::model::DisplayTransform;
use crate::world::block::model::{
    ItemDefinitionSources, ModelSources, first_item_model_ref, parse_display_transform,
};

const MODEL_PARENT_LIMIT: u32 = 16;

/// Per-item cache of one display transform, resolved from the item's model
/// JSON parent chain: the first model with any of `keys`, which it tries in
/// order (vanilla `ItemTransforms` falls back from a missing left-hand entry
/// to the same model's right-hand one).
pub struct DisplayResolver {
    keys: &'static [&'static str],
    cache: RefCell<HashMap<String, DisplayTransform>>,
    models: ModelSources,
    items: ItemDefinitionSources,
}

impl DisplayResolver {
    pub(crate) fn new(
        keys: &'static [&'static str],
        models: ModelSources,
        items: ItemDefinitionSources,
    ) -> Self {
        Self {
            keys,
            cache: RefCell::new(HashMap::new()),
            models,
            items,
        }
    }

    pub(crate) fn replace_definition_sources(
        &mut self,
        models: ModelSources,
        items: ItemDefinitionSources,
    ) {
        self.models = models;
        self.items = items;
        self.cache.get_mut().clear();
    }

    pub fn resolve(&self, item_name: &str, default: DisplayTransform) -> DisplayTransform {
        if let Some(t) = self.cache.borrow().get(item_name) {
            return *t;
        }
        let resolved = self
            .items
            .item_json(item_name)
            .and_then(|json| first_item_model_ref(&json))
            .and_then(|path| resolve_display(&path, &self.models, self.keys))
            .unwrap_or(default);
        self.cache
            .borrow_mut()
            .insert(item_name.to_string(), resolved);
        resolved
    }
}

/// The first model up the parent chain with any of `keys` in its `display`,
/// and its transform for the first of them it has.
fn resolve_display(
    start_path: &str,
    models: &ModelSources,
    keys: &[&str],
) -> Option<DisplayTransform> {
    let mut current = Some(start_path.to_string());
    let mut depth = 0u32;
    while let Some(path) = current.take() {
        if depth >= MODEL_PARENT_LIMIT {
            break;
        }
        depth += 1;

        let json = models.model_json(&path)?;

        if let Some(display) = json.get("display")
            && let Some(t) = keys
                .iter()
                .find_map(|key| display.get(key).and_then(parse_display_transform))
        {
            return Some(t);
        }

        current = json
            .get("parent")
            .and_then(|p| p.as_str())
            .map(str::to_string);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_left_hand_falls_back_to_the_same_models_right_hand() {
        let dir = crate::test_util::test_temp_dir("display_fallback");
        let mc = dir.join("minecraft");
        std::fs::create_dir_all(mc.join("items")).unwrap();
        std::fs::create_dir_all(mc.join("models/item")).unwrap();
        let write = |path: &str, json: &str| std::fs::write(mc.join(path), json).unwrap();
        write(
            "items/thing.json",
            r#"{"model": {"type": "minecraft:model", "model": "minecraft:item/thing"}}"#,
        );
        write(
            "models/item/thing.json",
            r#"{"parent": "item/base", "display": {"firstperson_righthand": {"rotation": [0, 10, 0]}}}"#,
        );
        write(
            "models/item/base.json",
            r#"{"display": {"firstperson_lefthand": {"rotation": [0, 99, 0]}}}"#,
        );
        let models = ModelSources::from_test_files(&[
            ("item/thing", mc.join("models/item/thing.json")),
            ("item/base", mc.join("models/item/base.json")),
        ]);
        let left = DisplayResolver::new(
            &["firstperson_lefthand", "firstperson_righthand"],
            models,
            ItemDefinitionSources::from_test_files(&[("thing", mc.join("items/thing.json"))]),
        );
        let resolved = left.resolve("thing", DisplayTransform::IDENTITY);
        assert_eq!(resolved.rotation, glam::Vec3::new(0.0, 10.0, 0.0));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
