use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use glam::{Mat3, Mat4, Quat, Vec3};
use serde::Deserialize;

use super::registry::{FaceTextures, Tint};
use crate::assets::{AssetId, AssetIndex, resolve_asset_path_with_packs};

#[derive(Deserialize)]
struct BlockstateFile {
    variants: Option<HashMap<String, VariantEntry>>,
    multipart: Option<Vec<MultipartCase>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum VariantEntry {
    Single(ModelRef),
    Array(Vec<ModelRef>),
}

impl VariantEntry {
    fn first(&self) -> Option<&ModelRef> {
        match self {
            VariantEntry::Single(r) => Some(r),
            VariantEntry::Array(arr) => arr.first(),
        }
    }
}

#[derive(Deserialize)]
struct MultipartCase {
    apply: MultipartApply,
    #[allow(dead_code)]
    when: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MultipartApply {
    Single(ModelRef),
    Array(Vec<ModelRef>),
}

impl MultipartApply {
    fn first(&self) -> Option<&ModelRef> {
        match self {
            MultipartApply::Single(r) => Some(r),
            MultipartApply::Array(arr) => arr.first(),
        }
    }
}

#[derive(Deserialize)]
struct ModelRef {
    model: String,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    uvlock: bool,
}

#[derive(Deserialize, Default, Clone)]
struct ModelFile {
    parent: Option<String>,
    #[serde(default, deserialize_with = "deserialize_texture_map")]
    textures: HashMap<String, String>,
    #[serde(default)]
    elements: Vec<ElementDef>,
    #[serde(default)]
    display: HashMap<String, serde_json::Value>,
    gui_light: Option<GuiLight>,
}

#[derive(Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum GuiLight {
    Front,
    #[default]
    Side,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DisplayTransform {
    pub(crate) rotation: Vec3,
    pub(crate) translation: Vec3,
    pub(crate) scale: Vec3,
}

impl DisplayTransform {
    pub(crate) const IDENTITY: Self = Self {
        rotation: Vec3::ZERO,
        translation: Vec3::ZERO,
        scale: Vec3::ONE,
    };

    pub(crate) fn to_matrix(self) -> Mat4 {
        Mat4::from_translation(self.translation)
            * Mat4::from_rotation_x(self.rotation.x.to_radians())
            * Mat4::from_rotation_y(self.rotation.y.to_radians())
            * Mat4::from_rotation_z(self.rotation.z.to_radians())
            * Mat4::from_scale(self.scale)
    }
}

pub(crate) fn parse_display_transform(json: &serde_json::Value) -> Option<DisplayTransform> {
    let obj = json.as_object()?;
    let rotation = obj
        .get("rotation")
        .map(|value| parse_vec3(value, Vec3::ZERO))
        .unwrap_or(Vec3::ZERO);
    let translation = obj
        .get("translation")
        .map(|value| parse_vec3(value, Vec3::ZERO))
        .unwrap_or(Vec3::ZERO)
        * (1.0 / 16.0);
    let scale = obj
        .get("scale")
        .map(|value| parse_vec3(value, Vec3::ONE))
        .unwrap_or(Vec3::ONE);
    Some(DisplayTransform {
        rotation,
        translation: translation.clamp(Vec3::splat(-5.0), Vec3::splat(5.0)),
        scale: scale.clamp(Vec3::splat(-4.0), Vec3::splat(4.0)),
    })
}

/// The shape vanilla's `display.gui` entries share, which differ only in yaw:
/// `block/block` is 225, `item/template_chest` 45.
#[cfg(test)]
fn gui_transform(y_rot_deg: f32) -> DisplayTransform {
    DisplayTransform {
        rotation: Vec3::new(30.0, y_rot_deg, 0.0),
        translation: Vec3::ZERO,
        scale: Vec3::splat(0.625),
    }
}

pub(crate) fn default_block_ground_transform() -> Mat4 {
    DisplayTransform {
        rotation: Vec3::ZERO,
        translation: Vec3::new(0.0, 3.0 / 16.0, 0.0),
        scale: Vec3::splat(0.25),
    }
    .to_matrix()
}

fn deserialize_texture_map<'de, D>(de: D) -> Result<HashMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let raw: HashMap<String, serde_json::Value> = HashMap::deserialize(de)?;
    let mut out = HashMap::new();
    for (k, v) in raw {
        if let Some(s) = v.as_str() {
            out.insert(k, s.to_string());
        } else if let Some(sprite) = v.get("sprite").and_then(serde_json::Value::as_str) {
            out.insert(k, sprite.to_string());
        }
    }
    Ok(out)
}

#[derive(Deserialize, Clone)]
struct ElementDef {
    from: [f32; 3],
    to: [f32; 3],
    #[serde(default)]
    rotation: Option<ElementRotation>,
    #[serde(default)]
    faces: HashMap<String, FaceDef>,
    #[serde(default = "default_true")]
    shade: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Clone)]
struct ElementRotation {
    origin: [f32; 3],
    axis: String,
    angle: f32,
    #[serde(default)]
    rescale: bool,
}

#[derive(Deserialize, Clone)]
struct FaceDef {
    uv: Option<[f32; 4]>,
    texture: String,
    cullface: Option<String>,
    #[serde(default)]
    rotation: Option<i32>,
    #[serde(rename = "tintindex")]
    tint_index: Option<i32>,
}

#[derive(Clone, Debug, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Direction {
    pub fn offset(&self) -> [i32; 3] {
        match self {
            Direction::Down => [0, -1, 0],
            Direction::Up => [0, 1, 0],
            Direction::North => [0, 0, -1],
            Direction::South => [0, 0, 1],
            Direction::West => [-1, 0, 0],
            Direction::East => [1, 0, 0],
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "down" => Some(Direction::Down),
            "up" => Some(Direction::Up),
            "north" => Some(Direction::North),
            "south" => Some(Direction::South),
            "west" => Some(Direction::West),
            "east" => Some(Direction::East),
            _ => None,
        }
    }

    fn rotate_y(self, degrees: i32) -> Self {
        let steps = degrees.rem_euclid(360) / 90;
        let mut d = self;
        for _ in 0..steps {
            d = match d {
                Direction::North => Direction::East,
                Direction::East => Direction::South,
                Direction::South => Direction::West,
                Direction::West => Direction::North,
                other => other,
            };
        }
        d
    }

    fn rotate_x(self, degrees: i32) -> Self {
        let steps = degrees.rem_euclid(360) / 90;
        let mut d = self;
        for _ in 0..steps {
            d = match d {
                Direction::North => Direction::Down,
                Direction::Down => Direction::South,
                Direction::South => Direction::Up,
                Direction::Up => Direction::North,
                other => other,
            };
        }
        d
    }

    /// The default table's shade, for the item paths that bake it in; terrain
    /// looks its dimension's table up at mesh time instead.
    pub(crate) fn shade_light(&self) -> f32 {
        CardinalLighting::DEFAULT.by_face(*self)
    }
}

/// Vanilla `CardinalLighting`: the per-face brightness a dimension shades
/// block faces with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardinalLighting {
    pub down: f32,
    pub up: f32,
    pub north: f32,
    pub south: f32,
    pub west: f32,
    pub east: f32,
}

impl CardinalLighting {
    pub const DEFAULT: Self = Self {
        down: 0.5,
        up: 1.0,
        north: 0.8,
        south: 0.8,
        west: 0.6,
        east: 0.6,
    };
    pub const NETHER: Self = Self {
        down: 0.9,
        up: 0.9,
        north: 0.8,
        south: 0.8,
        west: 0.6,
        east: 0.6,
    };

    pub fn by_face(&self, dir: Direction) -> f32 {
        match dir {
            Direction::Down => self.down,
            Direction::Up => self.up,
            Direction::North => self.north,
            Direction::South => self.south,
            Direction::West => self.west,
            Direction::East => self.east,
        }
    }
}

/// A dimension type's `cardinal_light`, vanilla `CardinalLighting.Type`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CardinalLightType {
    #[default]
    Default,
    Nether,
}

impl CardinalLightType {
    pub fn table(self) -> CardinalLighting {
        match self {
            Self::Default => CardinalLighting::DEFAULT,
            Self::Nether => CardinalLighting::NETHER,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BakedQuad {
    pub positions: [[f32; 3]; 4],
    pub uvs: [[f32; 2]; 4],
    pub texture: String,
    pub cullface: Option<Direction>,
    pub tint: super::registry::Tint,
    /// The default table's shade, for GUI and held items.
    pub shade_light: f32,
    /// The face terrain shades this quad as, `None` for `shade: false`.
    pub shade_face: Option<Direction>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BakedModel {
    pub quads: Vec<BakedQuad>,
    pub is_full_cube: bool,
    /// Vanilla `canOcclude`: full cubes occlude, but cutout blocks like leaves
    /// don't, so neighbor faces against them still render.
    pub occludes: bool,
}

#[derive(Clone)]
pub struct MultipartEntry {
    pub when: HashMap<String, String>,
    pub quads: Vec<BakedQuad>,
}

const FOLIAGE_TINTED: &[&str] = &[
    "oak_leaves",
    "dark_oak_leaves",
    "jungle_leaves",
    "acacia_leaves",
    "mangrove_leaves",
    "vine",
];

const DRY_FOLIAGE_TINTED: &[&str] = &["leaf_litter"];

const GRASS_TINTED: &[&str] = &[
    "grass_block",
    "grass",
    "short_grass",
    "tall_grass",
    "fern",
    "large_fern",
];

pub fn load_all_block_textures(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&crate::resource_pack::ResourcePackManager>,
) -> HashMap<String, FaceTextures> {
    let mut results = HashMap::new();
    let mut model_cache = HashMap::new();

    for_each_blockstate(
        jar_assets_dir,
        asset_index,
        packs,
        |block_name, blockstate| {
            let model_ref = extract_default_model_ref(blockstate)?;
            let resolved = resolve_model(
                &model_ref.model,
                jar_assets_dir,
                asset_index,
                &mut model_cache,
                packs,
            );
            let face_textures = build_face_textures(block_name, &resolved.textures)?;
            results.insert(block_name.to_string(), face_textures);
            Some(())
        },
    );

    tracing::info!(
        "Loaded {} block texture mappings from vanilla assets",
        results.len()
    );
    results
}

type BakedModelMap = HashMap<String, HashMap<String, BakedModel>>;
type MultipartMap = HashMap<String, Vec<MultipartEntry>>;

pub fn bake_all_models(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&crate::resource_pack::ResourcePackManager>,
) -> (BakedModelMap, MultipartMap) {
    let mut results: HashMap<String, HashMap<String, BakedModel>> = HashMap::new();
    let mut multipart_results: HashMap<String, Vec<MultipartEntry>> = HashMap::new();
    let mut model_cache = HashMap::new();
    let mut total = 0u32;

    for_each_blockstate(
        jar_assets_dir,
        asset_index,
        packs,
        |block_name, blockstate| {
            total += 1;
            let block_tint = determine_tint(block_name);
            let mut variants_map: HashMap<String, BakedModel> = HashMap::new();

            if let Some(variants) = &blockstate.variants {
                for (variant_key, variant_entry) in variants {
                    let model_ref = variant_entry.first()?;
                    let resolved = resolve_model(
                        &model_ref.model,
                        jar_assets_dir,
                        asset_index,
                        &mut model_cache,
                        packs,
                    );
                    if let Some(mut baked) =
                        bake_resolved_model(&resolved, model_ref.x, model_ref.y, block_tint)
                    {
                        if is_non_occluding(block_name) {
                            baked.occludes = false;
                        }
                        variants_map.insert(variant_key.clone(), baked);
                    }
                }
            } else if let Some(multipart) = &blockstate.multipart {
                let mut entries = Vec::new();
                for case in multipart {
                    let model_ref = case.apply.first()?;
                    let resolved = resolve_model(
                        &model_ref.model,
                        jar_assets_dir,
                        asset_index,
                        &mut model_cache,
                        packs,
                    );
                    if let Some(baked) =
                        bake_resolved_model(&resolved, model_ref.x, model_ref.y, block_tint)
                    {
                        let when = parse_when_condition(&case.when);
                        entries.push(MultipartEntry {
                            when,
                            quads: baked.quads,
                        });
                    }
                }
                if !entries.is_empty() {
                    multipart_results.insert(block_name.to_string(), entries);
                }
            }

            if !variants_map.is_empty() {
                results.insert(block_name.to_string(), variants_map);
            }
            Some(())
        },
    );

    let mut missing_names: Vec<String> = Vec::new();
    for_each_blockstate(jar_assets_dir, asset_index, packs, |block_name, _| {
        if !results.contains_key(block_name)
            && !multipart_results.contains_key(block_name)
            && !crate::world::block_entity::is_block_entity_block(block_name)
            && !crate::world::block_entity::is_fluid_block(block_name)
            && !crate::world::block_entity::is_invisible_block(block_name)
        {
            missing_names.push(block_name.to_string());
        }
        Some(())
    });
    missing_names.sort();
    let baked_count = results.len() + multipart_results.len();
    tracing::info!(
        "Baked models for {}/{} blocks ({} unhandled)",
        baked_count,
        total,
        missing_names.len()
    );
    if !missing_names.is_empty() {
        tracing::warn!("Unhandled baked models: {}", missing_names.join(", "));
    }
    (results, multipart_results)
}

pub struct BakedItemModels {
    pub models: HashMap<String, BakedModel>,
    pub generated_textures: HashSet<String>,
    pub flat_texture_keys: HashMap<String, String>,
    pub(crate) gui_transforms: HashMap<String, DisplayTransform>,
    pub ground_transforms: HashMap<String, Mat4>,
}

/// Every `minecraft/items/*.json` name across the jar and the active packs,
/// so a pack can add an item, not only replace one. Vanilla walks every pack
/// in `FallbackResourceManager`.
fn item_definition_names(
    jar_assets_dir: &Path,
    packs: Option<&crate::resource_pack::ResourcePackManager>,
) -> std::collections::BTreeSet<String> {
    let pack_dirs = packs
        .into_iter()
        .flat_map(|packs| packs.active_pack_dirs())
        .map(|dir| dir.join("assets"));
    std::iter::once(jar_assets_dir.to_path_buf())
        .chain(pack_dirs)
        .filter_map(|root| std::fs::read_dir(root.join("minecraft").join("items")).ok())
        .flatten()
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(str::to_owned)
        })
        .collect()
}

pub fn bake_item_models(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&crate::resource_pack::ResourcePackManager>,
) -> BakedItemModels {
    let mut item_models: HashMap<String, BakedModel> = HashMap::new();
    let mut flat_item_textures: HashSet<String> = HashSet::new();
    let mut flat_keys: HashMap<String, String> = HashMap::new();
    let mut gui_transforms: HashMap<String, DisplayTransform> = HashMap::new();
    let mut ground_transforms: HashMap<String, Mat4> = HashMap::new();
    let mut model_cache: HashMap<String, ModelFile> = HashMap::new();

    for item_name in item_definition_names(jar_assets_dir, packs) {
        let item_name = item_name.as_str();
        let item_asset_key = format!("minecraft/items/{item_name}.json");
        let item_path =
            resolve_asset_path_with_packs(jar_assets_dir, asset_index, &item_asset_key, packs);
        let Ok(contents) = std::fs::read_to_string(item_path) else {
            continue;
        };
        let Ok(json): Result<serde_json::Value, _> = serde_json::from_str(&contents) else {
            continue;
        };

        let parts = collect_model_parts(&json);
        if parts.is_empty() {
            continue;
        }

        let tint = determine_tint(item_name);
        let mut merged: Option<BakedModel> = None;
        // Vanilla applies each composite part's own display transforms. Pomme
        // merges the parts into one mesh, so it can apply only one; no vanilla
        // composite disagrees (beds share `block/template_bed`), so the first
        // part's wins and a disagreement is logged rather than modelled.
        let mut ground_transform: Option<Mat4> = None;
        let mut gui_transform: Option<DisplayTransform> = None;
        let mut gui_light: Option<GuiLight> = None;
        for part in &parts {
            let resolved = resolve_model(
                &part.path,
                jar_assets_dir,
                asset_index,
                &mut model_cache,
                packs,
            );
            match ground_transform {
                None => ground_transform = Some(resolved.ground_transform),
                Some(existing) if existing.abs_diff_eq(resolved.ground_transform, 1.0e-6) => {}
                Some(_) => tracing::warn!(
                    "{item_name}: composite part {} has a different ground transform; using the first part's",
                    part.path
                ),
            }
            match gui_transform {
                None => gui_transform = Some(resolved.gui_transform),
                Some(existing) if existing == resolved.gui_transform => {}
                Some(_) => tracing::warn!(
                    "{item_name}: composite part {} has a different GUI transform; using the first part's",
                    part.path
                ),
            }
            match gui_light {
                None => gui_light = Some(resolved.gui_light),
                Some(existing) if existing == resolved.gui_light => {}
                Some(_) => tracing::warn!(
                    "{item_name}: composite part {} has a different GUI light; using the first part's",
                    part.path
                ),
            }
            // A flat sprite (layer0, no elements) only makes sense as the
            // sole part; `merged` stays empty so no 3D model is inserted.
            if parts.len() == 1 && resolved.elements.is_empty() {
                if let Some(value) = resolved.textures.get("layer0")
                    && let Some(key) = texture_to_name(value)
                {
                    flat_item_textures.insert(key.clone());
                    flat_keys.insert(item_name.to_string(), key);
                }
                break;
            }
            let Some(mut baked) = bake_resolved_model(&resolved, 0, 0, tint) else {
                continue;
            };
            if let Some(m) = part.transform {
                for quad in &mut baked.quads {
                    for p in &mut quad.positions {
                        *p = m.transform_point3(Vec3::from_array(*p)).to_array();
                    }
                }
            }
            merged = Some(match merged.take() {
                None => baked,
                Some(mut model) => {
                    model.quads.extend(baked.quads);
                    model.is_full_cube = false;
                    model.occludes = false;
                    model
                }
            });
        }
        if let Some(transform) = ground_transform {
            ground_transforms.insert(item_name.to_string(), transform);
        }
        let gui_transform = gui_transform.unwrap_or(DisplayTransform::IDENTITY);
        gui_transforms.insert(item_name.to_string(), gui_transform);
        if let Some(mut baked) = merged {
            apply_gui_lambert(
                &mut baked.quads,
                gui_transform,
                gui_light.unwrap_or_default(),
            );
            item_models.insert(item_name.to_string(), baked);
        }
    }

    // The mesh comes from the special renderer, but the chest's display
    // transforms live in `item/template_chest` like any other item's.
    let chest = resolve_model(
        "item/template_chest",
        jar_assets_dir,
        asset_index,
        &mut model_cache,
        packs,
    );
    item_models.insert(
        "chest".to_string(),
        bake_chest_item_model(chest.gui_transform, chest.gui_light),
    );
    gui_transforms.insert("chest".to_string(), chest.gui_transform);
    ground_transforms.insert("chest".to_string(), chest.ground_transform);
    flat_keys.remove("chest");

    tracing::info!(
        "Baked {} item models, {} flat items, and registered {} generated-item textures",
        item_models.len(),
        flat_keys.len(),
        flat_item_textures.len()
    );
    BakedItemModels {
        models: item_models,
        generated_textures: flat_item_textures,
        flat_texture_keys: flat_keys,
        gui_transforms,
        ground_transforms,
    }
}

fn bake_chest_item_model(gui: DisplayTransform, gui_light: GuiLight) -> BakedModel {
    let tex = "entity/chest/normal";
    let mut quads = Vec::new();
    let shades = vanilla_gui_face_shades(gui, gui_light);
    add_chest_cube(
        &mut quads,
        1.0 / 16.0,
        0.0,
        1.0 / 16.0,
        15.0 / 16.0,
        10.0 / 16.0,
        15.0 / 16.0,
        0.0,
        19.0,
        14.0,
        10.0,
        14.0,
        tex,
        shades,
    );
    add_chest_cube(
        &mut quads,
        1.0 / 16.0,
        9.0 / 16.0,
        1.0 / 16.0,
        15.0 / 16.0,
        14.0 / 16.0,
        15.0 / 16.0,
        0.0,
        0.0,
        14.0,
        5.0,
        14.0,
        tex,
        shades,
    );
    add_chest_cube(
        &mut quads,
        7.0 / 16.0,
        7.0 / 16.0,
        15.0 / 16.0,
        9.0 / 16.0,
        11.0 / 16.0,
        1.0,
        0.0,
        0.0,
        2.0,
        4.0,
        1.0,
        tex,
        shades,
    );
    BakedModel {
        quads,
        is_full_cube: false,
        occludes: false,
    }
}

fn items_3d_lights() -> ([f32; 3], [f32; 3]) {
    let pose = Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0))
        * Mat4::from_rotation_y(1.0821041)
        * Mat4::from_rotation_x(3.2375858)
        * Mat4::from_rotation_y(-std::f32::consts::PI / 8.0)
        * Mat4::from_rotation_x(2.3561945);
    let transform = |x: f32, y: f32, z: f32| {
        pose.transform_vector3(Vec3::new(x, y, z).normalize())
            .to_array()
    };
    (transform(0.2, 1.0, -0.7), transform(-0.2, 1.0, 0.7))
}

fn items_flat_lights() -> ([f32; 3], [f32; 3]) {
    let pose =
        Mat4::from_rotation_y(-std::f32::consts::PI / 8.0) * Mat4::from_rotation_x(2.3561945);
    let transform = |x: f32, y: f32, z: f32| {
        pose.transform_vector3(Vec3::new(x, y, z).normalize())
            .to_array()
    };
    (transform(0.2, 1.0, -0.7), transform(-0.2, 1.0, 0.7))
}

fn gui_lights(gui_light: GuiLight) -> ([f32; 3], [f32; 3]) {
    match gui_light {
        GuiLight::Front => items_flat_lights(),
        GuiLight::Side => items_3d_lights(),
    }
}

fn lambert_shade(world_normal: [f32; 3], l0: [f32; 3], l1: [f32; 3]) -> f32 {
    let d0 = (l0[0] * world_normal[0] + l0[1] * world_normal[1] + l0[2] * world_normal[2]).max(0.0);
    let d1 = (l1[0] * world_normal[0] + l1[1] * world_normal[1] + l1[2] * world_normal[2]).max(0.0);
    ((d0 + d1) * 0.6 + 0.4).min(1.0)
}

fn transform_gui_normal(n_mesh: [f32; 3], display: DisplayTransform) -> [f32; 3] {
    // GuiItemAtlas applies (slot, -slot, slot) before ItemTransform. Vanilla's
    // PoseStack transforms normals by the inverse-transpose of that composed
    // pose, including each model's actual GUI rotation/scale.
    let pose = Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0)) * display.to_matrix();
    let normal = Mat3::from_mat4(pose).inverse().transpose() * Vec3::from_array(n_mesh);
    normal.normalize_or_zero().to_array()
}

fn vanilla_gui_face_shades(display: DisplayTransform, gui_light: GuiLight) -> [f32; 6] {
    let (l0, l1) = gui_lights(gui_light);
    let normals = [
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, -1.0],
        [0.0, 0.0, 1.0],
        [-1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
    ];
    let mut shades = [0.0; 6];
    for (i, &n) in normals.iter().enumerate() {
        shades[i] = lambert_shade(transform_gui_normal(n, display), l0, l1);
    }
    shades
}

/// TODO: `held_item.rs` draws this same mesh through `item_entity.vert`, so
/// the first-person hand picks up the GUI pose's lambert too. Vanilla lights
/// the hand with `Lighting.Entry.LEVEL` (it draws inside `renderLevel`, before
/// `GameRenderer` switches to `ITEMS_3D` for the GUI).
fn apply_gui_lambert(quads: &mut [BakedQuad], display: DisplayTransform, gui_light: GuiLight) {
    let (l0, l1) = gui_lights(gui_light);
    for quad in quads {
        // ItemFeatureRenderer sends BakedQuad.direction() through the pose's
        // normal matrix. It does not apply BlockModelLighter's cardinal face
        // brightness in GUI rendering, so this replaces (rather than
        // multiplies) the terrain-oriented shade byte from bake_resolved_model.
        //
        // Not `quad.shade_face`: that is `None` for `shade: false` elements,
        // and `putBakedQuad` takes `direction()` regardless of `shade()`.
        let direction = direction_from_positions(&quad.positions).unwrap_or(Direction::Up);
        let n_mesh = direction.offset().map(|component| component as f32);
        let n_gui = transform_gui_normal(n_mesh, display);
        quad.shade_light = lambert_shade(n_gui, l0, l1);
    }
}

#[allow(clippy::too_many_arguments)]
fn add_chest_cube(
    quads: &mut Vec<BakedQuad>,
    x0: f32,
    y0: f32,
    z0: f32,
    x1: f32,
    y1: f32,
    z1: f32,
    u: f32,
    v: f32,
    w: f32,
    h: f32,
    d: f32,
    texture: &str,
    shades: [f32; 6],
) {
    const TEX_SIZE: f32 = 64.0;

    // Vanilla ModelPart.Cube names the z=min vertices `t*` and z=max
    // vertices `l*`, then builds faces in DOWN/UP/WEST/NORTH/EAST/SOUTH
    // order. Keep that layout exactly: chest items use the special entity
    // model rather than a normal block/item JSON mesh.
    let t0 = [x0, y0, z0];
    let t1 = [x1, y0, z0];
    let t2 = [x1, y1, z0];
    let t3 = [x0, y1, z0];
    let l0 = [x0, y0, z1];
    let l1 = [x1, y0, z1];
    let l2 = [x1, y1, z1];
    let l3 = [x0, y1, z1];

    let u0 = u;
    let u1 = u + d;
    let u2 = u + d + w;
    let u22 = u + d + w + w;
    let u3 = u + d + w + d;
    let u4 = u + d + w + d + w;
    let v0 = v;
    let v1 = v + d;
    let v2 = v + d + h;

    let face_specs = [
        FaceSpec {
            positions: [l1, l0, t0, t1],
            uv_pixels: [[u2, v0], [u1, v0], [u1, v1], [u2, v1]],
            shade: shades[1],
        },
        FaceSpec {
            positions: [t2, t3, l3, l2],
            uv_pixels: [[u22, v1], [u2, v1], [u2, v0], [u22, v0]],
            shade: shades[0],
        },
        FaceSpec {
            positions: [t0, l0, l3, t3],
            uv_pixels: [[u1, v1], [u0, v1], [u0, v2], [u1, v2]],
            shade: shades[4],
        },
        FaceSpec {
            positions: [t1, t0, t3, t2],
            uv_pixels: [[u2, v1], [u1, v1], [u1, v2], [u2, v2]],
            shade: shades[2],
        },
        FaceSpec {
            positions: [l1, t1, t2, l2],
            uv_pixels: [[u3, v1], [u2, v1], [u2, v2], [u3, v2]],
            shade: shades[5],
        },
        FaceSpec {
            positions: [l0, l1, l2, l3],
            uv_pixels: [[u4, v1], [u3, v1], [u3, v2], [u4, v2]],
            shade: shades[3],
        },
    ];

    for spec in face_specs {
        quads.push(BakedQuad {
            positions: spec.positions,
            uvs: spec
                .uv_pixels
                .map(|[u_px, v_px]| [u_px / TEX_SIZE, v_px / TEX_SIZE]),
            texture: texture.to_string(),
            cullface: None,
            tint: super::registry::Tint::None,
            shade_light: spec.shade,
            shade_face: None,
        });
    }
}

struct FaceSpec {
    positions: [[f32; 3]; 4],
    uv_pixels: [[f32; 2]; 4],
    shade: f32,
}

struct ModelPart {
    path: String,
    transform: Option<Mat4>,
}

/// Model references to bake for one item. `minecraft:composite` contributes
/// every child, with the children's `transformation`s composed parent-to-child
/// (vanilla `CompositeModel.Unbaked.bake`); anything else keeps the old
/// first-model-string heuristic, which for select/condition trees picks one
/// representative state.
fn collect_model_parts(json: &serde_json::Value) -> Vec<ModelPart> {
    let mut parts = Vec::new();
    if let Some(node) = json.get("model") {
        collect_parts_from_node(node, None, &mut parts);
    }
    if parts.is_empty()
        && let Some(path) = first_item_model_ref(json)
    {
        parts.push(ModelPart {
            path,
            transform: None,
        });
    }
    parts
}

/// First `model` reference in an item definition, falling back to a special
/// renderer's `base`.
pub fn first_item_model_ref(json: &serde_json::Value) -> Option<String> {
    find_first_model_string(json)
        .or_else(|| find_first_string_for_key(json, "base"))
        .map(|path| strip_mc_prefix(&path).to_string())
}

fn collect_parts_from_node(
    node: &serde_json::Value,
    parent_transform: Option<Mat4>,
    parts: &mut Vec<ModelPart>,
) {
    let own_transform = match node.get("transformation") {
        Some(value) => match parse_item_transformation(value) {
            Some(transform) => Some(transform),
            None => {
                tracing::warn!("Skipping item-model part with unsupported transformation encoding");
                return;
            }
        },
        None => None,
    };
    let transform = match (parent_transform, own_transform) {
        (Some(parent), Some(own)) => Some(parent * own),
        (parent, own) => parent.or(own),
    };
    let node_type = node
        .get("type")
        .and_then(|t| t.as_str())
        .map(strip_mc_prefix);
    match node_type {
        Some("composite") => {
            if let Some(models) = node.get("models").and_then(|m| m.as_array()) {
                for child in models {
                    collect_parts_from_node(child, transform, parts);
                }
            }
        }
        Some("model") => {
            if let Some(path) = node.get("model").and_then(|m| m.as_str()) {
                parts.push(ModelPart {
                    path: strip_mc_prefix(path).to_string(),
                    transform,
                });
            }
        }
        // Other node types are left for the caller's whole-file fallback.
        _ => {}
    }
}

/// Vanilla `Transformation.compose` (Transformation.java:103): `translation ·
/// leftRotation · scale · rightRotation`, translation in block units. Pomme
/// currently supports the stock quaternion-array encoding. Other valid codec
/// forms are rejected by the caller instead of silently becoming identity.
fn parse_item_transformation(json: &serde_json::Value) -> Option<Mat4> {
    let object = json.as_object()?;
    let quat = |key: &str| -> Option<Quat> {
        let Some(value) = object.get(key) else {
            return Some(Quat::IDENTITY);
        };
        let arr = value.as_array()?;
        if arr.len() != 4 {
            return None;
        }
        let get = |i: usize| arr.get(i)?.as_f64().map(|v| v as f32);
        Some(Quat::from_xyzw(get(0)?, get(1)?, get(2)?, get(3)?))
    };
    let vec3 = |key: &str, default: Vec3| {
        object.get(key).map_or(Some(default), |v| {
            v.as_array().and_then(|arr| {
                if arr.len() != 3 {
                    return None;
                }
                Some(Vec3::new(
                    arr[0].as_f64()? as f32,
                    arr[1].as_f64()? as f32,
                    arr[2].as_f64()? as f32,
                ))
            })
        })
    };
    Some(
        Mat4::from_translation(vec3("translation", Vec3::ZERO)?)
            * Mat4::from_quat(quat("left_rotation")?)
            * Mat4::from_scale(vec3("scale", Vec3::ONE)?)
            * Mat4::from_quat(quat("right_rotation")?),
    )
}

pub fn parse_vec3(value: &serde_json::Value, default: Vec3) -> Vec3 {
    let Some(arr) = value.as_array() else {
        return default;
    };
    let get = |i: usize| arr.get(i).and_then(|v| v.as_f64()).map(|v| v as f32);
    Vec3::new(
        get(0).unwrap_or(default.x),
        get(1).unwrap_or(default.y),
        get(2).unwrap_or(default.z),
    )
}

pub fn strip_mc_prefix(s: &str) -> &str {
    s.strip_prefix("minecraft:").unwrap_or(s)
}

pub fn find_first_model_string(json: &serde_json::Value) -> Option<String> {
    find_first_string_for_key(json, "model")
}

pub fn find_first_string_for_key(json: &serde_json::Value, key: &str) -> Option<String> {
    match json {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get(key) {
                return Some(s.clone());
            }
            for v in map.values() {
                if let Some(r) = find_first_string_for_key(v, key) {
                    return Some(r);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(|v| find_first_string_for_key(v, key)),
        _ => None,
    }
}

fn parse_when_condition(when: &Option<serde_json::Value>) -> HashMap<String, String> {
    let mut result = HashMap::new();
    if let Some(serde_json::Value::Object(map)) = when {
        for (key, value) in map {
            if let serde_json::Value::String(s) = value {
                result.insert(key.clone(), s.clone());
            } else if let serde_json::Value::Bool(b) = value {
                result.insert(key.clone(), b.to_string());
            }
        }
    }
    result
}

fn for_each_blockstate(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&crate::resource_pack::ResourcePackManager>,
    mut callback: impl FnMut(&str, &BlockstateFile) -> Option<()>,
) {
    let Some(blockstates_dir) = resolve_blockstates_dir(jar_assets_dir, asset_index, packs) else {
        tracing::warn!("Blockstates directory not found");
        return;
    };

    let entries = match std::fs::read_dir(&blockstates_dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Failed to read blockstates dir: {e}");
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(blockstate) = serde_json::from_str::<BlockstateFile>(&contents) else {
            continue;
        };

        callback(name, &blockstate);
    }
}

fn resolve_blockstates_dir(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&crate::resource_pack::ResourcePackManager>,
) -> Option<PathBuf> {
    let candidates = [
        jar_assets_dir.join("assets/minecraft/blockstates"),
        jar_assets_dir.join("jar/assets/minecraft/blockstates"),
        PathBuf::from("reference/assets/assets/minecraft/blockstates"),
    ];

    for c in &candidates {
        if c.is_dir() {
            return Some(c.clone());
        }
    }

    // Also check the original simple path
    let path = jar_assets_dir.join("minecraft/blockstates");
    if path.is_dir() {
        return Some(path);
    }

    if asset_index.is_some() {
        let test_path = resolve_asset_path_with_packs(
            jar_assets_dir,
            asset_index,
            "minecraft/blockstates/stone.json",
            packs,
        );
        if test_path.exists() {
            return test_path.parent().map(|p| p.to_path_buf());
        }
    }

    None
}

fn extract_default_model_ref(blockstate: &BlockstateFile) -> Option<ModelRef> {
    if let Some(variants) = &blockstate.variants {
        let entry = variants.get("").or_else(|| variants.values().next())?;
        let r = entry.first()?;
        Some(ModelRef {
            model: r.model.clone(),
            x: r.x,
            y: r.y,
            uvlock: r.uvlock,
        })
    } else if let Some(multipart) = &blockstate.multipart {
        let r = multipart.first()?.apply.first()?;
        Some(ModelRef {
            model: r.model.clone(),
            x: r.x,
            y: r.y,
            uvlock: r.uvlock,
        })
    } else {
        None
    }
}

struct ResolvedModel {
    textures: HashMap<String, String>,
    elements: Vec<ElementDef>,
    gui_transform: DisplayTransform,
    gui_light: GuiLight,
    ground_transform: Mat4,
}

fn resolve_model(
    model_id: &str,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    cache: &mut HashMap<String, ModelFile>,
    packs: Option<&crate::resource_pack::ResourcePackManager>,
) -> ResolvedModel {
    let mut texture_map: HashMap<String, String> = HashMap::new();
    let mut elements: Option<Vec<ElementDef>> = None;
    let mut gui_transform: Option<DisplayTransform> = None;
    let mut gui_light: Option<GuiLight> = None;
    let mut ground_transform: Option<Mat4> = None;
    let mut current_id = model_id.to_string();

    for _ in 0..20 {
        let Some(model) = load_model(&current_id, jar_assets_dir, asset_index, cache, packs) else {
            break;
        };

        for (key, value) in &model.textures {
            texture_map
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }

        if elements.is_none() && !model.elements.is_empty() {
            elements = Some(model.elements.clone());
        }
        if gui_transform.is_none()
            && let Some(transform) = model.display.get("gui").and_then(parse_display_transform)
        {
            gui_transform = Some(transform);
        }
        if gui_light.is_none() {
            gui_light = model.gui_light;
        }
        if ground_transform.is_none()
            && let Some(transform) = model
                .display
                .get("ground")
                .and_then(parse_display_transform)
        {
            ground_transform = Some(transform.to_matrix());
        }

        match &model.parent {
            Some(parent) => current_id = parent.clone(),
            None => break,
        }
    }

    let mut resolved_textures = HashMap::new();
    for (key, value) in &texture_map {
        resolved_textures.insert(key.clone(), resolve_ref(value, &texture_map, 0));
    }

    ResolvedModel {
        textures: resolved_textures,
        elements: elements.unwrap_or_default(),
        gui_transform: gui_transform.unwrap_or(DisplayTransform::IDENTITY),
        gui_light: gui_light.unwrap_or_default(),
        ground_transform: ground_transform.unwrap_or(Mat4::IDENTITY),
    }
}

fn resolve_ref(value: &str, map: &HashMap<String, String>, depth: u32) -> String {
    if depth > 10 {
        return value.to_string();
    }
    if let Some(ref_name) = value.strip_prefix('#')
        && let Some(target) = map.get(ref_name)
    {
        return resolve_ref(target, map, depth + 1);
    }
    value.to_string()
}

fn load_model<'a>(
    model_id: &str,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    cache: &'a mut HashMap<String, ModelFile>,
    packs: Option<&crate::resource_pack::ResourcePackManager>,
) -> Option<&'a ModelFile> {
    if cache.contains_key(model_id) {
        return cache.get(model_id);
    }

    let asset_key = model_id_to_asset_key(model_id);
    let file_path = resolve_model_path(jar_assets_dir, asset_index, &asset_key, packs)?;

    let contents = std::fs::read_to_string(&file_path).ok()?;
    let model: ModelFile = match serde_json::from_str(&contents) {
        Ok(model) => model,
        Err(error) => {
            // Vanilla rejects the model too, but it says so rather than going quiet.
            tracing::warn!("{model_id}: model JSON is invalid ({error}); skipping");
            return None;
        }
    };
    cache.insert(model_id.to_string(), model);
    cache.get(model_id)
}

fn resolve_model_path(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    asset_key: &str,
    packs: Option<&crate::resource_pack::ResourcePackManager>,
) -> Option<PathBuf> {
    let primary = resolve_asset_path_with_packs(jar_assets_dir, asset_index, asset_key, packs);
    if primary.exists() {
        return Some(primary);
    }

    let ref_path = Path::new("reference/assets/assets")
        .join(asset_key.strip_prefix("minecraft/").unwrap_or(asset_key));
    if ref_path.exists() {
        return Some(ref_path);
    }

    None
}

fn model_id_to_asset_key(model_id: &str) -> String {
    AssetId::parse(model_id).asset_key("models", ".json")
}

fn texture_to_name(texture_ref: &str) -> Option<String> {
    if texture_ref.starts_with('#') {
        return None;
    }
    let id = AssetId::parse(texture_ref);
    if id.namespace == "minecraft" {
        if let Some(block_path) = id.path.strip_prefix("block/") {
            Some(block_path.to_string())
        } else if id.path.starts_with("item/")
            || id.path.starts_with("entity/")
            || id.path.starts_with("particle/")
        {
            Some(id.path.to_string())
        } else {
            Some(format!("minecraft:{}", id.path))
        }
    } else {
        Some(id.canonical())
    }
}

fn resolve_face_texture<'a>(
    reference: &str,
    textures: &'a HashMap<String, String>,
) -> Option<&'a str> {
    let slot = reference.strip_prefix('#').unwrap_or(reference);
    textures.get(slot).map(String::as_str)
}

fn bake_resolved_model(
    resolved: &ResolvedModel,
    rot_x: i32,
    rot_y: i32,
    tint: super::registry::Tint,
) -> Option<BakedModel> {
    if resolved.elements.is_empty() {
        return None;
    }

    let mut quads = Vec::new();

    for element in &resolved.elements {
        let from = [
            element.from[0] / 16.0,
            element.from[1] / 16.0,
            element.from[2] / 16.0,
        ];
        let to = [
            element.to[0] / 16.0,
            element.to[1] / 16.0,
            element.to[2] / 16.0,
        ];

        for (face_name, face_def) in &element.faces {
            let Some(dir) = Direction::from_str(face_name) else {
                continue;
            };

            let Some(texture_ref) = resolve_face_texture(&face_def.texture, &resolved.textures)
            else {
                continue;
            };
            let Some(texture_name) = texture_to_name(texture_ref) else {
                continue;
            };

            let positions = face_positions(dir, from, to);
            let uvs = face_uvs(dir, from, to, face_def.uv.as_ref(), face_def.rotation);

            let mut positions = apply_element_rotation(positions, &element.rotation);

            let mut cullface = face_def.cullface.as_deref().and_then(Direction::from_str);
            let quad_tint = if face_def.tint_index.is_some() {
                tint
            } else {
                super::registry::Tint::None
            };

            if rot_x != 0 || rot_y != 0 {
                positions = rotate_positions(positions, rot_x, rot_y);
                cullface = cullface.map(|d| d.rotate_x(rot_x).rotate_y(rot_y));
            }

            // Vanilla `FaceBakery.bakeQuad`: the shade direction is the
            // rotated quad's nearest cardinal, `UP` for a degenerate quad.
            let shade_face = element
                .shade
                .then(|| direction_from_positions(&positions).unwrap_or(Direction::Up));

            quads.push(BakedQuad {
                positions,
                uvs,
                texture: texture_name,
                cullface,
                tint: quad_tint,
                shade_light: shade_face.map_or(1.0, |face| face.shade_light()),
                shade_face,
            });
        }
    }

    if quads.is_empty() {
        return None;
    }

    let is_full_cube = check_full_cube(&quads);
    Some(BakedModel {
        quads,
        is_full_cube,
        occludes: is_full_cube,
    })
}

fn check_full_cube(quads: &[BakedQuad]) -> bool {
    if quads.len() != 6 {
        return false;
    }
    let mut dirs = [false; 6];
    for q in quads {
        match q.cullface {
            Some(Direction::Down) => dirs[0] = true,
            Some(Direction::Up) => dirs[1] = true,
            Some(Direction::North) => dirs[2] = true,
            Some(Direction::South) => dirs[3] = true,
            Some(Direction::West) => dirs[4] = true,
            Some(Direction::East) => dirs[5] = true,
            None => return false,
        }
    }
    dirs.iter().all(|&d| d)
}

/// Vanilla `FaceInfo`: the per-face vertex order all the UV rules are defined
/// against. Every face winds CCW viewed from outside, which the chunk
/// pipeline's backface culling needs.
pub(crate) fn face_positions(dir: Direction, from: [f32; 3], to: [f32; 3]) -> [[f32; 3]; 4] {
    let [x0, y0, z0] = from;
    let [x1, y1, z1] = to;
    match dir {
        Direction::Down => [[x0, y0, z1], [x0, y0, z0], [x1, y0, z0], [x1, y0, z1]],
        Direction::Up => [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
        Direction::North => [[x1, y1, z0], [x1, y0, z0], [x0, y0, z0], [x0, y1, z0]],
        Direction::South => [[x0, y1, z1], [x0, y0, z1], [x1, y0, z1], [x1, y1, z1]],
        Direction::West => [[x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1]],
        Direction::East => [[x1, y1, z1], [x1, y0, z1], [x1, y0, z0], [x1, y1, z0]],
    }
}

pub(crate) fn face_uvs(
    dir: Direction,
    from: [f32; 3],
    to: [f32; 3],
    explicit_uv: Option<&[f32; 4]>,
    rotation: Option<i32>,
) -> [[f32; 2]; 4] {
    // Vanilla `FaceBakery.defaultFaceUV`, normalized to 0..1: some faces
    // sample a window reflected about the texture center.
    let (u1, v1, u2, v2) = if let Some(uv) = explicit_uv {
        (uv[0] / 16.0, uv[1] / 16.0, uv[2] / 16.0, uv[3] / 16.0)
    } else {
        match dir {
            Direction::Down => (from[0], 1.0 - to[2], to[0], 1.0 - from[2]),
            Direction::Up => (from[0], from[2], to[0], to[2]),
            Direction::North => (1.0 - to[0], 1.0 - to[1], 1.0 - from[0], 1.0 - from[1]),
            Direction::South => (from[0], 1.0 - to[1], to[0], 1.0 - from[1]),
            Direction::West => (from[2], 1.0 - to[1], to[2], 1.0 - from[1]),
            Direction::East => (1.0 - to[2], 1.0 - to[1], 1.0 - from[2], 1.0 - from[1]),
        }
    };

    // Vanilla `CuboidFace.UVs` corner cycle, identical for every face (the
    // per-face variation lives in `face_positions`' vertex order).
    // `Quadrant.rotateVertexIndex` shifts each vertex forward through the
    // cycle, spinning the texture clockwise per 90 degrees viewed from
    // outside the block.
    let cycle = [[u1, v1], [u1, v2], [u2, v2], [u2, v1]];
    let shift = rotation.map_or(0, |r| {
        if r % 90 != 0 {
            // Vanilla `Quadrant.parseJson` rejects the whole model instead.
            tracing::warn!("face uv rotation {r} is not a multiple of 90, truncating");
        }
        r.rem_euclid(360) / 90
    }) as usize;
    std::array::from_fn(|i| cycle[(i + shift) % 4])
}

fn apply_element_rotation(
    mut positions: [[f32; 3]; 4],
    rotation: &Option<ElementRotation>,
) -> [[f32; 3]; 4] {
    let Some(rot) = rotation else {
        return positions;
    };

    let origin = [
        rot.origin[0] / 16.0,
        rot.origin[1] / 16.0,
        rot.origin[2] / 16.0,
    ];
    let angle_rad = rot.angle.to_radians();
    let cos = angle_rad.cos();
    let sin = angle_rad.sin();

    for pos in &mut positions {
        let dx = pos[0] - origin[0];
        let dy = pos[1] - origin[1];
        let dz = pos[2] - origin[2];

        let (nx, ny, nz) = match rot.axis.as_str() {
            "x" => (dx, cos * dy - sin * dz, sin * dy + cos * dz),
            "y" => (cos * dx + sin * dz, dy, -sin * dx + cos * dz),
            "z" => (cos * dx - sin * dy, sin * dx + cos * dy, dz),
            _ => (dx, dy, dz),
        };

        if rot.rescale {
            let scale = 1.0 / cos.abs();
            pos[0] = origin[0] + nx * scale;
            pos[1] = origin[1] + ny * scale;
            pos[2] = origin[2] + nz * scale;
        } else {
            pos[0] = origin[0] + nx;
            pos[1] = origin[1] + ny;
            pos[2] = origin[2] + nz;
        }
    }

    positions
}

/// The quad's winding normal, or `None` when it is degenerate.
pub(crate) fn quad_normal(positions: &[[f32; 3]; 4]) -> Option<Vec3> {
    let p0 = Vec3::from_array(positions[0]);
    let p1 = Vec3::from_array(positions[1]);
    let p2 = Vec3::from_array(positions[2]);
    (p1 - p0).cross(p2 - p0).try_normalize()
}

/// Vanilla `FaceBakery.findClosestDirection`: the cardinal with the largest
/// positive dot product, first in `Direction` order on a tie; `None` when the
/// normal points nowhere.
pub(crate) fn nearest_cardinal_direction(normal: Vec3) -> Option<Direction> {
    let mut best = None;
    let mut closest_product = 0.0f32;
    for candidate in [
        Direction::Down,
        Direction::Up,
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ] {
        let product = normal.dot(Vec3::from_array(candidate.offset().map(|v| v as f32)));
        if product >= 0.0 && product > closest_product {
            closest_product = product;
            best = Some(candidate);
        }
    }
    best
}

pub(crate) fn direction_from_positions(positions: &[[f32; 3]; 4]) -> Option<Direction> {
    nearest_cardinal_direction(quad_normal(positions)?)
}

fn rotate_positions(mut positions: [[f32; 3]; 4], rot_x: i32, rot_y: i32) -> [[f32; 3]; 4] {
    let center = 0.5f32;

    if rot_x != 0 {
        let angle = (rot_x as f32).to_radians();
        let cos = angle.cos();
        let sin = angle.sin();
        for pos in &mut positions {
            let dy = pos[1] - center;
            let dz = pos[2] - center;
            pos[1] = center + cos * dy + sin * dz;
            pos[2] = center - sin * dy + cos * dz;
        }
    }

    if rot_y != 0 {
        let angle = (rot_y as f32).to_radians();
        let cos = angle.cos();
        let sin = angle.sin();
        for pos in &mut positions {
            let dx = pos[0] - center;
            let dz = pos[2] - center;
            pos[0] = center + cos * dx - sin * dz;
            pos[2] = center + sin * dx + cos * dz;
        }
    }

    positions
}

fn build_face_textures(
    block_name: &str,
    textures: &HashMap<String, String>,
) -> Option<FaceTextures> {
    let mut faces = face_textures_base(block_name, textures)?;
    faces.particle = textures.get("particle").and_then(|v| texture_to_name(v));
    Some(faces)
}

fn face_textures_base(
    block_name: &str,
    textures: &HashMap<String, String>,
) -> Option<FaceTextures> {
    let get = |key: &str| -> Option<String> { textures.get(key).and_then(|v| texture_to_name(v)) };

    let (up, down, north, south, east, west) = (
        get("up"),
        get("down"),
        get("north"),
        get("south"),
        get("east"),
        get("west"),
    );

    let tint = determine_tint(block_name);

    if let (Some(up), Some(down), Some(north), Some(south), Some(east), Some(west)) =
        (up, down, north, south, east, west)
    {
        let (side_overlay, tint) = if block_name == "grass_block" {
            (Some("grass_block_side_overlay"), Tint::Grass)
        } else {
            (None, tint)
        };
        return Some(FaceTextures::new(
            &up,
            &down,
            &north,
            &south,
            &east,
            &west,
            side_overlay,
            tint,
        ));
    }

    if let Some(all) = get("all") {
        return Some(FaceTextures::uniform(&all, tint));
    }

    if let (Some(end), Some(side)) = (get("end"), get("side")) {
        return Some(FaceTextures::new(
            &end,
            &end,
            &side,
            &side,
            &side,
            &side,
            None,
            Tint::None,
        ));
    }

    if let (Some(top), Some(side)) = (get("top"), get("side")) {
        let bottom = get("bottom").unwrap_or_else(|| top.clone());
        return Some(FaceTextures::new(
            &top, &bottom, &side, &side, &side, &side, None, tint,
        ));
    }

    if let Some(cross) = get("cross") {
        return Some(FaceTextures::uniform(&cross, tint));
    }

    if let (Some(front), Some(side)) = (get("front"), get("side")) {
        let top = get("top")
            .or_else(|| get("end"))
            .unwrap_or_else(|| side.clone());
        let bottom = get("bottom").unwrap_or_else(|| top.clone());
        return Some(FaceTextures::new(
            &top,
            &bottom,
            &front,
            &side,
            &side,
            &side,
            None,
            Tint::None,
        ));
    }

    if let Some(p) = get("particle") {
        return Some(FaceTextures::uniform(&p, tint));
    }

    None
}

/// Full-cube blocks that mustn't cull adjacent faces (vanilla `noOcclusion`).
/// Solid `packed_ice`/`blue_ice` still occlude, so `ice` is matched exactly.
fn is_non_occluding(block_name: &str) -> bool {
    block_name.ends_with("_leaves")
        || block_name.ends_with("_stained_glass")
        || matches!(block_name, "glass" | "tinted_glass" | "ice" | "frosted_ice")
}

fn determine_tint(block_name: &str) -> Tint {
    if block_name == "redstone_wire" {
        Tint::Redstone
    } else if GRASS_TINTED.contains(&block_name) {
        Tint::Grass
    } else if DRY_FOLIAGE_TINTED.contains(&block_name) {
        Tint::DryFoliage
    } else if FOLIAGE_TINTED.contains(&block_name) || block_name.ends_with("_leaves") {
        // TODO: spruce_leaves and birch_leaves use fixed constant colors in
        // vanilla (BlockColors), not biome foliage; the `_leaves` suffix rule
        // mistints them.
        Tint::Foliage
    } else {
        Tint::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIRS: [Direction; 6] = [
        Direction::Down,
        Direction::Up,
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ];

    /// The (right, up) axes of each face viewed from outside the block:
    /// sky-up for the sides, and vanilla's map orientation for up/down.
    /// `right x up` is the outward normal.
    fn face_axes(dir: Direction) -> (Vec3, Vec3) {
        match dir {
            Direction::Down => (Vec3::X, Vec3::Z),
            Direction::Up => (Vec3::X, Vec3::NEG_Z),
            Direction::North => (Vec3::NEG_X, Vec3::Y),
            Direction::South => (Vec3::X, Vec3::Y),
            Direction::West => (Vec3::Z, Vec3::Y),
            Direction::East => (Vec3::NEG_Z, Vec3::Y),
        }
    }

    /// Quads must stay CCW viewed from outside for backface culling.
    #[test]
    fn face_winding_is_ccw_from_outside() {
        for dir in DIRS {
            let p = face_positions(dir, [0.0; 3], [1.0; 3]).map(Vec3::from_array);
            let normal = (p[1] - p[0]).cross(p[2] - p[0]);
            let outward = Vec3::from_array(dir.offset().map(|c| c as f32));
            assert!(normal.dot(outward) > 0.0, "{dir:?} winds the wrong way");
            assert_eq!(
                direction_from_positions(&p.map(|position| position.to_array())),
                Some(dir)
            );
        }
    }

    #[test]
    fn items_3d_lights_match_vanilla_pose_order() {
        let (light0, light1) = items_3d_lights();
        let expected = [
            (light0, [-0.9334393, -0.26269472, -0.24430019]),
            (light1, [-0.10357136, -0.97660685, 0.18844643]),
        ];
        for (actual, expected) in expected {
            for (actual, expected) in actual.into_iter().zip(expected) {
                assert!(
                    (actual - expected).abs() < 1.0e-6,
                    "expected {expected}, got {actual}"
                );
            }
        }
    }

    #[test]
    fn items_flat_lights_match_vanilla_pose_order() {
        let (light0, light1) = items_flat_lights();
        let expected = [
            (light0, [-0.222519, -0.17149863, 0.9597257]),
            (light1, [-0.21501213, -0.97182524, 0.0965678]),
        ];
        for (actual, expected) in expected {
            for (actual, expected) in actual.into_iter().zip(expected) {
                assert!(
                    (actual - expected).abs() < 1.0e-6,
                    "expected {expected}, got {actual}"
                );
            }
        }
    }

    #[test]
    fn gui_face_shades_include_vanilla_atlas_y_flip() {
        let block = vanilla_gui_face_shades(gui_transform(225.0), GuiLight::Side);
        let chest = vanilla_gui_face_shades(gui_transform(45.0), GuiLight::Side);
        let expected_block = [1.0, 0.4, 0.4, 1.0, 0.49398834, 0.6505372];
        let expected_chest = [1.0, 0.4, 1.0, 0.4, 0.6505372, 0.49398834];
        for (actual, expected) in block.into_iter().zip(expected_block) {
            assert!(
                (actual - expected).abs() < 1.0e-6,
                "expected {expected}, got {actual}"
            );
        }
        for (actual, expected) in chest.into_iter().zip(expected_chest) {
            assert!(
                (actual - expected).abs() < 1.0e-6,
                "expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn gui_face_shades_follow_model_display_transform() {
        let fence = vanilla_gui_face_shades(gui_transform(135.0), GuiLight::Side);
        let expected = [1.0, 0.4, 0.65053713, 0.49398836, 0.4, 1.0];
        for (actual, expected) in fence.into_iter().zip(expected) {
            assert!(
                (actual - expected).abs() < 1.0e-6,
                "expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn gui_lambert_replaces_terrain_cardinal_shade() {
        let mut quad = BakedQuad {
            positions: face_positions(Direction::Up, [0.0; 3], [1.0; 3]),
            uvs: [[0.0; 2]; 4],
            texture: "block/stone".to_string(),
            cullface: None,
            tint: Tint::None,
            shade_light: 0.25,
            shade_face: Some(Direction::Up),
        };
        apply_gui_lambert(
            std::slice::from_mut(&mut quad),
            gui_transform(225.0),
            GuiLight::Side,
        );
        assert!((quad.shade_light - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn chest_body_faces_match_vanilla_modelpart_cube() {
        let model = bake_chest_item_model(gui_transform(45.0), GuiLight::Side);
        assert_eq!(model.quads.len(), 18);

        let x0 = 1.0 / 16.0;
        let x1 = 15.0 / 16.0;
        let y0 = 0.0;
        let y1 = 10.0 / 16.0;
        let z0 = 1.0 / 16.0;
        let z1 = 15.0 / 16.0;
        let t0 = [x0, y0, z0];
        let t1 = [x1, y0, z0];
        let t2 = [x1, y1, z0];
        let t3 = [x0, y1, z0];
        let l0 = [x0, y0, z1];
        let l1 = [x1, y0, z1];
        let l2 = [x1, y1, z1];
        let l3 = [x0, y1, z1];

        let expected = [
            (
                [l1, l0, t0, t1],
                [[28.0, 19.0], [14.0, 19.0], [14.0, 33.0], [28.0, 33.0]],
            ),
            (
                [t2, t3, l3, l2],
                [[42.0, 33.0], [28.0, 33.0], [28.0, 19.0], [42.0, 19.0]],
            ),
            (
                [t0, l0, l3, t3],
                [[14.0, 33.0], [0.0, 33.0], [0.0, 43.0], [14.0, 43.0]],
            ),
            (
                [t1, t0, t3, t2],
                [[28.0, 33.0], [14.0, 33.0], [14.0, 43.0], [28.0, 43.0]],
            ),
            (
                [l1, t1, t2, l2],
                [[42.0, 33.0], [28.0, 33.0], [28.0, 43.0], [42.0, 43.0]],
            ),
            (
                [l0, l1, l2, l3],
                [[56.0, 33.0], [42.0, 33.0], [42.0, 43.0], [56.0, 43.0]],
            ),
        ];

        for (quad, (positions, uv_pixels)) in model.quads.iter().take(6).zip(expected) {
            assert_eq!(quad.positions, positions);
            assert_eq!(quad.uvs, uv_pixels.map(|[u, v]| [u / 64.0, v / 64.0]));
        }
    }

    #[test]
    fn model_rotation_uses_final_face_direction_for_cardinal_shading() {
        let face = FaceDef {
            uv: Some([0.0, 0.0, 16.0, 16.0]),
            texture: "side".to_string(),
            cullface: None,
            rotation: None,
            tint_index: None,
        };
        let resolved = ResolvedModel {
            textures: HashMap::from([("side".to_string(), "block/piston_side".to_string())]),
            elements: vec![ElementDef {
                from: [6.0, 6.0, 4.0],
                to: [10.0, 10.0, 20.0],
                rotation: None,
                faces: HashMap::from([("west".to_string(), face)]),
                shade: true,
            }],
            gui_transform: DisplayTransform::IDENTITY,
            gui_light: GuiLight::Side,
            ground_transform: Mat4::IDENTITY,
        };

        let baked = bake_resolved_model(&resolved, 0, 270, Tint::None).unwrap();
        assert_eq!(baked.quads.len(), 1);
        assert!((baked.quads[0].shade_light - Direction::South.shade_light()).abs() < 1.0e-6);
    }

    /// Every face must show the full-tile texture upright at rotation 0 and
    /// spin it clockwise per 90 degrees, vanilla's `FaceInfo` +
    /// `CuboidFace.UVs` + `Quadrant` behavior (the piston's side faces use
    /// 90/270 and read 180 degrees off when the direction is inverted).
    #[test]
    fn face_uvs_show_upright_clockwise_rotated_texture() {
        // Image corners in clockwise order; screen corner `s` under a
        // clockwise rotation by `steps` shows image corner `(s - steps) % 4`.
        const IMAGE_CORNERS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for dir in DIRS {
            let (right, up) = face_axes(dir);
            let positions = face_positions(dir, [0.0; 3], [1.0; 3]).map(Vec3::from_array);
            for (rot, steps) in [
                (None, 0),
                (Some(0), 0),
                (Some(90), 1),
                (Some(180), 2),
                (Some(270), 3),
            ] {
                let uvs = face_uvs(dir, [0.0; 3], [1.0; 3], Some(&[0.0, 0.0, 16.0, 16.0]), rot);
                let mid = |axis: Vec3| {
                    let coords = positions.map(|p| p.dot(axis));
                    (coords.iter().copied().fold(f32::INFINITY, f32::min)
                        + coords.iter().copied().fold(f32::NEG_INFINITY, f32::max))
                        / 2.0
                };
                let (right_mid, up_mid) = (mid(right), mid(up));
                for (p, uv) in positions.iter().zip(uvs) {
                    let screen_corner = match (p.dot(right) > right_mid, p.dot(up) > up_mid) {
                        (false, true) => 0,
                        (true, true) => 1,
                        (true, false) => 2,
                        (false, false) => 3,
                    };
                    let expected = IMAGE_CORNERS[(screen_corner + 4 - steps) % 4];
                    assert_eq!(uv, expected, "{dir:?} rotation {rot:?} vertex {p:?}");
                }
            }
        }
    }

    /// Default UV windows per face, vanilla `FaceBakery.defaultFaceUV`: the
    /// down/north/east windows reflect about the texture center.
    #[test]
    fn default_uv_windows_match_vanilla() {
        let from = [2.0 / 16.0, 3.0 / 16.0, 4.0 / 16.0];
        let to = [8.0 / 16.0, 9.0 / 16.0, 10.0 / 16.0];
        let expected = [
            (Direction::Down, (0.125, 0.375, 0.5, 0.75)),
            (Direction::Up, (0.125, 0.25, 0.5, 0.625)),
            (Direction::North, (0.5, 0.4375, 0.875, 0.8125)),
            (Direction::South, (0.125, 0.4375, 0.5, 0.8125)),
            (Direction::West, (0.25, 0.4375, 0.625, 0.8125)),
            (Direction::East, (0.375, 0.4375, 0.75, 0.8125)),
        ];
        for (dir, (u1, v1, u2, v2)) in expected {
            let uvs = face_uvs(dir, from, to, None, None);
            // The cycle assigns vertex 0 the (u1, v1) corner and vertex 2 the
            // (u2, v2) corner.
            assert_eq!(uvs[0], [u1, v1], "{dir:?} window origin");
            assert_eq!(uvs[2], [u2, v2], "{dir:?} window extent");
        }
    }

    /// The bed shape: a composite item model contributes every child, the
    /// foot carrying its one-block translation.
    #[test]
    fn composite_item_collects_all_parts() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{
                "model": {
                    "type": "minecraft:composite",
                    "models": [
                        {"type": "minecraft:model", "model": "minecraft:block/red_bed_head"},
                        {
                            "type": "minecraft:model",
                            "model": "minecraft:block/red_bed_foot",
                            "transformation": {"translation": [0.0, 0.0, 1.0]}
                        }
                    ]
                }
            }"#,
        )
        .unwrap();
        let parts = collect_model_parts(&json);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].path, "block/red_bed_head");
        assert!(parts[0].transform.is_none());
        assert_eq!(parts[1].path, "block/red_bed_foot");
        let moved = parts[1].transform.unwrap().transform_point3(Vec3::ZERO);
        assert!((moved - Vec3::new(0.0, 0.0, 1.0)).length() < 1e-6);
    }

    #[test]
    fn unsupported_item_transformation_encoding_is_rejected() {
        let raw_matrix = serde_json::json!([
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0
        ]);
        let axis_angle = serde_json::json!({
            "left_rotation": {"axis": [0.0, 1.0, 0.0], "angle": 90.0}
        });
        assert!(parse_item_transformation(&raw_matrix).is_none());
        assert!(parse_item_transformation(&axis_angle).is_none());
    }

    /// Non-composite trees (bundles' select/condition) keep the old
    /// first-model-string behavior.
    #[test]
    fn select_item_falls_back_to_first_model() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{
                "model": {
                    "type": "minecraft:select",
                    "cases": [
                        {"model": {"type": "minecraft:model", "model": "minecraft:item/bundle_open"}}
                    ],
                    "fallback": {"type": "minecraft:model", "model": "minecraft:item/bundle"}
                }
            }"#,
        )
        .unwrap();
        let parts = collect_model_parts(&json);
        assert_eq!(parts.len(), 1);
        assert!(parts[0].transform.is_none());
        let legacy = find_first_model_string(&json).unwrap();
        assert_eq!(parts[0].path, strip_mc_prefix(&legacy));
    }

    use crate::test_util::test_temp_dir;

    #[test]
    fn resolved_model_inherits_gui_transform() {
        let root = test_temp_dir("item_gui_transform");
        let models = root.join("minecraft/models/block");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(
            models.join("child.json"),
            r#"{"parent":"minecraft:block/parent"}"#,
        )
        .unwrap();
        std::fs::write(
            models.join("parent.json"),
            r#"{"gui_light":"front","display":{"gui":{"rotation":[30,135,0],"scale":[0.625,0.625,0.625]}}}"#,
        )
        .unwrap();

        let mut cache = HashMap::new();
        let resolved = resolve_model("block/child", &root, &None, &mut cache, None);
        assert_eq!(resolved.gui_transform, gui_transform(135.0));
        assert_eq!(resolved.gui_light, GuiLight::Front);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn item_definition_and_display_transforms_follow_resource_pack_override() {
        let root = test_temp_dir("item_model_pack");
        let jar = root.join("jar");
        let instance = root.join("instance");
        let items = jar.join("minecraft/items");
        let models = jar.join("minecraft/models/item");
        let pack_items = instance.join("resourcepacks/test_pack/assets/minecraft/items");
        let pack_models = instance.join("resourcepacks/test_pack/assets/other/models/item");
        std::fs::create_dir_all(&items).unwrap();
        std::fs::create_dir_all(&models).unwrap();
        std::fs::create_dir_all(&pack_items).unwrap();
        std::fs::create_dir_all(&pack_models).unwrap();
        std::fs::write(
            instance.join("resourcepacks/test_pack/pack.mcmeta"),
            r#"{"pack":{"pack_format":84,"description":"test"}}"#,
        )
        .unwrap();
        std::fs::write(
            items.join("test_item.json"),
            r#"{"model":{"type":"minecraft:model","model":"minecraft:item/base"}}"#,
        )
        .unwrap();
        std::fs::write(
            models.join("base.json"),
            r#"{"parent":"minecraft:item/generated","textures":{"layer0":"minecraft:item/base"}}"#,
        )
        .unwrap();
        std::fs::write(
            models.join("generated.json"),
            r#"{"display":{"ground":{"translation":[0,2,0],"scale":[0.5,0.5,0.5]}}}"#,
        )
        .unwrap();
        std::fs::write(
            pack_items.join("test_item.json"),
            r#"{"model":{"type":"minecraft:model","model":"other:item/replacement"}}"#,
        )
        .unwrap();
        std::fs::write(
            pack_models.join("replacement.json"),
            r#"{"parent":"minecraft:item/generated","textures":{"layer0":"other:item/replacement"},"display":{"gui":{"translation":[32,-96,0],"scale":[8,-8,1]}}}"#,
        )
        .unwrap();
        // An item the jar does not define at all.
        std::fs::write(
            pack_items.join("pack_only.json"),
            r#"{"model":{"type":"minecraft:model","model":"other:item/replacement"}}"#,
        )
        .unwrap();

        let mut packs = crate::resource_pack::ResourcePackManager::new(&instance);
        packs.enable_local_pack("test_pack");
        let baked = bake_item_models(&jar, &None, Some(&packs));
        assert_eq!(
            baked.flat_texture_keys.get("test_item").map(String::as_str),
            Some("other:item/replacement")
        );
        assert_eq!(
            baked.flat_texture_keys.get("pack_only").map(String::as_str),
            Some("other:item/replacement")
        );
        assert_eq!(
            baked.gui_transforms["test_item"],
            DisplayTransform {
                rotation: Vec3::ZERO,
                translation: Vec3::new(2.0, -5.0, 0.0),
                scale: Vec3::new(4.0, -4.0, 1.0),
            }
        );
        let transform = baked.ground_transforms["test_item"];
        let origin = transform.transform_point3(Vec3::ZERO);
        assert!((origin - Vec3::new(0.0, 2.0 / 16.0, 0.0)).length() < 1.0e-6);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bare_face_texture_slot_matches_vanilla_texture_slots() {
        let root = test_temp_dir("bare_texture_slot");
        let models = root.join("minecraft/models/block");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(
            models.join("test_core.json"),
            r#"{
                "textures":{"all":"block/test_core"},
                "elements":[{
                    "from":[4,0,4],"to":[12,8,12],
                    "faces":{
                        "down":{"texture":"all"},"up":{"texture":"all"},
                        "north":{"texture":"all"},"south":{"texture":"all"},
                        "west":{"texture":"all"},"east":{"texture":"all"}
                    }
                }]
            }"#,
        )
        .unwrap();

        let mut cache = HashMap::new();
        let resolved = resolve_model("block/test_core", &root, &None, &mut cache, None);
        let baked = bake_resolved_model(&resolved, 0, 0, Tint::None).unwrap();
        assert_eq!(baked.quads.len(), 6);
        assert!(baked.quads.iter().all(|quad| quad.texture == "test_core"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ground_display_transform_matches_vanilla_centered_mesh() {
        let json = serde_json::json!({
            "rotation": [0.0, 0.0, 0.0],
            "translation": [0.0, 3.0, 0.0],
            "scale": [0.5, 0.5, 0.5]
        });
        let transform = parse_display_transform(&json).unwrap().to_matrix();

        // ItemTransform applies translation/rotation/scale and then recenters
        // vanilla's 0..1 model. Pomme stores its item mesh already centered,
        // so the equivalent matrix leaves out only that final -0.5 step.
        let min = transform.transform_point3(Vec3::splat(-0.5));
        let max = transform.transform_point3(Vec3::splat(0.5));
        assert!((min - Vec3::new(-0.25, -0.0625, -0.25)).length() < 1e-6);
        assert!((max - Vec3::new(0.25, 0.4375, 0.25)).length() < 1e-6);
    }

    #[test]
    fn plain_item_is_a_single_part() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"model": {"type": "minecraft:model", "model": "minecraft:block/oak_stairs"}}"#,
        )
        .unwrap();
        let parts = collect_model_parts(&json);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].path, "block/oak_stairs");
        assert!(parts[0].transform.is_none());
    }
}
