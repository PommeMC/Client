//! Generated `EntityDataSerializers` names and cross-version numeric remaps.

use std::sync::OnceLock;

use crate::version::{EMBEDDED, NATIVE};

/// One wire version's serializer registration order and native destinations.
pub struct SerializerTable {
    names: Vec<String>,
    to_native: Box<[Option<u32>]>,
}

#[derive(serde::Deserialize)]
struct TableFile {
    version: String,
    protocol: i32,
    serializers: Vec<String>,
}

/// Canonicalizes the wire-identical reference serializer renamed in 1.21.5.
fn canonical_name(name: &str) -> &str {
    match name {
        "optional_uuid" => "optional_living_entity_reference",
        other => other,
    }
}

impl SerializerTable {
    /// Returns the generated table for an embedded protocol, cached once.
    pub fn for_protocol(protocol: i32) -> Option<&'static Self> {
        static TABLES: [OnceLock<SerializerTable>; EMBEDDED.len()] =
            [const { OnceLock::new() }; EMBEDDED.len()];
        crate::version::embedded_get(protocol, &TABLES, |entry| {
            let wire = Self::parse(entry.entity_serializers, entry.version);
            let native = EMBEDDED
                .iter()
                .find(|e| e.version == NATIVE)
                .expect("native embedded version");
            let native = Self::parse(native.entity_serializers, NATIVE);
            let to_native = wire
                .iter()
                .map(|name| {
                    native
                        .iter()
                        .position(|n| canonical_name(n) == canonical_name(name))
                        .map(|id| id as u32)
                })
                .collect();
            Self {
                names: wire,
                to_native,
            }
        })
    }

    /// Parses trusted generated data, failing immediately on stale provenance.
    fn parse(json: &str, expected: crate::version::ProtocolVersion) -> Vec<String> {
        let file: TableFile = serde_json::from_str(json).expect("generated serializer table");
        assert_eq!(file.version, expected.name, "serializer version");
        assert_eq!(file.protocol, expected.protocol, "serializer protocol");
        assert!(!file.serializers.is_empty(), "empty serializer table");
        for (id, name) in file.serializers.iter().enumerate() {
            assert!(
                !file.serializers[..id].contains(name),
                "duplicate serializer {name}"
            );
        }
        file.serializers
    }

    /// Maps a wire id to its native id; removed or unknown serializers have
    /// none.
    pub fn to_native(&self, id: u32) -> Option<u32> {
        self.to_native.get(id as usize).copied().flatten()
    }

    /// Returns registration names in numeric-id order.
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// Preserves the complete legacy mappings captured after arithmetic/name
    /// parity passed.
    #[test]
    fn generated_maps_match_verified_legacy() {
        let expected: BTreeMap<i32, Vec<Option<u32>>> =
            serde_json::from_str(include_str!("data/entity-serializer-remaps.json")).unwrap();
        assert_eq!(expected.len(), EMBEDDED.len());
        for entry in EMBEDDED {
            let protocol = entry.version.protocol;
            let table = SerializerTable::for_protocol(protocol).unwrap();
            let ids = &expected[&protocol];
            assert_eq!(ids.len(), 64);
            for (id, &native) in ids.iter().enumerate() {
                assert_eq!(
                    table.to_native(id as u32),
                    native,
                    "protocol {protocol}, serializer {id}"
                );
            }
            assert_eq!(table.to_native(u32::MAX), None);
        }
        assert_eq!(
            SerializerTable::for_protocol(NATIVE.protocol)
                .unwrap()
                .names()
                .len(),
            crate::entity_data::SERIALIZERS_26_2.len(),
        );
    }
}
