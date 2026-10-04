use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::resource_pack::ResourcePackManager;
use crate::util::JavaRandom;

/// Vanilla `SoundEventRegistrationSerializer`'s `attenuation_distance` default.
const DEFAULT_ATTENUATION_DISTANCE: i32 = 16;

/// A single playable file variant from `sounds.json` after resolving any
/// `type: "event"` indirection.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundVariant {
    /// Resolved file in the current resource-pack stack.
    pub path: PathBuf,
    pub weight: u32,
    /// Per-entry volume multiplier (defaults to 1.0 when unspecified).
    pub volume: f32,
    /// Per-entry pitch multiplier (defaults to 1.0 when unspecified).
    pub pitch: f32,
    /// Whether Vanilla decodes this sound as a queued stream.
    pub stream: bool,
    /// Vanilla per-entry attenuation distance in blocks.
    pub attenuation_distance: f32,
}

#[derive(Clone, Debug)]
enum SoundEntry {
    File(SoundVariant),
    Event {
        name: String,
        volume: f32,
        pitch: f32,
        stream: bool,
    },
}

/// A sound event's entry in `sounds.json`: its weighted entries and optional
/// subtitle translation key.
struct SoundEvent {
    entries: Vec<SoundEntry>,
    subtitle: Option<String>,
}

/// Parsed `sounds.json` registry across the built-in assets and active resource
/// packs.
#[derive(Default)]
pub struct SoundsIndex {
    events: HashMap<String, SoundEvent>,
}

impl SoundsIndex {
    /// Loads the built-in `minecraft/sounds.json`, then applies every active
    /// resource-pack `sounds.json` from low to high priority. Higher packs
    /// append by default and reset an event only when `replace: true`,
    /// matching Vanilla.
    pub fn load(
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        packs: &ResourcePackManager,
    ) -> Self {
        let mut index = Self {
            events: HashMap::new(),
        };

        let resolve_sound = |name: &str| {
            let asset_key = sound_asset_key(name);
            if let Some(path) = packs.resolve_asset(&asset_key) {
                return Some(path);
            }
            if let Some(path) = asset_index.as_ref().and_then(|idx| idx.resolve(&asset_key)) {
                return Some(path);
            }
            let path = jar_assets_dir.join(&asset_key);
            path.is_file().then_some(path)
        };

        let base = resolve_asset_path(jar_assets_dir, asset_index, "minecraft/sounds.json");
        index.apply_file("minecraft", &base, &resolve_sound);

        for pack_dir in packs.active_pack_dirs() {
            let assets_dir = pack_dir.join("assets");
            let Ok(entries) = std::fs::read_dir(&assets_dir) else {
                continue;
            };
            let mut namespaces: Vec<_> = entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .collect();
            namespaces.sort_by_key(|entry| entry.file_name());
            for namespace in namespaces {
                let namespace_name = namespace.file_name().to_string_lossy().into_owned();
                let sounds_path = namespace.path().join("sounds.json");
                if sounds_path.is_file() {
                    index.apply_file(&namespace_name, &sounds_path, &resolve_sound);
                }
            }
        }

        index
    }

    /// Resolves an event to a concrete file using Vanilla's weighted selection
    /// semantics. Event redirects contribute the referenced event's total
    /// weight and consume another random draw when selected.
    pub fn choose(&self, event: &str, seed: Option<u64>) -> Option<SoundVariant> {
        let mut random = SoundRandom::new(seed);
        self.choose_inner(normalize_event_name(event), &mut random, &mut Vec::new())
    }

    /// A one-event index, so a test can dispatch a sound without a resource
    /// pack on disk.
    #[cfg(test)]
    pub fn for_test_event(event: &str) -> Self {
        let variant = SoundVariant {
            path: PathBuf::from("test.ogg"),
            weight: 1,
            volume: 1.0,
            pitch: 1.0,
            stream: false,
            attenuation_distance: DEFAULT_ATTENUATION_DISTANCE as f32,
        };
        Self {
            events: HashMap::from([(
                normalize_event_name(event).to_string(),
                SoundEvent {
                    entries: vec![SoundEntry::File(variant)],
                    subtitle: None,
                },
            )]),
        }
    }

    /// The subtitle translation key for an event, e.g.
    /// `subtitles.block.anvil.land`.
    pub fn subtitle(&self, event: &str) -> Option<&str> {
        self.events
            .get(normalize_event_name(event))?
            .subtitle
            .as_deref()
    }

    fn apply_file(
        &mut self,
        namespace: &str,
        path: &Path,
        resolve_sound: &impl Fn(&str) -> Option<PathBuf>,
    ) {
        let Ok(content) = std::fs::read_to_string(path) else {
            tracing::warn!("sounds.json not found at {}", path.display());
            return;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
            tracing::warn!("failed to parse sounds.json at {}", path.display());
            return;
        };
        if let Err(error) = self.apply_value(namespace, &json, resolve_sound) {
            tracing::warn!("invalid sounds.json at {}: {error}", path.display());
        }
    }

    fn apply_value(
        &mut self,
        namespace: &str,
        json: &serde_json::Value,
        resolve_sound: &impl Fn(&str) -> Option<PathBuf>,
    ) -> Result<(), String> {
        let registrations = self.parse_value(namespace, json, resolve_sound)?;
        for (event_name, replace, event) in registrations {
            match self.events.get_mut(&event_name) {
                Some(existing) if replace => *existing = event,
                Some(existing) => existing.entries.extend(event.entries),
                None => {
                    self.events.insert(event_name, event);
                }
            }
        }
        Ok(())
    }

    fn parse_value(
        &self,
        namespace: &str,
        json: &serde_json::Value,
        resolve_sound: &impl Fn(&str) -> Option<PathBuf>,
    ) -> Result<Vec<(String, bool, SoundEvent)>, String> {
        let obj = json
            .as_object()
            .ok_or_else(|| "top-level value must be an object".to_string())?;
        let mut registrations = Vec::with_capacity(obj.len());

        for (event_path, def) in obj {
            if !valid_resource_location_path(event_path) {
                return Err(format!("invalid sound event path {event_path:?}"));
            }
            let def = def
                .as_object()
                .ok_or_else(|| format!("sound event {event_path:?} must be an object"))?;
            let replace = match def.get("replace") {
                Some(value) => value
                    .as_bool()
                    .ok_or_else(|| format!("sound event {event_path:?} has non-boolean replace"))?,
                None => false,
            };
            let subtitle = match def.get("subtitle") {
                Some(value) => Some(
                    value
                        .as_str()
                        .ok_or_else(|| {
                            format!("sound event {event_path:?} has non-string subtitle")
                        })?
                        .to_string(),
                ),
                None => None,
            };
            let sounds: &[serde_json::Value] = match def.get("sounds") {
                Some(value) => value
                    .as_array()
                    .ok_or_else(|| format!("sound event {event_path:?} has non-array sounds"))?,
                None => &[],
            };
            let mut entries = Vec::with_capacity(sounds.len());
            for entry in sounds {
                if let Some(entry) = self.parse_entry(entry, resolve_sound)? {
                    entries.push(entry);
                }
            }
            registrations.push((
                event_key(namespace, event_path),
                replace,
                SoundEvent { entries, subtitle },
            ));
        }
        Ok(registrations)
    }

    fn parse_entry(
        &self,
        entry: &serde_json::Value,
        resolve_sound: &impl Fn(&str) -> Option<PathBuf>,
    ) -> Result<Option<SoundEntry>, String> {
        match entry {
            serde_json::Value::String(name) => {
                validate_sound_name(name)?;
                let Some(path) = resolve_sound(name) else {
                    return Ok(None);
                };
                Ok(Some(SoundEntry::File(SoundVariant {
                    path,
                    weight: 1,
                    volume: 1.0,
                    pitch: 1.0,
                    stream: false,
                    attenuation_distance: DEFAULT_ATTENUATION_DISTANCE as f32,
                })))
            }
            serde_json::Value::Object(map) => {
                let name = map
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "sound object is missing string name".to_string())?;
                validate_sound_name(name)?;
                let volume = json_f32(map, "volume", 1.0)?;
                if volume <= 0.0 {
                    return Err("invalid sound volume".to_string());
                }
                let pitch = json_f32(map, "pitch", 1.0)?;
                if pitch <= 0.0 {
                    return Err("invalid sound pitch".to_string());
                }
                let weight = json_i32(map, "weight", 1)?;
                if weight <= 0 {
                    return Err("invalid sound weight".to_string());
                }
                let stream = json_bool(map, "stream", false)?;
                let _preload = json_bool(map, "preload", false)?;
                let attenuation_distance =
                    json_i32(map, "attenuation_distance", DEFAULT_ATTENUATION_DISTANCE)? as f32;
                let sound_type = match map.get("type") {
                    Some(value) => value
                        .as_str()
                        .ok_or_else(|| "sound type must be a string".to_string())?,
                    None => "file",
                };

                match sound_type {
                    "event" => Ok(Some(SoundEntry::Event {
                        name: normalize_event_name(name).to_string(),
                        volume,
                        pitch,
                        stream,
                    })),
                    "file" => {
                        let Some(path) = resolve_sound(name) else {
                            return Ok(None);
                        };
                        Ok(Some(SoundEntry::File(SoundVariant {
                            path,
                            weight: weight as u32,
                            volume,
                            pitch,
                            stream,
                            attenuation_distance,
                        })))
                    }
                    other => Err(format!("invalid sound type {other:?}")),
                }
            }
            _ => Err("sound entry must be a string or object".to_string()),
        }
    }

    fn choose_inner(
        &self,
        event: &str,
        random: &mut SoundRandom,
        stack: &mut Vec<String>,
    ) -> Option<SoundVariant> {
        if stack.iter().any(|entry| entry == event) {
            tracing::warn!("cyclic sound event reference involving {event}");
            return None;
        }
        let sound_event = self.events.get(event)?;
        stack.push(event.to_string());

        let weights: Vec<u32> = sound_event
            .entries
            .iter()
            .map(|entry| self.entry_weight(entry, stack))
            .collect();
        let total = weights
            .iter()
            .try_fold(0_u32, |total, &weight| total.checked_add(weight))?;
        let mut pick = random.next(total)?;

        let selected = sound_event
            .entries
            .iter()
            .zip(weights)
            .find_map(|(entry, weight)| {
                if pick < weight {
                    Some(entry)
                } else {
                    pick -= weight;
                    None
                }
            })?;

        let result = match selected {
            SoundEntry::File(variant) => Some(variant.clone()),
            SoundEntry::Event {
                name,
                volume,
                pitch,
                stream,
            } => {
                let mut variant = self.choose_inner(name, random, stack)?;
                variant.volume *= *volume;
                variant.pitch *= *pitch;
                variant.stream |= *stream;
                Some(variant)
            }
        };
        stack.pop();
        result
    }

    fn event_weight(&self, event: &str, stack: &mut Vec<String>) -> u32 {
        if stack.iter().any(|entry| entry == event) {
            tracing::warn!("cyclic sound event reference involving {event}");
            return 0;
        }
        let Some(sound_event) = self.events.get(event) else {
            return 0;
        };
        stack.push(event.to_string());
        let total = sound_event.entries.iter().fold(0_u32, |total, entry| {
            total.saturating_add(self.entry_weight(entry, stack))
        });
        stack.pop();
        total
    }

    fn entry_weight(&self, entry: &SoundEntry, stack: &mut Vec<String>) -> u32 {
        match entry {
            SoundEntry::File(variant) => variant.weight,
            SoundEntry::Event { name, .. } => self.event_weight(name, stack),
        }
    }
}

enum SoundRandom {
    Seeded(JavaRandom),
    Unseeded,
}

impl SoundRandom {
    fn new(seed: Option<u64>) -> Self {
        match seed {
            Some(seed) => Self::Seeded(JavaRandom::new(seed as i64)),
            None => Self::Unseeded,
        }
    }

    fn next(&mut self, bound: u32) -> Option<u32> {
        if bound == 0 {
            return None;
        }
        match self {
            Self::Seeded(random) => {
                let bound = i32::try_from(bound).ok()?;
                Some(random.next_int(bound) as u32)
            }
            Self::Unseeded => Some(fastrand::u32(0..bound)),
        }
    }
}

fn event_key(namespace: &str, path: &str) -> String {
    if namespace == "minecraft" {
        path.to_string()
    } else {
        format!("{namespace}:{path}")
    }
}

fn normalize_event_name(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

fn validate_sound_name(name: &str) -> Result<(), String> {
    let (namespace, path) = name.split_once(':').unwrap_or(("minecraft", name));
    if namespace.is_empty()
        || !namespace.bytes().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-' | b'.')
        })
        || !valid_resource_location_path(path)
    {
        return Err(format!("invalid sound identifier {name:?}"));
    }
    Ok(())
}

fn valid_resource_location_path(path: &str) -> bool {
    !path.is_empty()
        && path.bytes().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-' | b'.' | b'/')
        })
}

fn json_bool(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: bool,
) -> Result<bool, String> {
    match map.get(key) {
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("sound {key} must be a boolean")),
        None => Ok(default),
    }
}

fn json_f32(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: f32,
) -> Result<f32, String> {
    match map.get(key) {
        Some(value) => value
            .as_f64()
            .map(|value| value as f32)
            .ok_or_else(|| format!("sound {key} must be a number")),
        None => Ok(default),
    }
}

fn json_i32(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: i32,
) -> Result<i32, String> {
    match map.get(key) {
        Some(value) => {
            let value = value
                .as_i64()
                .ok_or_else(|| format!("sound {key} must be an integer"))?;
            i32::try_from(value).map_err(|_| format!("sound {key} is out of range"))
        }
        None => Ok(default),
    }
}

/// Converts a `sounds.json` file variant name into an asset key.
///
/// `music/menu/menu1` -> `minecraft/sounds/music/menu/menu1.ogg`
/// `namespace:path`   -> `namespace/sounds/path.ogg`
pub fn sound_asset_key(name: &str) -> String {
    let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
    format!("{ns}/sounds/{path}.ogg")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, weight: u32) -> SoundEntry {
        SoundEntry::File(SoundVariant {
            path: PathBuf::from(name),
            weight,
            volume: 1.0,
            pitch: 1.0,
            stream: false,
            attenuation_distance: DEFAULT_ATTENUATION_DISTANCE as f32,
        })
    }

    fn resolve_test_sound(name: &str) -> Option<PathBuf> {
        (!name.starts_with("missing")).then(|| PathBuf::from(name))
    }

    #[test]
    fn sound_asset_key_handles_default_and_explicit_namespaces() {
        assert_eq!(
            sound_asset_key("music/menu/menu1"),
            "minecraft/sounds/music/menu/menu1.ogg"
        );
        assert_eq!(sound_asset_key("mod:foo/bar"), "mod/sounds/foo/bar.ogg");
    }

    #[test]
    fn stacked_registrations_append_and_replace_like_vanilla() {
        let mut index = SoundsIndex {
            events: HashMap::new(),
        };
        index
            .apply_value(
                "minecraft",
                &serde_json::json!({
                    "test": {"subtitle": "low", "sounds": ["a"]}
                }),
                &resolve_test_sound,
            )
            .unwrap();
        index
            .apply_value(
                "minecraft",
                &serde_json::json!({
                    "test": {"subtitle": "ignored", "sounds": ["b"]}
                }),
                &resolve_test_sound,
            )
            .unwrap();
        let event = index.events.get("test").unwrap();
        assert_eq!(event.entries.len(), 2);
        assert_eq!(event.subtitle.as_deref(), Some("low"));

        index
            .apply_value(
                "minecraft",
                &serde_json::json!({
                    "test": {"replace": true, "subtitle": "high", "sounds": ["c"]}
                }),
                &resolve_test_sound,
            )
            .unwrap();
        let event = index.events.get("test").unwrap();
        assert_eq!(event.entries.len(), 1);
        assert_eq!(event.subtitle.as_deref(), Some("high"));
    }

    #[test]
    fn resource_pack_namespace_qualifies_event_name() {
        let mut index = SoundsIndex {
            events: HashMap::new(),
        };
        index
            .apply_value(
                "custom",
                &serde_json::json!({"event": {"sounds": ["custom:clip"]}}),
                &resolve_test_sound,
            )
            .unwrap();
        assert!(index.events.contains_key("custom:event"));
    }

    #[test]
    fn missing_file_entries_are_excluded_from_weighted_selection() {
        let mut index = SoundsIndex {
            events: HashMap::new(),
        };
        index
            .apply_value(
                "minecraft",
                &serde_json::json!({
                    "test": {"sounds": [
                        {"name": "missing/file", "weight": 10},
                        {"name": "present/file", "weight": 1}
                    ]}
                }),
                &resolve_test_sound,
            )
            .unwrap();
        let event = index.events.get("test").unwrap();
        assert_eq!(event.entries.len(), 1);
        assert_eq!(
            index.choose("test", Some(0)).unwrap().path,
            PathBuf::from("present/file")
        );
    }

    #[test]
    fn malformed_resource_layer_is_rejected_atomically() {
        let mut index = SoundsIndex {
            events: HashMap::new(),
        };
        index
            .apply_value(
                "minecraft",
                &serde_json::json!({
                    "test": {"subtitle": "base", "sounds": ["present/base"]}
                }),
                &resolve_test_sound,
            )
            .unwrap();

        let result = index.apply_value(
            "minecraft",
            &serde_json::json!({
                "test": {"replace": true, "subtitle": "replacement", "sounds": ["present/new"]},
                "broken": {"sounds": [{"name": "present/bad", "weight": 0}]}
            }),
            &resolve_test_sound,
        );
        assert!(result.is_err());

        let event = index.events.get("test").unwrap();
        assert_eq!(event.subtitle.as_deref(), Some("base"));
        assert_eq!(event.entries.len(), 1);
        assert!(!index.events.contains_key("broken"));
    }

    #[test]
    fn unknown_type_rejects_resource_but_negative_attenuation_is_valid() {
        let mut index = SoundsIndex {
            events: HashMap::new(),
        };
        assert!(
            index
                .apply_value(
                    "minecraft",
                    &serde_json::json!({
                        "broken": {"sounds": [{"name": "present/file", "type": "mystery"}]}
                    }),
                    &resolve_test_sound,
                )
                .is_err()
        );
        assert!(index.events.is_empty());

        index
            .apply_value(
                "minecraft",
                &serde_json::json!({
                    "test": {"sounds": [{"name": "present/file", "attenuation_distance": -4}]}
                }),
                &resolve_test_sound,
            )
            .unwrap();
        let SoundEntry::File(variant) = &index.events.get("test").unwrap().entries[0] else {
            panic!("expected file sound");
        };
        assert_eq!(variant.attenuation_distance, -4.0);
    }

    #[test]
    fn event_redirect_uses_nested_random_draw_and_multiplies_parameters() {
        let index = SoundsIndex {
            events: HashMap::from([
                (
                    "target".to_string(),
                    SoundEvent {
                        entries: vec![file("first", 1), file("second", 1)],
                        subtitle: None,
                    },
                ),
                (
                    "parent".to_string(),
                    SoundEvent {
                        entries: vec![SoundEntry::Event {
                            name: "target".to_string(),
                            volume: 0.5,
                            pitch: 2.0,
                            stream: true,
                        }],
                        subtitle: None,
                    },
                ),
            ]),
        };

        // LegacyRandomSource(1) draws 1 then 0 for nextInt(2). The first draw
        // selects the only redirect; the nested draw therefore selects `first`.
        let resolved = index.choose("parent", Some(1)).unwrap();
        assert_eq!(resolved.path, PathBuf::from("first"));
        assert_eq!(resolved.volume, 0.5);
        assert_eq!(resolved.pitch, 2.0);
        assert!(resolved.stream);
        assert_eq!(resolved.attenuation_distance, 16.0);
    }

    #[test]
    fn missing_event_redirect_has_zero_weight_without_hiding_siblings() {
        let index = SoundsIndex {
            events: HashMap::from([(
                "parent".to_string(),
                SoundEvent {
                    entries: vec![
                        SoundEntry::Event {
                            name: "missing".to_string(),
                            volume: 1.0,
                            pitch: 1.0,
                            stream: false,
                        },
                        file("fallback", 1),
                    ],
                    subtitle: None,
                },
            )]),
        };
        assert_eq!(
            index.choose("parent", Some(0)).unwrap().path,
            PathBuf::from("fallback")
        );
    }

    #[test]
    fn cyclic_event_redirect_is_rejected() {
        let index = SoundsIndex {
            events: HashMap::from([
                (
                    "a".to_string(),
                    SoundEvent {
                        entries: vec![SoundEntry::Event {
                            name: "b".to_string(),
                            volume: 1.0,
                            pitch: 1.0,
                            stream: false,
                        }],
                        subtitle: None,
                    },
                ),
                (
                    "b".to_string(),
                    SoundEvent {
                        entries: vec![SoundEntry::Event {
                            name: "a".to_string(),
                            volume: 1.0,
                            pitch: 1.0,
                            stream: false,
                        }],
                        subtitle: None,
                    },
                ),
            ]),
        };
        assert!(index.choose("a", Some(0)).is_none());
    }
}
