//! Per-version known-pack tables: the data packs a client can claim in
//! `select_known_packs`, and the registry elements they carry.
//!
//! A server offers the packs it loaded; vanilla answers with the ones it has
//! itself (`KnownPacksManager.trySelectingPacks`), and the server then sends
//! those packs' registry entries as ids alone, with no NBT
//! (`RegistrySynchronization.packRegistry`). The client fills the gaps from
//! its own copy of the pack — the jar's `data/<ns>/<registry>/<id>.json`, read
//! through `NetworkRegistryLoadTask`. These tables are that copy, generated
//! from the extracted reference by `tools/protogen knownpacks`.
//!
//! Only versions with a table can claim anything; everything else answers with
//! an empty list and gets full registry data.

use std::collections::HashMap;
use std::sync::OnceLock;

/// `KnownPack.VANILLA_NAMESPACE`.
const VANILLA_NAMESPACE: &str = "minecraft";

/// Embedded tables by protocol number.
const TABLES: &[(i32, &str)] = &[(776, include_str!("data/known-packs-26.2.json"))];

/// One entry of `select_known_packs`, in wire field order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KnownPack {
    pub namespace: String,
    pub id: String,
    pub version: String,
}

struct Table {
    packs: Vec<KnownPack>,
    /// registry path (`worldgen/biome`) -> element path (`plains`) -> element.
    registries: HashMap<String, HashMap<String, serde_json::Value>>,
}

fn table(protocol: i32) -> Option<&'static Table> {
    static CELLS: OnceLock<HashMap<i32, Table>> = OnceLock::new();
    CELLS
        .get_or_init(|| {
            TABLES
                .iter()
                .map(|&(protocol, text)| {
                    let parsed: serde_json::Value = serde_json::from_str(text)
                        .unwrap_or_else(|e| panic!("known-pack table for {protocol}: {e}"));
                    let namespace = parsed["namespace"].as_str().unwrap_or(VANILLA_NAMESPACE);
                    let version = parsed["version"].as_str().unwrap_or_default();
                    let packs = parsed["packs"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|id| {
                            Some(KnownPack {
                                namespace: namespace.to_owned(),
                                id: id.as_str()?.to_owned(),
                                version: version.to_owned(),
                            })
                        })
                        .collect();
                    let registries = parsed["registries"]
                        .as_object()
                        .map(|registries| {
                            registries
                                .iter()
                                .map(|(registry, elements)| {
                                    let elements = elements
                                        .as_object()
                                        .map(|e| {
                                            e.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                                        })
                                        .unwrap_or_default();
                                    (registry.clone(), elements)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (protocol, Table { packs, registries })
                })
                .collect()
        })
        .get(&protocol)
}

/// The packs this version's client has, as `select_known_packs` entries.
/// Empty when nothing is embedded for it.
pub fn known_packs(protocol: i32) -> &'static [KnownPack] {
    table(protocol).map_or(&[], |t| t.packs.as_slice())
}

/// Vanilla `KnownPacksManager.trySelectingPacks`: of the packs the server
/// offered, the ones this client has, in the order the server sent them.
pub fn select_packs(protocol: i32, offered: &[KnownPack]) -> Vec<KnownPack> {
    let known = known_packs(protocol);
    offered
        .iter()
        .filter(|pack| known.contains(pack))
        .cloned()
        .collect()
}

/// The element a known pack carries for `<registry>/<id>`, as vanilla reads it
/// from `data/<namespace>/<registry path>/<id>.json`. Both arguments are
/// identifiers; a non-vanilla namespace has no embedded copy.
pub fn element(protocol: i32, registry: &str, id: &str) -> Option<&'static serde_json::Value> {
    let registry = vanilla_path(registry)?;
    let id = vanilla_path(id)?;
    table(protocol)?.registries.get(registry)?.get(id)
}

/// The path of an identifier in the vanilla namespace (`minecraft:` implied).
fn vanilla_path(identifier: &str) -> Option<&str> {
    match identifier.split_once(':') {
        Some((VANILLA_NAMESPACE, path)) => Some(path),
        Some(_) => None,
        None => Some(identifier),
    }
}

/// Every embedded registry with its elements, for cross-checks against a
/// consumer's deserializers (see pomme-client's `azalea_compat`).
pub fn registries(
    protocol: i32,
) -> Vec<(
    &'static str,
    Vec<(&'static str, &'static serde_json::Value)>,
)> {
    let Some(table) = table(protocol) else {
        return Vec::new();
    };
    table
        .registries
        .iter()
        .map(|(registry, elements)| {
            (
                registry.as_str(),
                elements.iter().map(|(id, e)| (id.as_str(), e)).collect(),
            )
        })
        .collect()
}

/// Whether this version can claim anything at all.
pub fn has_table(protocol: i32) -> bool {
    table(protocol).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::NATIVE;

    fn pack(id: &str, version: &str) -> KnownPack {
        KnownPack {
            namespace: VANILLA_NAMESPACE.to_owned(),
            id: id.to_owned(),
            version: version.to_owned(),
        }
    }

    #[test]
    fn native_claims_core_at_its_own_version() {
        let packs = known_packs(NATIVE.protocol);
        assert_eq!(packs.first(), Some(&pack("core", NATIVE.name)));
        // The feature packs under `data/minecraft/datapacks/`, which the
        // trusted vanilla repository also knows.
        for id in [
            "minecart_improvements",
            "redstone_experiments",
            "trade_rebalance",
        ] {
            assert!(packs.contains(&pack(id, NATIVE.name)), "missing {id}");
        }
    }

    #[test]
    fn selection_keeps_the_servers_order_and_drops_the_unknown() {
        let offered = vec![
            pack("trade_rebalance", NATIVE.name),
            pack("mystery_pack", NATIVE.name),
            pack("core", NATIVE.name),
            // Same pack, another game version: not ours.
            pack("core", "1.21.4"),
        ];
        assert_eq!(
            select_packs(NATIVE.protocol, &offered),
            vec![
                pack("trade_rebalance", NATIVE.name),
                pack("core", NATIVE.name)
            ],
        );
    }

    #[test]
    fn versions_without_a_table_claim_nothing() {
        let offered = vec![pack("core", "1.21.4")];
        assert!(!has_table(769));
        assert!(known_packs(769).is_empty());
        assert!(select_packs(769, &offered).is_empty());
    }

    #[test]
    fn overworld_dimension_type_matches_vanilla() {
        let overworld = element(
            NATIVE.protocol,
            "minecraft:dimension_type",
            "minecraft:overworld",
        )
        .expect("overworld dimension type");
        assert_eq!(overworld["height"], 384);
        assert_eq!(overworld["min_y"], -64);
        assert_eq!(overworld["has_skylight"], true);
        assert_eq!(
            element(NATIVE.protocol, "dimension_type", "the_nether").map(|d| &d["cardinal_light"]),
            Some(&serde_json::Value::from("nether")),
        );
    }

    #[test]
    fn biome_elements_keep_what_the_network_codec_reads() {
        let plains = element(
            NATIVE.protocol,
            "minecraft:worldgen/biome",
            "minecraft:plains",
        )
        .expect("plains biome");
        assert_eq!(plains["downfall"], 0.4);
        assert_eq!(plains["effects"]["water_color"], "#3f76e4");
        // Worldgen and spawner data belong to `Biome.DIRECT_CODEC`.
        assert!(plains.get("carvers").is_none());
        assert!(plains.get("features").is_none());
        assert!(plains.get("spawners").is_none());
    }

    #[test]
    fn every_synchronized_registry_is_present() {
        // `RegistryDataLoader.SYNCHRONIZED_REGISTRIES` for 26.2.
        let expected: [(&str, usize); 29] = [
            ("worldgen/biome", 66),
            ("chat_type", 7),
            ("trim_pattern", 18),
            ("trim_material", 11),
            ("wolf_variant", 9),
            ("wolf_sound_variant", 7),
            ("pig_variant", 3),
            ("pig_sound_variant", 3),
            ("frog_variant", 3),
            ("cat_variant", 11),
            ("cat_sound_variant", 2),
            ("cow_sound_variant", 2),
            ("cow_variant", 3),
            ("chicken_sound_variant", 2),
            ("chicken_variant", 3),
            ("zombie_nautilus_variant", 2),
            ("painting_variant", 51),
            ("sulfur_cube_archetype", 12),
            ("dimension_type", 4),
            ("damage_type", 51),
            ("banner_pattern", 43),
            ("enchantment", 43),
            ("jukebox_song", 22),
            ("instrument", 8),
            ("test_environment", 1),
            ("test_instance", 1),
            ("dialog", 3),
            ("world_clock", 2),
            ("timeline", 4),
        ];
        let registries = &table(NATIVE.protocol).expect("native table").registries;
        assert_eq!(registries.len(), expected.len());
        for (registry, count) in expected {
            assert_eq!(
                registries.get(registry).map(HashMap::len),
                Some(count),
                "{registry}",
            );
        }
    }

    #[test]
    fn other_namespaces_have_no_embedded_copy() {
        assert!(element(NATIVE.protocol, "pomme:dimension_type", "overworld").is_none());
        assert!(element(NATIVE.protocol, "dimension_type", "pomme:overworld").is_none());
    }
}
