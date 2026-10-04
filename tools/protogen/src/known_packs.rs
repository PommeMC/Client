//! Generates a version's known-pack table (see pomme-protocol's
//! `known_packs`): the packs the client can claim in `select_known_packs`,
//! plus the element data of every synchronized registry they carry, read from
//! the extracted `data/<ns>/<registry>/<id>.json` files.

use std::path::Path;

use crate::{Error, resolve_protocol_number};

/// Vanilla `BuiltInPackSource.CORE_PACK_INFO` plus the feature packs under
/// `data/minecraft/datapacks/`, all `KnownPack.vanilla(id)` (namespace
/// `minecraft`, version `SharedConstants.getCurrentVersion().id()`).
const CORE_PACK: &str = "core";
const VANILLA_NAMESPACE: &str = "minecraft";

/// The registries `RegistryDataLoader.SYNCHRONIZED_REGISTRIES` lists, by the
/// directory they load from (`registry path`, so `worldgen/biome` nests).
/// Order follows the vanilla list.
const SYNCHRONIZED_REGISTRIES: [&str; 29] = [
    "worldgen/biome",
    "chat_type",
    "trim_pattern",
    "trim_material",
    "wolf_variant",
    "wolf_sound_variant",
    "pig_variant",
    "pig_sound_variant",
    "frog_variant",
    "cat_variant",
    "cat_sound_variant",
    "cow_sound_variant",
    "cow_variant",
    "chicken_sound_variant",
    "chicken_variant",
    "zombie_nautilus_variant",
    "painting_variant",
    "sulfur_cube_archetype",
    "dimension_type",
    "damage_type",
    "banner_pattern",
    "enchantment",
    "jukebox_song",
    "instrument",
    "test_environment",
    "test_instance",
    "dialog",
    "world_clock",
    "timeline",
];

/// `Biome.NETWORK_CODEC` reads only the climate settings, the syncable
/// attributes and the special effects; the worldgen and spawner halves of the
/// file belong to `DIRECT_CODEC` and never reach a client. Dropping them takes
/// the biome data from ~240KB to ~20KB. The attributes stay unfiltered, as
/// pomme reads none of them.
const BIOME_NETWORK_FIELDS: [&str; 6] = [
    "has_precipitation",
    "temperature",
    "temperature_modifier",
    "downfall",
    "attributes",
    "effects",
];

pub fn generate(root: &Path, version: &str, out_path: &str) -> Result<(), Error> {
    let protocol = resolve_protocol_number(&root.join("decompiled"), None)?;
    let data_root = root.join("extracted/data").join(VANILLA_NAMESPACE);
    if !data_root.is_dir() {
        return Err(format!("{data_root:?}: no extracted data directory").into());
    }

    let mut packs = vec![serde_json::Value::from(CORE_PACK)];
    let datapacks = data_root.join("datapacks");
    if datapacks.is_dir() {
        let mut feature_packs: Vec<String> = std::fs::read_dir(&datapacks)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        feature_packs.sort();
        // A claimed pack's synchronized elements arrive without data, but
        // only core's are embedded, so a feature pack carrying any would be
        // filled wrong or not at all.
        for pack in &feature_packs {
            let pack_data = datapacks.join(pack).join("data");
            if !pack_data.is_dir() {
                continue;
            }
            for namespace in std::fs::read_dir(&pack_data)? {
                let namespace = namespace?;
                for registry in SYNCHRONIZED_REGISTRIES {
                    if namespace.path().join(registry).is_dir() {
                        return Err(format!(
                            "{pack} carries {}:{registry} elements; claiming it needs per-pack tables",
                            namespace.file_name().to_string_lossy()
                        )
                        .into());
                    }
                }
            }
        }
        packs.extend(feature_packs.into_iter().map(serde_json::Value::from));
    }

    let mut registries = serde_json::Map::new();
    for registry in SYNCHRONIZED_REGISTRIES {
        let dir = data_root.join(registry);
        if !dir.is_dir() {
            eprintln!("warning: {registry} absent from this version's data, skipping");
            continue;
        }
        let mut elements = serde_json::Map::new();
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("{path:?}: unreadable file name"))?
                .to_owned();
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{path:?}: {e}"))?;
            let mut element: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("{path:?}: {e}"))?;
            if registry == "worldgen/biome"
                && let Some(fields) = element.as_object_mut()
            {
                fields.retain(|key, _| BIOME_NETWORK_FIELDS.contains(&key.as_str()));
            }
            elements.insert(id, element);
        }
        if elements.is_empty() {
            return Err(format!("{registry}: no elements in {dir:?}").into());
        }
        println!("{registry}: {} elements", elements.len());
        registries.insert(registry.to_string(), serde_json::Value::Object(elements));
    }

    let file = serde_json::json!({
        "version": version,
        "protocol": protocol,
        "namespace": VANILLA_NAMESPACE,
        "packs": packs,
        "registries": registries,
    });
    let mut text = serde_json::to_string(&file)?;
    text.push('\n');
    std::fs::write(out_path, text)?;
    println!("wrote {out_path}");
    Ok(())
}
