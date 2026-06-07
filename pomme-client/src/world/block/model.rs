use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::registry::{FaceTextures, Tint};
use crate::assets::{AssetIndex, resolve_asset_path_with_packs};

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

    fn shade_light(&self) -> f32 {
        match self {
            Direction::Up => 1.0,
            Direction::Down => 0.5,
            Direction::North | Direction::South => 0.8,
            Direction::East | Direction::West => 0.6,
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
    pub shade_light: f32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BakedModel {
    pub quads: Vec<BakedQuad>,
    pub is_full_cube: bool,
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

const GRASS_TINTED: &[&str] = &[
    "grass_block",
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
                    if let Some(baked) =
                        bake_resolved_model(&resolved, model_ref.x, model_ref.y, block_tint)
                    {
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

pub fn bake_item_models(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&crate::resource_pack::ResourcePackManager>,
) -> (
    HashMap<String, BakedModel>,
    HashSet<String>,
    HashMap<String, String>,
) {
    let mut item_models: HashMap<String, BakedModel> = HashMap::new();
    let mut item_textures: HashSet<String> = HashSet::new();
    let mut flat_keys: HashMap<String, String> = HashMap::new();
    let mut model_cache: HashMap<String, ModelFile> = HashMap::new();

    let items_dir = jar_assets_dir.join("minecraft").join("items");
    let entries = match std::fs::read_dir(&items_dir) {
        Ok(e) => e,
        Err(_) => return (item_models, item_textures, flat_keys),
    };

    for entry in entries.flatten() {
        let fname = entry.file_name().to_string_lossy().to_string();
        let Some(item_name) = fname.strip_suffix(".json") else {
            continue;
        };
        let Ok(contents) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(json): Result<serde_json::Value, _> = serde_json::from_str(&contents) else {
            continue;
        };

        let raw_path =
            find_first_model_string(&json).or_else(|| find_first_string_for_key(&json, "base"));
        let Some(raw_path) = raw_path else { continue };
        let path = raw_path
            .strip_prefix("minecraft:")
            .unwrap_or(&raw_path)
            .to_string();

        let resolved = resolve_model(&path, jar_assets_dir, asset_index, &mut model_cache, packs);

        if resolved.elements.is_empty() {
            if let Some(value) = resolved.textures.get("layer0") {
                let stripped = value.strip_prefix("minecraft:").unwrap_or(value);
                let key = if let Some(rest) = stripped.strip_prefix("block/") {
                    rest.to_string()
                } else {
                    item_textures.insert(stripped.to_string());
                    stripped.to_string()
                };
                flat_keys.insert(item_name.to_string(), key);
            }
            continue;
        }

        let tint = determine_tint(item_name);
        if let Some(mut baked) = bake_resolved_model(&resolved, 0, 0, tint) {
            apply_gui_lambert(&mut baked.quads, BLOCK_GUI_ROTATION_DEG);
            item_models.insert(item_name.to_string(), baked);
        }
    }

    item_models.insert("chest".to_string(), bake_chest_item_model());
    flat_keys.remove("chest");

    tracing::info!(
        "Baked {} item models, {} flat items, and registered {} item textures",
        item_models.len(),
        flat_keys.len(),
        item_textures.len()
    );
    (item_models, item_textures, flat_keys)
}

pub fn bake_chest_item_model() -> BakedModel {
    let tex = "entity/chest/normal";
    let mut quads = Vec::new();
    let shades = vanilla_gui_face_shades(CHEST_GUI_ROTATION_DEG);
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
    }
}

const CHEST_GUI_ROTATION_DEG: [f32; 3] = [30.0, 45.0, 0.0];
const BLOCK_GUI_ROTATION_DEG: [f32; 3] = [30.0, 225.0, 0.0];

fn rotate_y(v: [f32; 3], angle: f32) -> [f32; 3] {
    let (s, c) = angle.sin_cos();
    [c * v[0] + s * v[2], v[1], -s * v[0] + c * v[2]]
}

fn rotate_x(v: [f32; 3], angle: f32) -> [f32; 3] {
    let (s, c) = angle.sin_cos();
    [v[0], c * v[1] - s * v[2], s * v[1] + c * v[2]]
}

fn items_3d_lights() -> ([f32; 3], [f32; 3]) {
    let base = |x: f32, y: f32, z: f32| {
        let len = (x * x + y * y + z * z).sqrt();
        [x / len, y / len, z / len]
    };
    let transform = |v: [f32; 3]| {
        let v = rotate_y(v, -std::f32::consts::PI / 8.0);
        let v = rotate_x(v, 2.3561945);
        let v = rotate_y(v, 1.0821041);
        let v = rotate_x(v, 3.2375858);
        [v[0], -v[1], v[2]]
    };
    (
        transform(base(0.2, 1.0, -0.7)),
        transform(base(-0.2, 1.0, 0.7)),
    )
}

fn lambert_shade(world_normal: [f32; 3], l0: [f32; 3], l1: [f32; 3]) -> f32 {
    let d0 = (l0[0] * world_normal[0] + l0[1] * world_normal[1] + l0[2] * world_normal[2]).max(0.0);
    let d1 = (l1[0] * world_normal[0] + l1[1] * world_normal[1] + l1[2] * world_normal[2]).max(0.0);
    ((d0 + d1) * 0.6 + 0.4).min(1.0)
}

fn rotate_mesh_normal(n_mesh: [f32; 3], rotation_deg: [f32; 3]) -> [f32; 3] {
    let after_y = rotate_y(n_mesh, rotation_deg[1].to_radians());
    rotate_x(after_y, rotation_deg[0].to_radians())
}

fn vanilla_gui_face_shades(rotation_deg: [f32; 3]) -> [f32; 6] {
    let (l0, l1) = items_3d_lights();
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
        shades[i] = lambert_shade(rotate_mesh_normal(n, rotation_deg), l0, l1);
    }
    shades
}

fn apply_gui_lambert(quads: &mut [BakedQuad], rotation_deg: [f32; 3]) {
    let (l0, l1) = items_3d_lights();
    for quad in quads {
        let p = &quad.positions;
        let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
        let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
        let nx = e1[1] * e2[2] - e1[2] * e2[1];
        let ny = e1[2] * e2[0] - e1[0] * e2[2];
        let nz = e1[0] * e2[1] - e1[1] * e2[0];
        let len = (nx * nx + ny * ny + nz * nz).sqrt();
        if len < 1e-6 {
            continue;
        }
        let n_mesh = [nx / len, ny / len, nz / len];
        let n_world = rotate_mesh_normal(n_mesh, rotation_deg);
        quad.shade_light *= lambert_shade(n_world, l0, l1);
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
    let face_specs: [FaceSpec; 6] = [
        FaceSpec {
            positions: [[x0, y1, z1], [x1, y1, z1], [x1, y1, z0], [x0, y1, z0]],
            uv_pixels: (u + d + w, v, u + d + w + w, v + d),
            uv_pattern: UvPattern::Up,
            shade: shades[0],
        },
        FaceSpec {
            positions: [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
            uv_pixels: (u + d, v, u + d + w, v + d),
            uv_pattern: UvPattern::Down,
            shade: shades[1],
        },
        FaceSpec {
            positions: [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]],
            uv_pixels: (u + d, v + d, u + d + w, v + d + h),
            uv_pattern: UvPattern::North,
            shade: shades[2],
        },
        FaceSpec {
            positions: [[x1, y0, z1], [x1, y1, z1], [x0, y1, z1], [x0, y0, z1]],
            uv_pixels: (u + d + w + d, v + d, u + d + w + d + w, v + d + h),
            uv_pattern: UvPattern::SouthWestEast,
            shade: shades[3],
        },
        FaceSpec {
            positions: [[x0, y0, z1], [x0, y1, z1], [x0, y1, z0], [x0, y0, z0]],
            uv_pixels: (u, v + d, u + d, v + d + h),
            uv_pattern: UvPattern::SouthWestEast,
            shade: shades[4],
        },
        FaceSpec {
            positions: [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
            uv_pixels: (u + d + w, v + d, u + d + w + d, v + d + h),
            uv_pattern: UvPattern::SouthWestEast,
            shade: shades[5],
        },
    ];
    for spec in face_specs {
        let (u_min_px, v_min_px, u_max_px, v_max_px) = spec.uv_pixels;
        let u1 = (u_min_px + 0.5) / TEX_SIZE;
        let v1 = (v_min_px + 0.5) / TEX_SIZE;
        let u2 = (u_max_px - 0.5) / TEX_SIZE;
        let v2 = (v_max_px - 0.5) / TEX_SIZE;
        let uvs = match spec.uv_pattern {
            UvPattern::Up => [[u1, v2], [u2, v2], [u2, v1], [u1, v1]],
            UvPattern::Down => [[u1, v1], [u2, v1], [u2, v2], [u1, v2]],
            UvPattern::North => [[u1, v2], [u1, v1], [u2, v1], [u2, v2]],
            UvPattern::SouthWestEast => [[u2, v2], [u2, v1], [u1, v1], [u1, v2]],
        };
        quads.push(BakedQuad {
            positions: spec.positions,
            uvs,
            texture: texture.to_string(),
            cullface: None,
            tint: super::registry::Tint::None,
            shade_light: spec.shade,
        });
    }
}

struct FaceSpec {
    positions: [[f32; 3]; 4],
    uv_pixels: (f32, f32, f32, f32),
    uv_pattern: UvPattern,
    shade: f32,
}

enum UvPattern {
    Up,
    Down,
    North,
    SouthWestEast,
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
    let model: ModelFile = serde_json::from_str(&contents).ok()?;
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
    let stripped = model_id.strip_prefix("minecraft:").unwrap_or(model_id);
    format!("minecraft/models/{stripped}.json")
}

fn texture_to_name(texture_ref: &str) -> Option<&str> {
    if texture_ref.starts_with('#') {
        return None;
    }
    let stripped = texture_ref
        .strip_prefix("minecraft:")
        .unwrap_or(texture_ref);
    stripped.strip_prefix("block/")
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

            let texture_ref = resolve_ref(&face_def.texture, &resolved.textures, 0);
            let Some(texture_name) = texture_to_name(&texture_ref) else {
                continue;
            };

            let positions = face_positions(dir, from, to);
            let uvs = face_uvs(dir, from, to, face_def.uv.as_ref(), face_def.rotation);

            let mut positions = apply_element_rotation(positions, &element.rotation);

            let mut cullface = face_def.cullface.as_deref().and_then(Direction::from_str);

            let shade_light = if element.shade {
                dir.shade_light()
            } else {
                1.0
            };
            let quad_tint = if face_def.tint_index.is_some() {
                tint
            } else {
                super::registry::Tint::None
            };

            if rot_x != 0 || rot_y != 0 {
                positions = rotate_positions(positions, rot_x, rot_y);
                cullface = cullface.map(|d| d.rotate_x(rot_x).rotate_y(rot_y));
            }

            quads.push(BakedQuad {
                positions,
                uvs,
                texture: texture_name.to_string(),
                cullface,
                tint: quad_tint,
                shade_light,
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

fn face_positions(dir: Direction, from: [f32; 3], to: [f32; 3]) -> [[f32; 3]; 4] {
    // CCW winding when viewed from outside, matching chunk pipeline backface
    // culling
    match dir {
        Direction::Up => [
            [from[0], to[1], to[2]],
            [to[0], to[1], to[2]],
            [to[0], to[1], from[2]],
            [from[0], to[1], from[2]],
        ],
        Direction::Down => [
            [from[0], from[1], from[2]],
            [to[0], from[1], from[2]],
            [to[0], from[1], to[2]],
            [from[0], from[1], to[2]],
        ],
        Direction::North => [
            [from[0], from[1], from[2]],
            [from[0], to[1], from[2]],
            [to[0], to[1], from[2]],
            [to[0], from[1], from[2]],
        ],
        Direction::South => [
            [to[0], from[1], to[2]],
            [to[0], to[1], to[2]],
            [from[0], to[1], to[2]],
            [from[0], from[1], to[2]],
        ],
        Direction::West => [
            [from[0], from[1], to[2]],
            [from[0], to[1], to[2]],
            [from[0], to[1], from[2]],
            [from[0], from[1], from[2]],
        ],
        Direction::East => [
            [to[0], from[1], from[2]],
            [to[0], to[1], from[2]],
            [to[0], to[1], to[2]],
            [to[0], from[1], to[2]],
        ],
    }
}

fn face_uvs(
    dir: Direction,
    from: [f32; 3],
    to: [f32; 3],
    explicit_uv: Option<&[f32; 4]>,
    rotation: Option<i32>,
) -> [[f32; 2]; 4] {
    let (u1, v1, u2, v2) = if let Some(uv) = explicit_uv {
        (uv[0] / 16.0, uv[1] / 16.0, uv[2] / 16.0, uv[3] / 16.0)
    } else {
        match dir {
            Direction::Up | Direction::Down => (from[0], from[2], to[0], to[2]),
            Direction::North | Direction::South => (from[0], 1.0 - to[1], to[0], 1.0 - from[1]),
            Direction::East | Direction::West => (from[2], 1.0 - to[1], to[2], 1.0 - from[1]),
        }
    };

    let mut uvs = match dir {
        Direction::Up => [[u1, v2], [u2, v2], [u2, v1], [u1, v1]],
        Direction::Down => [[u1, v1], [u2, v1], [u2, v2], [u1, v2]],
        Direction::North => [[u1, v2], [u1, v1], [u2, v1], [u2, v2]],
        Direction::South | Direction::West | Direction::East => {
            [[u2, v2], [u2, v1], [u1, v1], [u1, v2]]
        }
    };

    if let Some(rot) = rotation {
        let steps = ((rot % 360 + 360) % 360) / 90;
        for _ in 0..steps {
            uvs.rotate_right(1);
        }
    }

    uvs
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

fn rotate_positions(mut positions: [[f32; 3]; 4], rot_x: i32, rot_y: i32) -> [[f32; 3]; 4] {
    let center = 0.5f32;

    if rot_x != 0 {
        let angle = (rot_x as f32).to_radians();
        let cos = angle.cos();
        let sin = angle.sin();
        for pos in &mut positions {
            let dy = pos[1] - center;
            let dz = pos[2] - center;
            pos[1] = center + cos * dy - sin * dz;
            pos[2] = center + sin * dy + cos * dz;
        }
    }

    if rot_y != 0 {
        let angle = (rot_y as f32).to_radians();
        let cos = angle.cos();
        let sin = angle.sin();
        for pos in &mut positions {
            let dx = pos[0] - center;
            let dz = pos[2] - center;
            pos[0] = center + cos * dx + sin * dz;
            pos[2] = center - sin * dx + cos * dz;
        }
    }

    positions
}

fn build_face_textures(
    block_name: &str,
    textures: &HashMap<String, String>,
) -> Option<FaceTextures> {
    let get = |key: &str| -> Option<&str> { textures.get(key).and_then(|v| texture_to_name(v)) };

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
            up,
            down,
            north,
            south,
            east,
            west,
            side_overlay,
            tint,
        ));
    }

    if let Some(all) = get("all") {
        return Some(FaceTextures::uniform(all, tint));
    }

    if let (Some(end), Some(side)) = (get("end"), get("side")) {
        return Some(FaceTextures::new(
            end,
            end,
            side,
            side,
            side,
            side,
            None,
            Tint::None,
        ));
    }

    if let (Some(top), Some(side)) = (get("top"), get("side")) {
        let bottom = get("bottom").unwrap_or(top);
        return Some(FaceTextures::new(
            top, bottom, side, side, side, side, None, tint,
        ));
    }

    if let Some(cross) = get("cross") {
        return Some(FaceTextures::uniform(cross, tint));
    }

    if let (Some(front), Some(side)) = (get("front"), get("side")) {
        let top = get("top").or(get("end")).unwrap_or(side);
        let bottom = get("bottom").unwrap_or(top);
        return Some(FaceTextures::new(
            top,
            bottom,
            front,
            side,
            side,
            side,
            None,
            Tint::None,
        ));
    }

    if let Some(p) = get("particle") {
        return Some(FaceTextures::uniform(p, tint));
    }

    None
}

fn determine_tint(block_name: &str) -> Tint {
    if GRASS_TINTED.contains(&block_name) {
        Tint::Grass
    } else if FOLIAGE_TINTED.contains(&block_name) || block_name.ends_with("_leaves") {
        Tint::Foliage
    } else {
        Tint::None
    }
}
