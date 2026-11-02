//! Selected blockstate, model and item definition JSON, read in parallel.
//! Only the builtin-only view is kept in the version's shared cache.
//!
//! Pomme-only: vanilla reads these out of its one jar, where Pomme reads the
//! extracted copy. Opening those ~6,000 small files one by one on a cold
//! start took 13-14 s on Windows (measured; each first open is scanned),
//! against 0.4 s warm. The first start reads them on every core, later ones
//! read the bundle.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::model::{ModelInputs, read_model_json};
use super::render_assets::{LoadProgress, LoadStage, load_cache, save_cache};
use crate::resources::ResourceContent;

const BUNDLE_FILE: &str = "asset_json_v1.json";

#[derive(Serialize, Deserialize)]
pub struct JsonBundle {
    /// Blockstate file stems and contents, in name order.
    blockstates: Vec<(String, String)>,
    /// Item definition names (`minecraft/items/*.json` stems), in name order.
    items: Vec<String>,
    /// Selected model and item contents by asset key
    /// (`minecraft/models/block/stone.json`).
    files: HashMap<String, String>,
}

impl JsonBundle {
    /// The cached bundle, else one read from source-pinned selections.
    /// Callers supply `cache_dir` only for the builtin-only active view.
    pub fn load(
        inputs: &ModelInputs,
        cache_dir: Option<&Path>,
        progress: Option<&LoadProgress>,
    ) -> Self {
        let path = cache_dir.map(|dir| dir.join(BUNDLE_FILE));
        if let Some(bundle) = path.as_deref().and_then(load_cache) {
            return bundle;
        }
        let bundle = Self::read_assets(inputs, progress);
        if let Some(path) = &path
            && !bundle.blockstates.is_empty()
        {
            save_cache(path, &bundle);
        }
        bundle
    }

    fn read_assets(inputs: &ModelInputs, progress: Option<&LoadProgress>) -> Self {
        // Keep every blockstate layer in low-to-high order, but only winning
        // models/items. Enumeration and filters have already run on the owner.
        let blockstate_inputs: Vec<(String, &ResourceContent)> = inputs
            .blockstates
            .iter()
            .filter_map(|(id, stack)| {
                let name = id
                    .path()
                    .strip_prefix("blockstates/")?
                    .strip_suffix(".json")?;
                Some((name, stack))
            })
            .flat_map(|(name, stack)| stack.iter().map(move |content| (name.to_owned(), content)))
            .collect();
        let items = inputs
            .items
            .keys()
            .filter_map(|id| {
                Some(
                    id.path()
                        .strip_prefix("items/")?
                        .strip_suffix(".json")?
                        .to_owned(),
                )
            })
            .collect();
        let keyed: Vec<(String, &ResourceContent)> = inputs
            .models
            .iter()
            .chain(inputs.items.iter())
            .map(|(id, content)| (format!("{}/{}", id.namespace(), id.path()), content))
            .collect();
        let selected: Vec<&ResourceContent> = blockstate_inputs
            .iter()
            .map(|(_, content)| *content)
            .chain(keyed.iter().map(|(_, content)| *content))
            .collect();
        LoadProgress::begin_stage(progress, LoadStage::Files, selected.len());
        let mut contents = crate::util::parallel_map(&selected, |content| {
            let contents = read_model_json(content);
            LoadProgress::step(progress);
            contents
        })
        .into_iter();

        let blockstates = blockstate_inputs
            .into_iter()
            .zip(contents.by_ref())
            .filter_map(|((name, _), text)| Some((name, text?)))
            .collect();
        let files = keyed
            .into_iter()
            .zip(contents)
            .filter_map(|((key, _), text)| Some((key, text?)))
            .collect();
        Self {
            blockstates,
            items,
            files,
        }
    }

    pub(super) fn blockstates(&self) -> &[(String, String)] {
        &self.blockstates
    }

    pub(super) fn item_names(&self) -> &[String] {
        &self.items
    }

    pub(super) fn get(&self, asset_key: &str) -> Option<&str> {
        self.files.get(asset_key).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_holds_blockstates_models_and_items_by_key() {
        let root = std::env::temp_dir().join(format!("pomme-json-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mc = root.join("assets/minecraft");
        for dir in ["blockstates", "models/block", "items"] {
            std::fs::create_dir_all(mc.join(dir)).unwrap();
        }
        std::fs::write(mc.join("blockstates/stone.json"), "{\"variants\":{}}").unwrap();
        std::fs::write(mc.join("blockstates/dirt.json"), "{}").unwrap();
        std::fs::write(mc.join("blockstates/notes.txt"), "x").unwrap();
        std::fs::write(mc.join("models/block/stone.json"), "{\"parent\":\"a\"}").unwrap();
        std::fs::write(mc.join("items/stone.json"), "{\"model\":{}}").unwrap();

        let cache = root.join("cache");
        let resources = crate::test_util::test_resources(std::slice::from_ref(&root));
        let inputs = ModelInputs::prepare(&resources).unwrap();
        let built = JsonBundle::load(&inputs, Some(&cache), None);
        let names: Vec<_> = built
            .blockstates()
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(names, ["dirt", "stone"]);
        assert_eq!(built.item_names(), ["stone"]);
        assert_eq!(
            built.get("minecraft/models/block/stone.json"),
            Some("{\"parent\":\"a\"}")
        );
        assert_eq!(
            built.get("minecraft/items/stone.json"),
            Some("{\"model\":{}}")
        );

        // A second load reads the cached file, not the assets.
        std::fs::remove_dir_all(&mc).unwrap();
        let cached = JsonBundle::load(&inputs, Some(&cache), None);
        assert_eq!(cached.blockstates().len(), 2);
        assert_eq!(
            cached.get("minecraft/models/block/stone.json"),
            Some("{\"parent\":\"a\"}")
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
