use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::resource_pack::ResourcePackManager;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AssetId<'a> {
    pub namespace: &'a str,
    pub path: &'a str,
}

impl<'a> AssetId<'a> {
    pub fn parse(value: &'a str) -> Self {
        match value.split_once(':') {
            Some((namespace, path)) if !namespace.is_empty() && !path.is_empty() => {
                Self { namespace, path }
            }
            _ => Self {
                namespace: "minecraft",
                path: value,
            },
        }
    }

    pub fn asset_key(self, category: &str, suffix: &str) -> String {
        format!("{}/{category}/{}{}", self.namespace, self.path, suffix)
    }

    pub fn canonical(self) -> String {
        if self.namespace == "minecraft" {
            self.path.to_string()
        } else {
            format!("{}:{}", self.namespace, self.path)
        }
    }
}

pub(crate) fn valid_asset_key(asset_key: &str) -> bool {
    let Some((namespace, path)) = asset_key.split_once('/') else {
        return false;
    };
    if namespace.is_empty()
        || matches!(namespace, "." | "..")
        || !namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.-".contains(&byte)
        })
    {
        return false;
    }
    if path.is_empty()
        || path.contains('\\')
        || !path.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"/._-".contains(&byte)
        })
    {
        return false;
    }
    !path
        .split('/')
        .any(|component| component.is_empty() || matches!(component, "." | ".."))
}

/// Pomme's brand mark, embedded rather than resolved from the vanilla asset
/// tree: the window icon and the credits roll's logo both come from it.
pub const POMME_ICON_PNG: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icon.png"));

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

pub fn resource_stack_paths(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    asset_key: &str,
    packs: Option<&ResourcePackManager>,
) -> Vec<PathBuf> {
    if !valid_asset_key(asset_key) {
        tracing::warn!("Rejecting invalid Minecraft asset key {asset_key:?}");
        return Vec::new();
    }
    let mut stack = Vec::new();
    let jar = jar_assets_dir.join(asset_key);
    if jar.is_file() {
        stack.push(jar);
    }
    if let Some(path) = asset_index.as_ref().and_then(|idx| idx.resolve(asset_key)) {
        stack.push(path);
    }
    if let Some(packs) = packs {
        for root in packs.active_pack_dirs() {
            let path = root.join("assets").join(asset_key);
            if path.is_file() {
                stack.push(path);
            }
        }
    }
    stack
}

pub fn resolve_asset_path_with_packs(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    asset_key: &str,
    packs: Option<&ResourcePackManager>,
) -> PathBuf {
    if !valid_asset_key(asset_key) {
        tracing::warn!("Rejecting invalid Minecraft asset key {asset_key:?}");
        return jar_assets_dir.join("__invalid_asset_key__");
    }
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

    pub(crate) fn keys(&self) -> impl Iterator<Item = &str> {
        self.hashes.keys().map(String::as_str)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_keys_reject_traversal_and_invalid_resource_location_syntax() {
        for valid in [
            "minecraft/font/default.json",
            "example/textures/font/custom.png",
            "example/font/subdir/test.ttf",
        ] {
            assert!(valid_asset_key(valid), "expected valid asset key: {valid}");
        }
        for invalid in [
            "../outside",
            "minecraft/../outside",
            "minecraft/font/../../outside",
            "minecraft/./font/default.json",
            "minecraft//font/default.json",
            "minecraft/\\outside",
            "/minecraft/font/default.json",
            "Minecraft/font/default.json",
            "minecraft/CAPS.ttf",
            "minecraft/",
        ] {
            assert!(
                !valid_asset_key(invalid),
                "accepted invalid asset key: {invalid}"
            );
        }
    }

    #[test]
    fn invalid_asset_key_never_joins_outside_root() {
        let root = std::env::temp_dir().join(format!("pomme-assets-{}", uuid::Uuid::new_v4()));
        let resolved = resolve_asset_path_with_packs(&root, &None, "minecraft/../../outside", None);
        assert_eq!(resolved, root.join("__invalid_asset_key__"));
        assert!(resource_stack_paths(&root, &None, "minecraft/../../outside", None).is_empty());
    }
}
