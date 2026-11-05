//! Every blockstate, model and item definition JSON a version's jar ships,
//! read in one go and kept as one file in the version's cache.
//!
//! Pomme-only: vanilla reads these out of its one jar, where Pomme reads the
//! extracted copy. Opening those ~6,000 small files one by one on a cold
//! start took 13-14 s on Windows (measured; each first open is scanned),
//! against 0.4 s warm. The first start reads them on every core, later ones
//! read the bundle.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::render_assets::{LoadProgress, LoadStage, load_cache, save_cache};
use crate::assets::AssetIndex;

const BUNDLE_FILE: &str = "asset_json_v1.json";

#[derive(Serialize, Deserialize)]
pub struct JsonBundle {
    /// Blockstate file stems and contents, in name order.
    blockstates: Vec<(String, String)>,
    /// Item definition names (`minecraft/items/*.json` stems), in name order.
    items: Vec<String>,
    /// Model and item definition contents by asset key
    /// (`minecraft/models/block/stone.json`). Keys the asset index resolves
    /// are left out, so they still resolve through it.
    files: HashMap<String, String>,
}

impl JsonBundle {
    /// The cached bundle, else one read from the jar's extracted assets and
    /// cached (when `cache_dir` is given).
    pub fn load(
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        cache_dir: Option<&Path>,
        progress: Option<&LoadProgress>,
    ) -> Self {
        let path = cache_dir.map(|dir| dir.join(BUNDLE_FILE));
        if let Some(bundle) = path.as_deref().and_then(load_cache) {
            return bundle;
        }
        let bundle = Self::read_assets(jar_assets_dir, asset_index, progress);
        if let Some(path) = &path
            && !bundle.blockstates.is_empty()
        {
            save_cache(path, &bundle);
        }
        bundle
    }

    fn read_assets(
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        progress: Option<&LoadProgress>,
    ) -> Self {
        let blockstate_paths: Vec<(String, PathBuf)> =
            super::model::resolve_blockstates_dir(jar_assets_dir, asset_index, None)
                .map(|dir| json_files(&dir))
                .unwrap_or_default();
        let items: Vec<String> = super::model::item_definition_names(jar_assets_dir, None)
            .into_iter()
            .collect();

        let mut keyed: Vec<(String, PathBuf)> = Vec::new();
        collect_models(
            &jar_assets_dir.join("minecraft/models"),
            "minecraft/models",
            &mut keyed,
        );
        keyed.extend(items.iter().map(|name| {
            let key = format!("minecraft/items/{name}.json");
            let path = jar_assets_dir.join(&key);
            (key, path)
        }));
        keyed.retain(|(key, _)| {
            asset_index
                .as_ref()
                .is_none_or(|index| index.resolve(key).is_none())
        });

        let paths: Vec<&Path> = blockstate_paths
            .iter()
            .map(|(_, path)| path.as_path())
            .chain(keyed.iter().map(|(_, path)| path.as_path()))
            .collect();
        LoadProgress::begin_stage(progress, LoadStage::Files, paths.len());
        let mut contents = crate::util::parallel_map(&paths, |path| {
            let contents = std::fs::read_to_string(path).ok();
            LoadProgress::step(progress);
            contents
        })
        .into_iter();

        let blockstates = blockstate_paths
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

/// The `.json` files directly in `dir`, by stem, in name order.
fn json_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path.file_name()?.to_str()?.strip_suffix(".json")?;
            Some((stem.to_owned(), path))
        })
        .collect();
    files.sort();
    files
}

fn collect_models(dir: &Path, key_prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let key = format!("{key_prefix}/{name}");
        if entry.file_type().is_ok_and(|ty| ty.is_dir()) {
            collect_models(&path, &key, out);
        } else if name.ends_with(".json") {
            out.push((key, path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_holds_blockstates_models_and_items_by_key() {
        let root = std::env::temp_dir().join(format!("pomme-json-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mc = root.join("minecraft");
        for dir in ["blockstates", "models/block", "items"] {
            std::fs::create_dir_all(mc.join(dir)).unwrap();
        }
        std::fs::write(mc.join("blockstates/stone.json"), "{\"variants\":{}}").unwrap();
        std::fs::write(mc.join("blockstates/dirt.json"), "{}").unwrap();
        std::fs::write(mc.join("blockstates/notes.txt"), "x").unwrap();
        std::fs::write(mc.join("models/block/stone.json"), "{\"parent\":\"a\"}").unwrap();
        std::fs::write(mc.join("items/stone.json"), "{\"model\":{}}").unwrap();

        let cache = root.join("cache");
        let built = JsonBundle::load(&root, &None, Some(&cache), None);
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
        let cached = JsonBundle::load(&root, &None, Some(&cache), None);
        assert_eq!(cached.blockstates().len(), 2);
        assert_eq!(
            cached.get("minecraft/models/block/stone.json"),
            Some("{\"parent\":\"a\"}")
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
