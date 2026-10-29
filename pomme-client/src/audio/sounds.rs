use std::collections::HashMap;
use std::fmt;

use crate::resources::{
    ResourceContent, ResourceError, ResourceId, ResourceManager, ThreadSafeResourceContent,
};
use crate::util::JavaRandom;

/// Vanilla `SoundEventRegistrationSerializer`'s `attenuation_distance` default.
const DEFAULT_ATTENUATION_DISTANCE: i32 = 16;

const MAX_SOUNDS_JSON_BYTES: usize = 8 * 1024 * 1024;

/// A single playable file variant from `sounds.json` after resolving any
/// `type: "event"` indirection.
#[derive(Clone, Debug)]
pub struct SoundVariant {
    /// Exact selected source, transferable without reading the OGG content.
    pub asset: SoundAsset,
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

#[derive(Clone)]
pub(super) struct SoundAsset {
    pub content: ThreadSafeResourceContent,
}

impl fmt::Debug for SoundAsset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SoundAsset")
            .field("id", self.content.id())
            .field("origin", self.content.origin())
            .finish()
    }
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
    /// Applies each namespace's `sounds.json` stack from low to high priority.
    /// The manager supplies built-in and selected layers, including filters.
    /// Higher packs append by default and reset an event only when
    /// `replace: true`, matching Vanilla.
    pub fn load(resources: &ResourceManager) -> Result<Self, ResourceError> {
        let mut index = Self {
            events: HashMap::new(),
        };

        // Lookup and source retention only; the worker opens and reads content.
        // Missing, filtered or inaccessible winners never retry a lower source.
        let resolve_sound = |name: &str| {
            let resolve = || -> Result<Option<SoundAsset>, ResourceError> {
                let id = sound_resource_id(name)?;
                let Some(resource) = resources.get_resource(&id)? else {
                    return Ok(None);
                };
                Ok(Some(SoundAsset {
                    content: resource.content().clone().into_thread_safe()?,
                }))
            };
            match resolve() {
                Ok(asset) => asset,
                Err(error) => {
                    tracing::warn!(name, %error, "failed to resolve sound content");
                    None
                }
            }
        };

        for namespace in resources.namespaces()? {
            let id = ResourceId::new(&format!("{namespace}:sounds.json"))?;
            for resource in resources.get_resource_stack(&id)? {
                index.apply_content(&namespace, resource.content(), &resolve_sound);
            }
        }

        Ok(index)
    }

    /// Resolves an event to a concrete file using Vanilla's weighted selection
    /// semantics. Event redirects contribute the referenced event's total
    /// weight and consume another random draw when selected.
    pub fn choose(&self, event: &str, seed: Option<u64>) -> Option<SoundVariant> {
        let mut random = SoundRandom::new(seed);
        self.choose_inner(normalize_event_name(event), &mut random, &mut Vec::new())
    }

    /// A one-event index, so a test can dispatch a sound without installed
    /// resource packs or an audio worker.
    #[cfg(test)]
    pub fn for_test_event(event: &str) -> Self {
        let variant = SoundVariant {
            asset: tests::resolve_test_sound("test").unwrap(),
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

    fn apply_content(
        &mut self,
        namespace: &str,
        content: &ResourceContent,
        resolve_sound: &impl Fn(&str) -> Option<SoundAsset>,
    ) {
        let bytes = match content.read_bytes(MAX_SOUNDS_JSON_BYTES) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%error, "failed to read sounds.json layer");
                return;
            }
        };
        let json = match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(json) => json,
            Err(error) => {
                tracing::warn!(id = %content.id(), origin = ?content.origin(), %error, "failed to parse sounds.json layer");
                return;
            }
        };
        if let Err(error) = self.apply_value(namespace, &json, resolve_sound) {
            tracing::warn!(id = %content.id(), origin = ?content.origin(), %error, "invalid sounds.json layer");
        }
    }

    fn apply_value(
        &mut self,
        namespace: &str,
        json: &serde_json::Value,
        resolve_sound: &impl Fn(&str) -> Option<SoundAsset>,
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
        resolve_sound: &impl Fn(&str) -> Option<SoundAsset>,
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
        resolve_sound: &impl Fn(&str) -> Option<SoundAsset>,
    ) -> Result<Option<SoundEntry>, String> {
        match entry {
            serde_json::Value::String(name) => {
                validate_sound_name(name)?;
                let Some(asset) = resolve_sound(name) else {
                    return Ok(None);
                };
                Ok(Some(SoundEntry::File(SoundVariant {
                    asset,
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
                        let Some(asset) = resolve_sound(name) else {
                            return Ok(None);
                        };
                        Ok(Some(SoundEntry::File(SoundVariant {
                            asset,
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

/// Converts a file variant name into a logical OGG ID. Always append `.ogg`,
/// including when the input already has an extension, as the old resolver did.
///
/// `music/menu/menu1` -> `minecraft:sounds/music/menu/menu1.ogg`
/// `namespace:path`   -> `namespace:sounds/path.ogg`
fn sound_resource_id(name: &str) -> Result<ResourceId, ResourceError> {
    let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
    ResourceId::new(&format!("{ns}:sounds/{path}.ogg"))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::resources::packs::ResourceConfiguration;

    fn file(name: &str, weight: u32) -> SoundEntry {
        SoundEntry::File(SoundVariant {
            asset: resolve_test_sound(name).unwrap(),
            weight,
            volume: 1.0,
            pitch: 1.0,
            stream: false,
            attenuation_distance: DEFAULT_ATTENUATION_DISTANCE as f32,
        })
    }

    pub(super) fn resolve_test_sound(name: &str) -> Option<SoundAsset> {
        if name.starts_with("missing") {
            return None;
        }
        // These selection-only fixtures need real transferable source handles,
        // not decodable audio. The opened archive remains pinned after unlink.
        let id = sound_resource_id(name).unwrap();
        let path = std::env::temp_dir().join(format!(
            "pomme-sound-fixture-{}-{}.zip",
            std::process::id(),
            fastrand::u64(..)
        ));
        let file = std::fs::File::create_new(&path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file(
                format!("assets/{}/{}", id.namespace(), id.path()),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"sound-fixture").unwrap();
        drop(archive.finish().unwrap());
        let resources =
            ResourceManager::from_sources(ResourceConfiguration::jar_fixture_sources(&path))
                .unwrap();
        let content = resources
            .get_resource(&id)
            .unwrap()
            .unwrap()
            .content()
            .clone()
            .into_thread_safe()
            .unwrap();
        std::fs::remove_file(path).unwrap();
        Some(SoundAsset { content })
    }

    #[test]
    fn sound_asset_key_handles_default_and_explicit_namespaces() {
        assert_eq!(
            sound_resource_id("music/menu/menu1").unwrap().to_string(),
            "minecraft:sounds/music/menu/menu1.ogg"
        );
        assert_eq!(
            sound_resource_id("mod:foo/bar").unwrap().to_string(),
            "mod:sounds/foo/bar.ogg"
        );
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
            index.choose("test", Some(0)).unwrap().asset.content.id(),
            &sound_resource_id("present/file").unwrap()
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
        assert_eq!(
            resolved.asset.content.id(),
            &sound_resource_id("first").unwrap()
        );
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
            index.choose("parent", Some(0)).unwrap().asset.content.id(),
            &sound_resource_id("fallback").unwrap()
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
