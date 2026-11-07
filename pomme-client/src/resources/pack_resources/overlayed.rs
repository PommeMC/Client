//! One logical pack with Vanilla's last-overlay-first resource precedence.

use std::collections::{HashMap, HashSet};
use std::io;
use std::rc::Rc;

use serde_json::Value;

use super::{IoSupplier, PackMetadataResources, PackResources, PackType, ResourceOutput};
use crate::resources::packs::PackLocationInfo;
use crate::resources::resource::{ReadStream, ResourceId};

pub(crate) struct OverlayedPackResources {
    primary: Rc<dyn PackResources>,
    overlays: Vec<Rc<dyn PackResources>>,
}

impl OverlayedPackResources {
    /// Overlays arrive in metadata order. Reuse their storage in priority
    /// order; selection and applicability are the caller's responsibility.
    pub(crate) fn new(
        primary: Rc<dyn PackResources>,
        mut overlays: Vec<Rc<dyn PackResources>>,
    ) -> Self {
        overlays.reverse();
        Self { primary, overlays }
    }

    fn stack(&self) -> impl Iterator<Item = &Rc<dyn PackResources>> {
        self.overlays.iter().chain(std::iter::once(&self.primary))
    }
}

impl PackMetadataResources for OverlayedPackResources {
    fn location(&self) -> &PackLocationInfo {
        self.primary.location()
    }

    // Root files and pack metadata belong only to the primary, not the stack.
    fn get_root_resource(&self, path: &[&str]) -> io::Result<Option<IoSupplier<ReadStream>>> {
        self.primary.get_root_resource(path)
    }

    fn metadata_section_json(&self, name: &str) -> io::Result<Option<Value>> {
        self.primary.metadata_section_json(name)
    }

    /// Like Vanilla's forEach, visit in priority order and stop on failure.
    /// Repeated close and retained supplier behavior belong to each child.
    fn close(&self) -> io::Result<()> {
        for pack in self.stack() {
            pack.close()?;
        }
        Ok(())
    }
}

impl PackResources for OverlayedPackResources {
    fn get_resource(
        &self,
        pack_type: PackType,
        id: &ResourceId,
    ) -> io::Result<Option<IoSupplier<ReadStream>>> {
        for pack in self.stack() {
            if let Some(supplier) = pack.get_resource(pack_type, id)? {
                return Ok(Some(supplier));
            }
        }
        Ok(None)
    }

    fn list_resources(
        &self,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        output: &mut ResourceOutput<'_>,
    ) -> io::Result<()> {
        let mut resources = HashMap::new();
        for pack in self.stack() {
            pack.list_resources(pack_type, namespace, directory, &mut |id, supplier| {
                resources.entry(id).or_insert(supplier);
            })?;
        }
        // Delay output until all layers succeed, retaining the first supplier.
        for (id, supplier) in resources {
            output(id, supplier);
        }
        Ok(())
    }

    fn get_namespaces(&self, pack_type: PackType) -> io::Result<HashSet<String>> {
        let mut namespaces = HashSet::new();
        for pack in self.stack() {
            namespaces.extend(pack.get_namespaces(pack_type)?);
        }
        Ok(namespaces)
    }
}

#[cfg(test)]
mod test_fixture {
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::{fs, io};

    use super::OverlayedPackResources;
    use crate::chat_component::Component;
    use crate::resources::pack_resources::PathPackResources;
    use crate::resources::packs::{PackLocationInfo, PackSource};

    pub(super) struct TempDir(pub(super) PathBuf);

    impl TempDir {
        fn new() -> io::Result<Self> {
            for attempt in 0.. {
                let path = std::env::temp_dir().join(format!(
                    "pomme-overlayed-api-{}-{attempt}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Ok(Self(path)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            unreachable!()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.0) {
                tracing::warn!(path = %self.0.display(), %error, "Failed to remove test temporary directory");
            }
        }
    }

    pub(super) fn default_location() -> PackLocationInfo {
        PackLocationInfo {
            id: "overlayed-api".into(),
            title: Component::text("Overlayed API fixture"),
            source: PackSource::built_in(),
            known_pack_info: None,
        }
    }

    pub(super) fn fixture(location: PackLocationInfo) -> (TempDir, OverlayedPackResources) {
        let temp = TempDir::new().unwrap();
        let primary_root = temp.0.join("primary");
        let overlay_root = temp.0.join("overlay");
        fs::create_dir(&primary_root).unwrap();
        fs::create_dir_all(primary_root.join("assets/api_test/test/nested")).unwrap();
        fs::create_dir_all(primary_root.join("data/api_data/test")).unwrap();
        fs::write(
            primary_root.join("assets/api_test/test/one.txt"),
            b"client one",
        )
        .unwrap();
        fs::write(
            primary_root.join("assets/api_test/test/nested/two.txt"),
            b"client two",
        )
        .unwrap();
        fs::write(
            primary_root.join("data/api_data/test/server.txt"),
            b"server data",
        )
        .unwrap();
        fs::create_dir(&overlay_root).unwrap();
        fs::write(primary_root.join("root.txt"), b"primary root").unwrap();
        fs::write(overlay_root.join("root.txt"), b"overlay root").unwrap();
        fs::write(
            primary_root.join("pack.mcmeta"),
            br#"{"test":{"value":"primary"},"other":{"value":"other"}}"#,
        )
        .unwrap();
        fs::write(
            overlay_root.join("pack.mcmeta"),
            br#"{"test":{"value":"overlay"}}"#,
        )
        .unwrap();
        let overlay_location = PackLocationInfo {
            id: "overlay-location".into(),
            title: Component::translate("location.overlay.title", Vec::new()),
            source: PackSource::built_in(),
            known_pack_info: None,
        };
        let pack = OverlayedPackResources::new(
            Rc::new(PathPackResources::new(location, primary_root)),
            vec![Rc::new(PathPackResources::new(
                overlay_location,
                overlay_root,
            ))],
        );
        (temp, pack)
    }
}

#[cfg(test)]
mod pack_metadata_resources_tests {
    use pomme_protocol::KnownPack;
    use serde_json::json;

    use super::test_fixture::{default_location, fixture};
    use crate::chat_component::Component;
    use crate::resources::pack_resources::tests;
    use crate::resources::packs::{PackLocationInfo, PackSource};

    #[test]
    fn test_get_root_resource() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_root_resource(&pack, &["root.txt"], b"primary root");
    }

    #[test]
    fn test_get_root_resource_missing() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_root_resource_missing(&pack, &["missing.txt"]);
    }

    #[test]
    fn test_metadata_section_json() {
        let (_temp, pack) = fixture(default_location());
        tests::test_metadata_section_json(&pack, "test", Some(json!({"value": "primary"})));
    }

    #[test]
    fn test_metadata_section_missing() {
        let (_temp, pack) = fixture(default_location());
        tests::test_metadata_section_json(&pack, "missing", None);
    }

    #[test]
    fn test_close() {
        let (_temp, pack) = fixture(default_location());
        tests::test_close(&pack);
    }

    #[test]
    fn test_location() {
        let expected = PackLocationInfo {
            id: "overlayed-primary-location".into(),
            title: Component::translate("location.overlayed.primary.title", Vec::new()),
            source: PackSource::feature(),
            known_pack_info: Some(KnownPack {
                namespace: "location_test".into(),
                id: "overlayed_primary_known".into(),
                version: "test-1".into(),
            }),
        };
        let (_temp, pack) = fixture(expected.clone());
        tests::test_location(&pack, &expected);
    }
}

#[cfg(test)]
mod pack_metadata_resources_ext_tests {
    use std::{fs, io};

    use serde_json::json;

    use super::test_fixture::{default_location, fixture};
    use crate::resources::pack_resources::{MetadataSectionType, tests};

    #[test]
    fn test_get_metadata_section() {
        let (_temp, pack) = fixture(default_location());
        let section = MetadataSectionType::<String> {
            name: "test".into(),
            decode: |value| {
                value
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "expected string"))
            },
            encode: |value| Ok(json!({"value": value})),
        };
        tests::test_get_metadata_section(&pack, &section, Some("primary".into()));
    }

    #[test]
    fn test_get_metadata_section_missing() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_metadata_section_missing(&pack, "missing");
    }

    #[test]
    fn test_get_metadata_section_null() {
        let (temp, pack) = fixture(default_location());
        fs::write(temp.0.join("primary/pack.mcmeta"), br#"{"null":null}"#).unwrap();
        tests::test_get_metadata_section_null(&pack, "null");
    }

    #[test]
    fn test_get_metadata_section_decode_error() {
        let (_temp, pack) = fixture(default_location());
        let section = MetadataSectionType::<String> {
            name: "test".into(),
            decode: |_| {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "decoder sentinel",
                ))
            },
            encode: |value| Ok(json!({"value": value})),
        };
        tests::test_get_metadata_section_decode_error(&pack, &section, io::ErrorKind::Unsupported);
    }
}

#[cfg(test)]
mod pack_resources_tests {
    use std::collections::HashSet;

    use pomme_protocol::KnownPack;

    use super::test_fixture::{default_location, fixture};
    use crate::resources::pack_resources::{PackType, tests};
    use crate::resources::packs::PackLocationInfo;
    use crate::resources::resource_id;

    #[test]
    fn test_get_resource_client() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_resource(
            &pack,
            PackType::ClientResources,
            &resource_id!("api_test:test/one.txt"),
            b"client one",
        );
    }

    #[test]
    fn test_get_resource_server() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_resource(
            &pack,
            PackType::ServerData,
            &resource_id!("api_data:test/server.txt"),
            b"server data",
        );
    }

    #[test]
    fn test_get_resource_missing() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_resource_missing(
            &pack,
            PackType::ClientResources,
            &resource_id!("api_test:test/missing.txt"),
        );
    }

    #[test]
    fn test_get_resource_wrong_domain() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_resource_missing(
            &pack,
            PackType::ServerData,
            &resource_id!("api_test:test/one.txt"),
        );
    }

    #[test]
    fn test_list_resources_client() {
        let (_temp, pack) = fixture(default_location());
        tests::test_list_resources(
            &pack,
            PackType::ClientResources,
            "api_test",
            "test",
            &[
                (
                    resource_id!("api_test:test/one.txt"),
                    b"client one".to_vec(),
                ),
                (
                    resource_id!("api_test:test/nested/two.txt"),
                    b"client two".to_vec(),
                ),
            ],
        );
    }

    #[test]
    fn test_list_resources_server() {
        let (_temp, pack) = fixture(default_location());
        tests::test_list_resources(
            &pack,
            PackType::ServerData,
            "api_data",
            "test",
            &[(
                resource_id!("api_data:test/server.txt"),
                b"server data".to_vec(),
            )],
        );
    }

    #[test]
    fn test_list_resources_empty_directory() {
        let (_temp, pack) = fixture(default_location());
        tests::test_list_resources(
            &pack,
            PackType::ClientResources,
            "api_test",
            "",
            &[
                (
                    resource_id!("api_test:test/one.txt"),
                    b"client one".to_vec(),
                ),
                (
                    resource_id!("api_test:test/nested/two.txt"),
                    b"client two".to_vec(),
                ),
            ],
        );
    }

    #[test]
    fn test_list_resources_missing_directory() {
        let (_temp, pack) = fixture(default_location());
        tests::test_list_resources(&pack, PackType::ClientResources, "api_test", "missing", &[]);
    }

    #[test]
    fn test_list_resources_missing_namespace() {
        let (_temp, pack) = fixture(default_location());
        tests::test_list_resources(&pack, PackType::ClientResources, "missing", "test", &[]);
    }

    #[test]
    fn test_list_resources_wrong_domain() {
        let (_temp, pack) = fixture(default_location());
        tests::test_list_resources(&pack, PackType::ServerData, "api_test", "test", &[]);
    }

    #[test]
    fn test_get_namespaces_client() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_namespaces(
            &pack,
            PackType::ClientResources,
            &HashSet::from(["api_test".into()]),
        );
    }

    #[test]
    fn test_get_namespaces_server() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_namespaces(
            &pack,
            PackType::ServerData,
            &HashSet::from(["api_data".into()]),
        );
    }

    #[test]
    fn test_pack_id() {
        let (_temp, pack) = fixture(default_location());
        tests::test_pack_id(&pack, "overlayed-api");
    }

    #[test]
    fn test_known_pack_info_none() {
        let (_temp, pack) = fixture(default_location());
        tests::test_known_pack_info(&pack, None);
    }

    #[test]
    fn test_known_pack_info_some() {
        let expected = KnownPack {
            namespace: "api_identity".into(),
            id: "overlayed_known".into(),
            version: "api-2".into(),
        };
        let location = PackLocationInfo {
            known_pack_info: Some(expected.clone()),
            ..default_location()
        };
        let (_temp, pack) = fixture(location);
        tests::test_known_pack_info(&pack, Some(&expected));
    }
}
