use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(crate) use crate::world::block::model::DisplayTransform;
use crate::world::block::model::{first_item_model_ref, parse_display_transform, strip_mc_prefix};

const MODEL_PARENT_LIMIT: u32 = 16;

/// Per-item cache of one display transform, resolved from the item's model
/// JSON parent chain: the first model with any of `keys`, which it tries in
/// order (vanilla `ItemTransforms` falls back from a missing left-hand entry
/// to the same model's right-hand one).
pub struct DisplayResolver {
    keys: &'static [&'static str],
    cache: RefCell<HashMap<String, DisplayTransform>>,
    items_dir: PathBuf,
    models_dir: PathBuf,
}

impl DisplayResolver {
    pub fn new(jar_assets_dir: &Path, keys: &'static [&'static str]) -> Self {
        let mc_base = jar_assets_dir.join("minecraft");
        Self {
            keys,
            cache: RefCell::new(HashMap::new()),
            items_dir: mc_base.join("items"),
            models_dir: mc_base.join("models"),
        }
    }

    pub fn resolve(&self, item_name: &str, default: DisplayTransform) -> DisplayTransform {
        if let Some(t) = self.cache.borrow().get(item_name) {
            return *t;
        }
        let resolved = resolve_item_model_path(item_name, &self.items_dir)
            .and_then(|path| resolve_display(&path, &self.models_dir, self.keys))
            .unwrap_or(default);
        self.cache
            .borrow_mut()
            .insert(item_name.to_string(), resolved);
        resolved
    }
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

fn resolve_item_model_path(name: &str, items_dir: &Path) -> Option<String> {
    let item_json = read_json(&items_dir.join(format!("{name}.json")))?;
    first_item_model_ref(&item_json)
}

/// The first model up the parent chain with any of `keys` in its `display`,
/// and its transform for the first of them it has.
fn resolve_display(start_path: &str, models_dir: &Path, keys: &[&str]) -> Option<DisplayTransform> {
    let mut current = Some(start_path.to_string());
    let mut depth = 0u32;
    while let Some(path) = current.take() {
        if depth >= MODEL_PARENT_LIMIT {
            break;
        }
        depth += 1;

        let file = models_dir.join(format!("{path}.json"));
        let json = read_json(&file)?;

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
        let left = DisplayResolver::new(&dir, &["firstperson_lefthand", "firstperson_righthand"]);
        let resolved = left.resolve("thing", DisplayTransform::IDENTITY);
        assert_eq!(resolved.rotation, glam::Vec3::new(0.0, 10.0, 0.0));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
