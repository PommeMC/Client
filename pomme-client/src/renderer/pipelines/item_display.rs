use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::{Mat4, Vec3};

use crate::world::block::model::{find_first_model_string, find_first_string_for_key};

const MODEL_PARENT_LIMIT: u32 = 16;

#[derive(Debug, Clone, Copy)]
pub struct DisplayTransform {
    pub rotation: Vec3,
    pub translation: Vec3,
    pub scale: Vec3,
}

impl DisplayTransform {
    pub const IDENTITY: Self = Self {
        rotation: Vec3::ZERO,
        translation: Vec3::ZERO,
        scale: Vec3::ONE,
    };

    pub fn to_matrix(self) -> Mat4 {
        let t = Mat4::from_translation(self.translation);
        let r = Mat4::from_rotation_x(self.rotation.x.to_radians())
            * Mat4::from_rotation_y(self.rotation.y.to_radians())
            * Mat4::from_rotation_z(self.rotation.z.to_radians());
        let s = Mat4::from_scale(self.scale);
        t * r * s
    }
}

/// Per-item cache of one `display.<key>` transform, resolved from the item's
/// model JSON parent chain.
pub struct DisplayResolver {
    key: &'static str,
    cache: RefCell<HashMap<String, DisplayTransform>>,
    items_dir: PathBuf,
    models_dir: PathBuf,
}

impl DisplayResolver {
    pub fn new(jar_assets_dir: &Path, key: &'static str) -> Self {
        let mc_base = jar_assets_dir.join("minecraft");
        Self {
            key,
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
            .and_then(|path| resolve_display(&path, &self.models_dir, self.key))
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

fn strip_mc_ns(s: &str) -> &str {
    s.strip_prefix("minecraft:").unwrap_or(s)
}

fn resolve_item_model_path(name: &str, items_dir: &Path) -> Option<String> {
    let item_json = read_json(&items_dir.join(format!("{name}.json")))?;
    let model_path = find_first_model_string(&item_json)
        .or_else(|| find_first_string_for_key(&item_json, "base"))?;
    Some(strip_mc_ns(&model_path).to_string())
}

fn parse_vec3(value: &serde_json::Value, default: Vec3) -> Vec3 {
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

fn parse_display_transform(json: &serde_json::Value) -> Option<DisplayTransform> {
    let obj = json.as_object()?;
    let rotation = obj
        .get("rotation")
        .map(|v| parse_vec3(v, Vec3::ZERO))
        .unwrap_or(Vec3::ZERO);
    let translation = obj
        .get("translation")
        .map(|v| parse_vec3(v, Vec3::ZERO))
        .unwrap_or(Vec3::ZERO);
    let scale = obj
        .get("scale")
        .map(|v| parse_vec3(v, Vec3::ONE))
        .unwrap_or(Vec3::ONE);
    Some(DisplayTransform {
        rotation,
        translation: translation * (1.0 / 16.0),
        scale,
    })
}

/// First `display.<key>` transform found walking up the model parent chain.
fn resolve_display(start_path: &str, models_dir: &Path, key: &str) -> Option<DisplayTransform> {
    let mut current = Some(start_path.to_string());
    let mut depth = 0u32;
    while let Some(path) = current.take() {
        if depth >= MODEL_PARENT_LIMIT {
            break;
        }
        depth += 1;

        let file = models_dir.join(format!("{path}.json"));
        let json = read_json(&file)?;

        if let Some(entry) = json.get("display").and_then(|d| d.get(key))
            && let Some(t) = parse_display_transform(entry)
        {
            return Some(t);
        }

        current = json
            .get("parent")
            .and_then(|p| p.as_str())
            .map(|p| strip_mc_ns(p).to_string());
    }

    None
}
