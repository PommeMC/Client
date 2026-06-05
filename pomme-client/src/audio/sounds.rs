use std::collections::HashMap;
use std::path::Path;

use crate::assets::{AssetIndex, resolve_asset_path};

/// A single playable variant for a sound event, as listed in `sounds.json`.
pub struct SoundVariant {
    /// The resource name, e.g. `music/menu/menu1` or `namespace:path`.
    pub name: String,
    pub weight: u32,
    /// Per-entry volume multiplier (defaults to 1.0 when unspecified).
    pub volume: f32,
}

/// Parsed `sounds.json`: a map from event name (e.g. `music.menu`) to its
/// variants.
pub struct SoundsIndex {
    events: HashMap<String, Vec<SoundVariant>>,
}

impl SoundsIndex {
    /// Loads and parses `minecraft/sounds.json` from the asset index / jar
    /// assets. Returns an empty index if the file is missing or malformed.
    pub fn load(jar_assets_dir: &Path, asset_index: &Option<AssetIndex>) -> Self {
        let path = resolve_asset_path(jar_assets_dir, asset_index, "minecraft/sounds.json");
        let mut events = HashMap::new();

        let Ok(content) = std::fs::read_to_string(&path) else {
            tracing::warn!("sounds.json not found at {}", path.display());
            return Self { events };
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
            tracing::warn!("failed to parse sounds.json");
            return Self { events };
        };
        let Some(obj) = json.as_object() else {
            return Self { events };
        };

        for (event, def) in obj {
            let Some(sounds) = def.get("sounds").and_then(|s| s.as_array()) else {
                continue;
            };
            let mut variants = Vec::new();
            for entry in sounds {
                match entry {
                    // Bare string form: `"music/menu/menu1"`.
                    serde_json::Value::String(name) => {
                        variants.push(SoundVariant {
                            name: name.clone(),
                            weight: 1,
                            volume: 1.0,
                        });
                    }
                    // Object form: `{ "name", "weight", "volume", "type", ... }`.
                    serde_json::Value::Object(map) => {
                        // `type: "event"` redirects to another event; not handled yet.
                        if map.get("type").and_then(|t| t.as_str()) == Some("event") {
                            continue;
                        }
                        if let Some(name) = map.get("name").and_then(|n| n.as_str()) {
                            let weight = map
                                .get("weight")
                                .and_then(|w| w.as_u64())
                                .unwrap_or(1)
                                .max(1) as u32;
                            let volume =
                                map.get("volume").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                            variants.push(SoundVariant {
                                name: name.to_string(),
                                weight,
                                volume,
                            });
                        }
                    }
                    _ => {}
                }
            }
            if !variants.is_empty() {
                events.insert(event.clone(), variants);
            }
        }

        Self { events }
    }

    pub fn variants(&self, event: &str) -> Option<&[SoundVariant]> {
        self.events.get(event).map(Vec::as_slice)
    }
}

/// Converts a `sounds.json` variant name into an asset key.
///
/// `music/menu/menu1` -> `minecraft/sounds/music/menu/menu1.ogg`
/// `namespace:path`   -> `namespace/sounds/path.ogg`
pub fn sound_asset_key(name: &str) -> String {
    let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
    format!("{ns}/sounds/{path}.ogg")
}
