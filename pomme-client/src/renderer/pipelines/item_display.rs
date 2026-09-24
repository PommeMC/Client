use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(crate) use crate::world::block::model::DisplayTransform;
use crate::world::block::model::{first_item_model_ref, parse_display_transform, strip_mc_prefix};

const MODEL_PARENT_LIMIT: u32 = 16;

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

fn resolve_item_model_path(name: &str, items_dir: &Path) -> Option<String> {
    let item_json = read_json(&items_dir.join(format!("{name}.json")))?;
    first_item_model_ref(&item_json)
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
            .map(|p| strip_mc_prefix(p).to_string());
    }

    None
}
