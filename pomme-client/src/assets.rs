use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::resource_pack::ResourcePackManager;

pub fn load_image(path: &Path) -> Result<image::DynamicImage, image::ImageError> {
    image::open(path).or_else(|_| {
        let data = std::fs::read(path).map_err(image::ImageError::IoError)?;
        image::load_from_memory(&data)
    })
}

pub fn resolve_asset_path(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    asset_key: &str,
) -> PathBuf {
    resolve_asset_path_with_packs(jar_assets_dir, asset_index, asset_key, None)
}

pub fn resolve_asset_path_with_packs(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    asset_key: &str,
    packs: Option<&ResourcePackManager>,
) -> PathBuf {
    if let Some(packs) = packs
        && let Some(path) = packs.resolve_asset(asset_key)
    {
        return path;
    }
    if let Some(path) = asset_index.as_ref().and_then(|idx| idx.resolve(asset_key)) {
        return path;
    }
    jar_assets_dir.join(asset_key)
}

#[derive(Clone)]
pub struct AssetIndex {
    objects_dir: PathBuf,
    hashes: HashMap<String, String>,
}

impl AssetIndex {
    pub fn load(indexes_dir: &Path, objects_dir: &Path, version: &str) -> Option<Self> {
        let index_path = indexes_dir.join(format!("{version}.json"));

        let content = std::fs::read_to_string(&index_path)
            .map_err(|e| tracing::warn!("Failed to read asset index: {e}"))
            .ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| tracing::warn!("Failed to parse asset index: {e}"))
            .ok()?;

        let objects = parsed.get("objects")?.as_object()?;
        let hashes = objects
            .iter()
            .filter_map(|(k, v)| {
                let hash = v.get("hash")?.as_str()?;
                Some((k.clone(), hash.to_owned()))
            })
            .collect();

        Some(Self {
            objects_dir: objects_dir.to_path_buf(),
            hashes,
        })
    }

    pub fn resolve(&self, asset_key: &str) -> Option<PathBuf> {
        let hash = self.hashes.get(asset_key)?;
        let path = self.objects_dir.join(&hash[..2]).join(hash);
        path.exists().then_some(path)
    }
}
