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
//! Only versions with a table can claim anything; the rest answer with an
//! empty list and get full registry data.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::version::{EMBEDDED, NATIVE, ProtocolVersion};

/// One entry of `select_known_packs`, in wire field order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KnownPack {
    pub namespace: String,
    pub id: String,
    pub version: String,
}

/// One version's known packs and the elements they carry.
pub struct KnownPackTable {
    packs: Vec<KnownPack>,
    /// registry path (`worldgen/biome`) -> element path (`plains`) -> element.
    registries: HashMap<String, HashMap<String, serde_json::Value>>,
}

#[derive(serde::Deserialize)]
struct TableFile {
    version: String,
    protocol: i32,
    namespace: String,
    packs: Vec<String>,
    registries: HashMap<String, HashMap<String, serde_json::Value>>,
}

impl KnownPackTable {
    /// The table for the version the client speaks natively.
    pub fn native() -> &'static KnownPackTable {
        Self::for_protocol(NATIVE.protocol).expect("native known-pack table is embedded")
    }

    /// The table for a protocol number, or `None` for a version with nothing
    /// embedded — those claim no packs.
    pub fn for_protocol(protocol: i32) -> Option<&'static KnownPackTable> {
        static TABLES: [OnceLock<Option<KnownPackTable>>; EMBEDDED.len()] =
            [const { OnceLock::new() }; EMBEDDED.len()];
        crate::version::embedded_get(protocol, &TABLES, |e| {
            e.known_packs.map(|json| {
                Self::parse(json, e.version).unwrap_or_else(|err| {
                    panic!("embedded {} known-pack table: {err}", e.version.name)
                })
            })
        })?
        .as_ref()
    }

    fn parse(json: &str, expected: ProtocolVersion) -> Result<Self, String> {
        let file: TableFile = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if file.version != expected.name || file.protocol != expected.protocol {
            return Err(format!(
                "table is {}/{}, expected {}/{}",
                file.version, file.protocol, expected.name, expected.protocol
            ));
        }
        if let Some((registry, _)) = file.registries.iter().find(|(_, e)| e.is_empty()) {
            return Err(format!("empty registry {registry}"));
        }
        Ok(Self {
            packs: file
                .packs
                .into_iter()
                .map(|id| KnownPack {
                    namespace: file.namespace.clone(),
                    id,
                    version: file.version.clone(),
                })
                .collect(),
            registries: file.registries,
        })
    }

    /// The packs this version's client has.
    pub fn packs(&self) -> &[KnownPack] {
        &self.packs
    }

    /// Whether an offered pack is one of them, as
    /// `KnownPacksManager.trySelectingPacks` looks its request up.
    pub fn knows(&self, namespace: &str, id: &str, version: &str) -> bool {
        self.packs
            .iter()
            .any(|p| p.namespace == namespace && p.id == id && p.version == version)
    }

    /// The element a known pack carries for `<registry>/<id>`, as vanilla reads
    /// it from `data/<namespace>/<registry path>/<id>.json`. Both arguments are
    /// identifiers; a non-vanilla namespace has no embedded copy.
    pub fn element(&self, registry: &str, id: &str) -> Option<&serde_json::Value> {
        self.registries
            .get(vanilla_path(registry)?)?
            .get(vanilla_path(id)?)
    }

    /// Every registry with its elements, for cross-checks against a consumer's
    /// deserializers (see pomme-client's `azalea_compat`).
    pub fn registries(
        &self,
    ) -> impl Iterator<Item = (&str, impl Iterator<Item = (&str, &serde_json::Value)>)> {
        self.registries.iter().map(|(registry, elements)| {
            (
                registry.as_str(),
                elements.iter().map(|(id, e)| (id.as_str(), e)),
            )
        })
    }
}

/// The path of an identifier in the vanilla namespace (`minecraft:` implied).
fn vanilla_path(identifier: &str) -> Option<&str> {
    match identifier.split_once(':') {
        Some(("minecraft", path)) => Some(path),
        Some(_) => None,
        None => Some(identifier),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_claims_core_and_the_feature_packs() {
        let table = KnownPackTable::native();
        assert_eq!(
            table.packs().first(),
            Some(&KnownPack {
                namespace: "minecraft".to_owned(),
                id: "core".to_owned(),
                version: NATIVE.name.to_owned(),
            })
        );
        // The packs under `data/minecraft/datapacks/`, which vanilla's trusted
        // repository also knows.
        for id in [
            "minecart_improvements",
            "redstone_experiments",
            "trade_rebalance",
        ] {
            assert!(table.knows("minecraft", id, NATIVE.name), "missing {id}");
        }
        // Another game version's core is a different pack.
        assert!(!table.knows("minecraft", "core", "1.21.4"));
        assert!(!table.knows("pomme", "core", NATIVE.name));
        assert!(!table.knows("minecraft", "mystery_pack", NATIVE.name));
    }

    #[test]
    fn versions_without_a_table_claim_nothing() {
        assert!(KnownPackTable::for_protocol(769).is_none());
    }

    #[test]
    fn overworld_dimension_type_matches_vanilla() {
        let table = KnownPackTable::native();
        let overworld = table
            .element("minecraft:dimension_type", "minecraft:overworld")
            .expect("overworld dimension type");
        assert_eq!(overworld["height"], 384);
        assert_eq!(overworld["min_y"], -64);
        assert_eq!(overworld["has_skylight"], true);
        assert_eq!(
            table
                .element("dimension_type", "the_nether")
                .map(|d| &d["cardinal_light"]),
            Some(&serde_json::Value::from("nether")),
        );
    }

    #[test]
    fn biome_elements_keep_what_the_network_codec_reads() {
        let plains = KnownPackTable::native()
            .element("minecraft:worldgen/biome", "minecraft:plains")
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
        let table = KnownPackTable::native();
        assert_eq!(table.registries().count(), expected.len());
        for (registry, count) in expected {
            let elements = table
                .registries()
                .find(|(r, _)| *r == registry)
                .map(|(_, e)| e.count());
            assert_eq!(elements, Some(count), "{registry}");
        }
    }

    #[test]
    fn other_namespaces_have_no_embedded_copy() {
        let table = KnownPackTable::native();
        assert!(table.element("pomme:dimension_type", "overworld").is_none());
        assert!(table.element("dimension_type", "pomme:overworld").is_none());
    }
}
