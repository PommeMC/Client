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

/// Vanilla's layers catch their own errors, so one that fails is logged and
/// counts as empty; the rest of the stack still answers.
fn logged<T>(pack: &dyn PackResources, operation: &str, result: io::Result<T>) -> Option<T> {
    result
        .inspect_err(|error| {
            tracing::warn!(pack = %pack.location().id, operation, %error, "Pack layer failed");
        })
        .ok()
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
        Ok(self.stack().find_map(|pack| {
            logged(
                pack.as_ref(),
                "get resource",
                pack.get_resource(pack_type, id),
            )
            .flatten()
        }))
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
            let result =
                pack.list_resources(pack_type, namespace, directory, &mut |id, supplier| {
                    resources.entry(id).or_insert(supplier);
                });
            logged(pack.as_ref(), "list resources", result);
        }
        for (id, supplier) in resources {
            output(id, supplier);
        }
        Ok(())
    }

    fn get_namespaces(&self, pack_type: PackType) -> io::Result<HashSet<String>> {
        Ok(self
            .stack()
            .filter_map(|pack| {
                logged(
                    pack.as_ref(),
                    "list namespaces",
                    pack.get_namespaces(pack_type),
                )
            })
            .flatten()
            .collect())
    }
}

#[cfg(test)]
mod test_fixture {

    use std::fs;
    use std::rc::Rc;

    use super::OverlayedPackResources;
    use crate::chat_component::Component;
    use crate::resources::pack_resources::PathPackResources;
    use crate::resources::packs::PackLocationInfo;
    use crate::resources::test_dir::TempDir;

    pub(super) fn default_location() -> PackLocationInfo {
        PackLocationInfo::test("overlayed-api")
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
    use serde_json::json;

    use super::test_fixture::{default_location, fixture};
    use crate::chat_component::Component;
    use crate::resources::pack_resources::tests;
    use crate::resources::packs::PackLocationInfo;

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
            known_pack_info: Some(pomme_protocol::KnownPack {
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
mod pack_resources_tests {
    use std::collections::HashSet;

    use super::test_fixture::{default_location, fixture};
    use crate::resources::pack_resources::{PackType, tests};
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
    fn test_get_resource_missing() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_resource_missing(
            &pack,
            PackType::ClientResources,
            &resource_id!("api_test:test/missing.txt"),
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
    fn test_list_resources_empty_directory() {
        let (_temp, pack) = fixture(default_location());
        tests::test_list_resources(&pack, PackType::ClientResources, "api_test", "", &[]);
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
    fn test_get_namespaces_client() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_namespaces(
            &pack,
            PackType::ClientResources,
            &HashSet::from(["api_test".into()]),
        );
    }

    #[test]
    fn test_pack_id() {
        let (_temp, pack) = fixture(default_location());
        tests::test_pack_id(&pack, "overlayed-api");
    }
}

#[cfg(test)]
mod failing_layer_tests {
    use std::rc::Rc;

    use super::OverlayedPackResources;
    use super::test_fixture::{default_location, fixture};
    use crate::resources::pack_resources::test_pack::TestPack;
    use crate::resources::pack_resources::{PackResources, PackType};
    use crate::resources::resource_id;

    #[test]
    fn a_failing_overlay_leaves_the_primary_answering() {
        let (_temp, overlayed) = fixture(default_location());
        let pack = OverlayedPackResources::new(
            Rc::new(overlayed),
            vec![Rc::new(TestPack::new("broken", &[]).broken())],
        );
        let id = resource_id!("api_test:test/one.txt");
        assert!(
            pack.get_resource(PackType::ClientResources, &id)
                .unwrap()
                .is_some()
        );
        let mut listed = Vec::new();
        pack.list_resources(
            PackType::ClientResources,
            "api_test",
            "test",
            &mut |id, _| listed.push(id),
        )
        .unwrap();
        assert!(listed.contains(&id));
        assert!(
            pack.get_namespaces(PackType::ClientResources)
                .unwrap()
                .contains("api_test")
        );
    }
}

#[cfg(test)]
mod priority_tests {
    use std::rc::Rc;

    use super::OverlayedPackResources;
    use crate::resources::pack_resources::test_pack::TestPack;
    use crate::resources::pack_resources::{PackType, tests};
    use crate::resources::resource_id;

    /// Vanilla `OverlayedPackResources`: the last overlay wins, then earlier
    /// ones, then the primary, in lookups and listings alike.
    #[test]
    fn the_last_overlay_wins() {
        let pack = OverlayedPackResources::new(
            Rc::new(TestPack::new(
                "primary",
                &[("ns/dir/a.txt", "primary"), ("ns/dir/b.txt", "primary")],
            )),
            vec![
                Rc::new(TestPack::new(
                    "first",
                    &[("ns/dir/a.txt", "first"), ("ns/dir/b.txt", "first")],
                )),
                Rc::new(TestPack::new("second", &[("ns/dir/a.txt", "second")])),
            ],
        );
        let (a, b) = (resource_id!("ns:dir/a.txt"), resource_id!("ns:dir/b.txt"));
        tests::test_get_resource(&pack, PackType::ClientResources, &a, b"second");
        tests::test_get_resource(&pack, PackType::ClientResources, &b, b"first");
        tests::test_list_resources(
            &pack,
            PackType::ClientResources,
            "ns",
            "dir",
            &[(a, b"second".to_vec()), (b, b"first".to_vec())],
        );
    }
}
