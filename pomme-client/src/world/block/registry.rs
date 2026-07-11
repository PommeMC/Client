use std::collections::HashMap;
use std::path::Path;

use azalea_block::BlockState;
use serde::{Deserialize, Serialize};

pub const BLOCK_CACHE_FILE: &str = "block_cache_v2.json";

use super::model;
use super::model::BakedModel;
use crate::assets::AssetIndex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tint {
    None,
    Grass,
    Foliage,
    DryFoliage,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FaceTextures {
    pub top: String,
    pub bottom: String,
    pub north: String,
    pub south: String,
    pub east: String,
    pub west: String,
    pub side_overlay: Option<String>,
    pub tint: Tint,
    /// The model's `particle` texture slot (vanilla `getParticleMaterial`),
    /// used for block-break particles.
    #[serde(default)]
    pub particle: Option<String>,
}

impl FaceTextures {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        top: &str,
        bottom: &str,
        north: &str,
        south: &str,
        east: &str,
        west: &str,
        side_overlay: Option<&str>,
        tint: Tint,
    ) -> Self {
        Self {
            top: top.into(),
            bottom: bottom.into(),
            north: north.into(),
            south: south.into(),
            east: east.into(),
            west: west.into(),
            side_overlay: side_overlay.map(Into::into),
            tint,
            particle: None,
        }
    }

    pub fn uniform(name: &str, tint: Tint) -> Self {
        Self::new(name, name, name, name, name, name, None, tint)
    }
}

#[derive(Clone)]
pub struct BlockRegistry {
    textures: HashMap<String, FaceTextures>,
    baked: HashMap<String, HashMap<String, BakedModel>>,
    multipart: HashMap<String, Vec<model::MultipartEntry>>,
    item_models: HashMap<String, BakedModel>,
    flat_item_textures: std::collections::HashSet<String>,
    flat_item_texture_keys: HashMap<String, String>,
    /// Block name -> its single `BlockState`, for one-state blocks (see
    /// `placeable_block_for_item`).
    placeable_blocks: HashMap<&'static str, BlockState>,
}

impl BlockRegistry {
    pub fn load(
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        game_dir: &Path,
        packs: Option<&crate::resource_pack::ResourcePackManager>,
    ) -> Self {
        let cache_path = game_dir.join(BLOCK_CACHE_FILE);

        let textures = if packs.is_none() {
            if let Some(cached) = load_cache(&cache_path) {
                tracing::info!("Block registry: {} blocks (cached textures)", cached.len());
                Some(cached)
            } else {
                None
            }
        } else {
            None
        };

        let textures = textures.unwrap_or_else(|| {
            let mut textures = model::load_all_block_textures(jar_assets_dir, asset_index, packs);

            textures
                .entry("water".into())
                .or_insert_with(|| FaceTextures::uniform("water_still", Tint::None));
            textures
                .entry("lava".into())
                .or_insert_with(|| FaceTextures::uniform("lava_still", Tint::None));

            save_cache(&cache_path, &textures);
            tracing::info!(
                "Block registry: {} blocks (built and cached)",
                textures.len()
            );
            textures
        });

        let (baked, multipart) = model::bake_all_models(jar_assets_dir, asset_index, packs);
        let (item_models, flat_item_textures, flat_item_texture_keys) =
            model::bake_item_models(jar_assets_dir, asset_index, packs);

        Self {
            textures,
            baked,
            multipart,
            item_models,
            flat_item_textures,
            flat_item_texture_keys,
            placeable_blocks: build_placeable_blocks(),
        }
    }

    /// Resolves a held item's registry name (unprefixed, e.g. `"stone"`) to the
    /// `BlockState` to predict on placement, or `None` if the item is not a
    /// single-state block. Item and block share a registry name for this set.
    pub fn placeable_block_for_item(&self, item_name: &str) -> Option<BlockState> {
        self.placeable_blocks.get(item_name).copied()
    }

    pub fn get_item_model(&self, name: &str) -> Option<&BakedModel> {
        self.item_models.get(name)
    }

    pub fn flat_item_textures(&self) -> impl Iterator<Item = &str> + '_ {
        self.flat_item_textures.iter().map(String::as_str)
    }

    pub fn get_flat_item_texture_key(&self, name: &str) -> Option<&str> {
        self.flat_item_texture_keys.get(name).map(String::as_str)
    }

    pub fn get_textures(&self, state: BlockState) -> Option<&FaceTextures> {
        self.textures.get(super::block_id(state))
    }

    pub fn get_baked_model(&self, state: BlockState) -> Option<&BakedModel> {
        let variants = self.baked.get(super::block_id(state))?;

        if variants.len() == 1 {
            return variants.values().next();
        }

        // Vanilla variant keys only list the properties that affect the model, so
        // match by subset rather than exact string equality (an empty key matches
        // any state, serving as the default variant).
        let props = super::block_properties(state);
        variants
            .iter()
            .find(|(key, _)| {
                constraints_match(props, key.split(',').filter_map(|p| p.split_once('=')))
            })
            .map(|(_, model)| model)
            .or_else(|| variants.values().next())
    }

    pub fn get_multipart_quads(&self, state: BlockState) -> Option<Vec<&model::BakedQuad>> {
        let entries = self.multipart.get(super::block_id(state))?;
        let props = super::block_properties(state);

        let mut quads = Vec::new();
        for entry in entries {
            let when = entry.when.iter().map(|(k, v)| (k.as_str(), v.as_str()));
            if constraints_match(props, when) {
                quads.extend(entry.quads.iter());
            }
        }

        if quads.is_empty() { None } else { Some(quads) }
    }

    fn baked_model_flag(&self, state: BlockState, f: impl Fn(&BakedModel) -> bool) -> bool {
        if super::is_air(state) {
            return false;
        }
        self.get_baked_model(state).map(f).unwrap_or(false)
    }

    pub fn is_opaque_full_cube(&self, state: BlockState) -> bool {
        self.baked_model_flag(state, |m| m.is_full_cube)
    }

    /// Whether `state` culls a neighbor's adjacent face. Unlike
    /// [`Self::is_opaque_full_cube`], non-occluding blocks like leaves return
    /// false even though they bake as full cubes.
    pub fn occludes_neighbor(&self, state: BlockState) -> bool {
        self.baked_model_flag(state, |m| m.occludes)
    }

    pub fn texture_names(&self) -> impl Iterator<Item = &str> + '_ {
        let face_textures = self.textures.values().flat_map(|ft| {
            let base = [
                &ft.top, &ft.bottom, &ft.north, &ft.south, &ft.east, &ft.west,
            ];
            base.into_iter()
                .map(|s| s.as_str())
                .chain(ft.side_overlay.as_deref())
        });

        let baked_textures = self.baked.values().flat_map(|variants| {
            variants
                .values()
                .flat_map(|model| model.quads.iter().map(|q| q.texture.as_str()))
        });

        let multipart_textures = self.multipart.values().flat_map(|entries| {
            entries
                .iter()
                .flat_map(|e| e.quads.iter().map(|q| q.texture.as_str()))
        });

        let item_model_textures = self
            .item_models
            .values()
            .flat_map(|model| model.quads.iter().map(|q| q.texture.as_str()));

        face_textures
            .chain(baked_textures)
            .chain(multipart_textures)
            .chain(item_model_textures)
    }
}

/// Builds the block-name -> single-`BlockState` map from the block table,
/// keeping only names that map to exactly one state.
fn build_placeable_blocks() -> HashMap<&'static str, BlockState> {
    let mut seen: HashMap<&'static str, Option<BlockState>> = HashMap::new();
    for (state, data) in super::all_states() {
        seen.entry(data.id)
            .and_modify(|v| *v = None)
            .or_insert(Some(state));
    }
    seen.into_iter()
        .filter_map(|(name, state)| state.map(|s| (name, s)))
        .collect()
}

/// Whether every `key=value` constraint holds for `props`. A value may list
/// alternatives separated by `|`, as vanilla multipart `when` clauses do.
fn constraints_match<'a>(
    props: &super::PropMap,
    mut constraints: impl Iterator<Item = (&'a str, &'a str)>,
) -> bool {
    constraints.all(|(k, v)| {
        props
            .get(k)
            .is_some_and(|pv| v.split('|').any(|opt| opt == pv))
    })
}

fn load_cache(path: &Path) -> Option<HashMap<String, FaceTextures>> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_cache(path: &Path, textures: &HashMap<String, FaceTextures>) {
    if let Ok(json) = serde_json::to_string(textures)
        && let Err(e) = std::fs::write(path, json)
    {
        tracing::warn!("Failed to write block cache: {e}");
    }
}
