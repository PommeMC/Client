use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use crate::resources::{ResourceContent, ResourceId};
pub(crate) use crate::world::block::model::DisplayTransform;
use crate::world::block::model::{
    ModelInputs, first_item_model_ref, parse_display_transform, read_model_json, strip_mc_prefix,
    using_item_model_ref,
};

const MODEL_PARENT_LIMIT: u32 = 16;

/// `display` keys tried in order: vanilla `ItemTransforms` falls back from a
/// missing left-hand entry to the same model's right-hand one.
pub type DisplayKeys = &'static [&'static str];

/// An item's resolved `(in_use, keys)` lookups; `None` when no model has
/// the keys.
type ItemDisplays = HashMap<(bool, DisplayKeys), Option<DisplayTransform>>;

/// Per-item cache of display transforms, each resolved from the item's model
/// JSON parent chain: the first model with any of the lookup's keys.
pub struct DisplayResolver {
    cache: RefCell<HashMap<String, ItemDisplays>>,
    items: BTreeMap<ResourceId, ResourceContent>,
    models: BTreeMap<ResourceId, ResourceContent>,
}

impl DisplayResolver {
    pub fn new(inputs: &ModelInputs) -> Self {
        Self {
            cache: RefCell::new(HashMap::new()),
            items: inputs.items.clone(),
            models: inputs.models.clone(),
        }
    }

    /// The transform for `item_name` under `keys`, from its `using_item`
    /// model while it's `in_use`.
    pub fn resolve(
        &self,
        item_name: &str,
        in_use: bool,
        keys: DisplayKeys,
        default: DisplayTransform,
    ) -> DisplayTransform {
        let lookup = (in_use, keys);
        if let Some(t) = self
            .cache
            .borrow()
            .get(item_name)
            .and_then(|c| c.get(&lookup))
        {
            return t.unwrap_or(default);
        }
        let resolved = resolve_item_model_path(item_name, in_use, &self.items)
            .and_then(|path| resolve_display(&path, &self.models, keys));
        self.cache
            .borrow_mut()
            .entry(item_name.to_owned())
            .or_default()
            .insert(lookup, resolved);
        resolved.unwrap_or(default)
    }
}

fn read_json(content: &ResourceContent) -> Option<serde_json::Value> {
    let s = read_model_json(content)?;
    serde_json::from_str(&s).ok()
}

fn resolve_item_model_path(
    name: &str,
    in_use: bool,
    items: &BTreeMap<ResourceId, ResourceContent>,
) -> Option<String> {
    let id = ResourceId::from_parts("minecraft", &format!("items/{name}.json")).ok()?;
    let item_json = read_json(items.get(&id)?)?;
    in_use
        .then(|| using_item_model_ref(&item_json))
        .flatten()
        .or_else(|| first_item_model_ref(&item_json))
}

/// The first model up the parent chain with any of `keys` in its `display`,
/// and its transform for the first of them it has.
fn resolve_display(
    start_path: &str,
    models: &BTreeMap<ResourceId, ResourceContent>,
    keys: &[&str],
) -> Option<DisplayTransform> {
    let mut current = Some(start_path.to_string());
    let mut depth = 0u32;
    while let Some(path) = current.take() {
        if depth >= MODEL_PARENT_LIMIT {
            break;
        }
        depth += 1;

        let id = ResourceId::from_parts("minecraft", &format!("models/{path}.json")).ok()?;
        let json = read_json(models.get(&id)?)?;

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
            .map(|p| strip_mc_prefix(p).to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_left_hand_falls_back_to_the_same_models_right_hand() {
        let dir = crate::test_util::test_temp_dir("display_fallback");
        let mc = dir.join("assets/minecraft");
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
        let resources = crate::test_util::test_resources(std::slice::from_ref(&dir));
        let inputs = ModelInputs::prepare(&resources).unwrap();
        let resolved = DisplayResolver::new(&inputs).resolve(
            "thing",
            false,
            &["firstperson_lefthand", "firstperson_righthand"],
            DisplayTransform::IDENTITY,
        );
        assert_eq!(resolved.rotation, glam::Vec3::new(0.0, 10.0, 0.0));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
