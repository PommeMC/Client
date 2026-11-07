//! One native directory mounted as a client namespace with sparse aliases.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::{fs, io};

use serde_json::Value;

use super::path::{
    file_supplier, list_namespace, missing_directory, validate_namespace, validate_path,
};
use super::{
    IoSupplier, METADATA_EXTENSION, PackMetadataResources, PackResources, PackType,
    PathPackResources, ResourceOutput,
};
use crate::resources::packs::{PackLocationInfo, PackMetadata, ResourcesSupplier};
use crate::resources::resource::{ReadStream, ResourceId};

pub(crate) struct MappedPackResources {
    root: PathPackResources,
    mapping: Rc<PreparedMapping>,
}

/// Immutable lexical configuration only; no opened views or metadata caches.
pub(crate) struct MappedResourcesSupplier {
    root: PathBuf,
    mapping: Rc<PreparedMapping>,
}

impl MappedResourcesSupplier {
    /// Resolves aliases and their companions once, without filesystem I/O.
    pub(crate) fn new(
        root: PathBuf,
        namespace: String,
        remappings: BTreeMap<ResourceId, PathBuf>,
    ) -> io::Result<Self> {
        Ok(Self {
            root,
            mapping: Rc::new(PreparedMapping::new(namespace, remappings)?),
        })
    }

    fn open(&self, location: &PackLocationInfo) -> MappedPackResources {
        MappedPackResources::from_prepared(
            location.clone(),
            self.root.clone(),
            self.mapping.clone(),
        )
    }
}

impl ResourcesSupplier for MappedResourcesSupplier {
    fn open_metadata(
        &self,
        location: &PackLocationInfo,
    ) -> io::Result<Rc<dyn PackMetadataResources>> {
        Ok(Rc::new(self.open(location)))
    }

    fn open_resources(
        &self,
        location: &PackLocationInfo,
        _metadata: &PackMetadata,
    ) -> io::Result<Vec<Rc<dyn PackResources>>> {
        Ok(vec![Rc::new(self.open(location))])
    }
}

struct PreparedMapping {
    namespace: String,
    /// Includes one derived companion per explicit mapping. Exact mappings
    /// always win, even when their target is absent. Translation is
    /// single-step.
    remappings: BTreeMap<ResourceId, PathBuf>,
}

impl MappedPackResources {
    /// Validates lexical configuration once, without filesystem I/O. As for
    /// PathPackResources, setup owns root availability and symlink policy.
    /// The namespace is a declared mount, not a live existence assertion;
    /// an unavailable root can still be constructed and lists no files.
    #[cfg(test)]
    pub(crate) fn new(
        location: PackLocationInfo,
        root: PathBuf,
        namespace: String,
        remappings: BTreeMap<ResourceId, PathBuf>,
    ) -> io::Result<Self> {
        Ok(Self::from_prepared(
            location,
            root,
            Rc::new(PreparedMapping::new(namespace, remappings)?),
        ))
    }

    fn from_prepared(
        location: PackLocationInfo,
        root: PathBuf,
        mapping: Rc<PreparedMapping>,
    ) -> Self {
        Self {
            root: PathPackResources::new(location, root),
            mapping,
        }
    }
}

impl PreparedMapping {
    fn new(namespace: String, remappings: BTreeMap<ResourceId, PathBuf>) -> io::Result<Self> {
        validate_namespace(&namespace)?;
        for (id, path) in &remappings {
            if id.namespace() != namespace
                || validate_path(id.path()).is_err()
                || !valid_relative_path(path)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid directory remapping {id} -> {}", path.display()),
                ));
            }
        }
        let mut resolved = remappings.clone();
        for (id, path) in remappings {
            let companion =
                ResourceId::new(&format!("{id}{METADATA_EXTENSION}")).map_err(io::Error::other)?;
            let mut companion_path = path.into_os_string();
            companion_path.push(METADATA_EXTENSION);
            resolved.entry(companion).or_insert(companion_path.into());
        }
        Ok(Self {
            namespace,
            remappings: resolved,
        })
    }
}

impl PackMetadataResources for MappedPackResources {
    fn location(&self) -> &PackLocationInfo {
        self.root.location()
    }

    fn get_root_resource(&self, path: &[&str]) -> io::Result<Option<IoSupplier<ReadStream>>> {
        self.root.get_root_resource(path)
    }

    fn metadata_section_json(&self, name: &str) -> io::Result<Option<Value>> {
        self.root.metadata_section_json(name)
    }

    /// No retained open handles. Suppliers and metadata survive repeated close.
    fn close(&self) -> io::Result<()> {
        self.root.close()
    }
}

impl PackResources for MappedPackResources {
    fn get_resource(
        &self,
        _pack_type: PackType,
        id: &ResourceId,
    ) -> io::Result<Option<IoSupplier<ReadStream>>> {
        validate_namespace(id.namespace())?;
        validate_path(id.path())?;
        if id.namespace() != self.mapping.namespace {
            return Ok(None);
        }
        let relative = self
            .mapping
            .remappings
            .get(id)
            .map_or(Path::new(id.path()), PathBuf::as_path);
        let path = self.root.root().join(relative);
        // Mapped resources require a regular file.
        // Only the selected candidate is inspected, never source ancestors.
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => Ok(Some(file_supplier(path))),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("resource is not a file: {}", path.display()),
            )),
            Err(error) if missing_directory(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn list_resources(
        &self,
        _pack_type: PackType,
        namespace: &str,
        directory: &str,
        output: &mut ResourceOutput<'_>,
    ) -> io::Result<()> {
        validate_namespace(namespace)?;
        if directory.is_empty() || namespace != self.mapping.namespace {
            return Ok(());
        }
        validate_path(directory)?;
        list_namespace(
            self.root.root(),
            namespace,
            directory,
            &mut |id, supplier| {
                // Only files reach this callback: a remapped directory name must
                // not prune its ordinary descendants. Derived sidecars shadow too.
                if !self.mapping.remappings.contains_key(&id) {
                    output(id, supplier);
                }
            },
        )?;
        for (id, relative) in &self.mapping.remappings {
            if !id.is_under(directory) {
                continue;
            }
            let path = self.root.root().join(relative);
            // Explicitly configured aliases use ordinary candidate metadata,
            // including trusted links, independently of the physical prefix.
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => output(id.clone(), file_supplier(path)),
                Ok(_) => {}
                Err(error) if missing_directory(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn get_namespaces(&self, _pack_type: PackType) -> io::Result<HashSet<String>> {
        Ok(HashSet::from([self.mapping.namespace.clone()]))
    }
}

// Native targets need not use ResourceId's alphabet (spaces/Unicode are valid),
// but must satisfy the setup-only, relative normal-component policy.
fn valid_relative_path(path: &Path) -> bool {
    path.to_str().is_some_and(|path| {
        !path.contains(['\\', ':'])
            && path
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
    }) && path
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
}

#[cfg(test)]
mod test_fixture {
    use std::collections::BTreeMap;
    use std::fs;

    use super::MappedPackResources;
    use crate::resources::packs::PackLocationInfo;
    use crate::resources::test_dir::TempDir;

    pub(super) fn default_location() -> PackLocationInfo {
        PackLocationInfo::test("mapped-api")
    }

    pub(super) fn fixture(location: PackLocationInfo) -> (TempDir, MappedPackResources) {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.0.join("test/nested")).unwrap();
        fs::write(temp.0.join("test/one.txt"), b"client one").unwrap();
        fs::write(temp.0.join("test/nested/two.txt"), b"client two").unwrap();
        fs::write(temp.0.join("root.txt"), b"mapped root").unwrap();
        fs::write(
            temp.0.join("pack.mcmeta"),
            br#"{"test":{"value":"mapped"},"other":{"value":"other"}}"#,
        )
        .unwrap();
        let pack =
            MappedPackResources::new(location, temp.0.clone(), "api_test".into(), BTreeMap::new())
                .unwrap();
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
        tests::test_get_root_resource(&pack, &["root.txt"], b"mapped root");
    }

    #[test]
    fn test_get_root_resource_missing() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_root_resource_missing(&pack, &["missing.txt"]);
    }

    #[test]
    fn test_metadata_section_json() {
        let (_temp, pack) = fixture(default_location());
        tests::test_metadata_section_json(&pack, "test", Some(json!({"value": "mapped"})));
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
            id: "mapped-location".into(),
            title: Component::translate("location.mapped.title", Vec::new()),
            known_pack_info: Some(pomme_protocol::KnownPack {
                namespace: "location_test".into(),
                id: "mapped_known".into(),
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
        tests::test_pack_id(&pack, "mapped-api");
    }
}

#[cfg(test)]
mod resources_supplier_tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::MappedResourcesSupplier;
    use crate::resources::packs::ResourcesSupplier;

    #[test]
    fn test_open() {
        let supplier = MappedResourcesSupplier::new(
            PathBuf::from("unused-supplier-root"),
            "api_test".into(),
            BTreeMap::new(),
        )
        .unwrap();
        <dyn ResourcesSupplier>::test_open(&supplier, 1);
    }
}

#[cfg(test)]
mod path_validation {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::{fs, io};

    use super::test_fixture::default_location;
    use super::{MappedPackResources, MappedResourcesSupplier};
    use crate::resources::pack_resources::{PackType, path_validation as assertions};
    use crate::resources::resource::ResourceId;
    use crate::resources::test_dir::TempDir;

    fn fixture() -> (TempDir, MappedPackResources) {
        let temp = TempDir::new().unwrap();
        for directory in ["pack/valid", "pack/physical", "pack-evil", "outside"] {
            fs::create_dir_all(temp.0.join(directory)).unwrap();
        }
        fs::write(temp.0.join("pack/valid/inside.txt"), b"inside").unwrap();
        fs::write(temp.0.join("pack/physical/target.txt"), b"alias target").unwrap();
        fs::write(temp.0.join("pack-evil/sentinel.txt"), b"sibling sentinel").unwrap();
        fs::write(temp.0.join("outside/sentinel.txt"), b"outside sentinel").unwrap();
        let pack = MappedPackResources::new(
            default_location(),
            temp.0.join("pack"),
            "test".into(),
            BTreeMap::from([(
                ResourceId::from_parts("test", "valid/alias.txt").unwrap(),
                PathBuf::from("physical/target.txt"),
            )]),
        )
        .unwrap();
        (temp, pack)
    }

    #[test]
    fn get_resource_rejects_escape() {
        let (temp, pack) = fixture();
        let inside = ResourceId::from_parts("test", "valid/inside.txt").unwrap();
        let alias = ResourceId::from_parts("test", "valid/alias.txt").unwrap();
        let invalid = [
            ("test", "../outside/sentinel.txt"),
            ("test", "../pack-evil/sentinel.txt"),
            ("test", "/outside/sentinel.txt"),
        ]
        .map(|(namespace, path)| ResourceId::from_parts(namespace, path).unwrap());
        assertions::assert_get_resource_invalid_input(
            &pack,
            PackType::ClientResources,
            &[(&inside, b"inside"), (&alias, b"alias target")],
            &invalid,
        );
        #[cfg(unix)]
        assertions::assert_get_resource_invalid_input(
            &pack,
            PackType::ClientResources,
            &[(&alias, b"alias target")],
            &[ResourceId::from_parts(
                "test",
                temp.0.join("outside/sentinel.txt").to_str().unwrap(),
            )
            .unwrap()],
        );
        #[cfg(not(unix))]
        let _ = temp;
    }

    #[test]
    fn list_resources_rejects_escape() {
        let (temp, pack) = fixture();
        let absolute = temp.0.join("outside");
        assertions::assert_list_resources_invalid_input(
            &pack,
            PackType::ClientResources,
            "test",
            "valid",
            &[
                (
                    ResourceId::from_parts("test", "valid/inside.txt").unwrap(),
                    b"inside".to_vec(),
                ),
                (
                    ResourceId::from_parts("test", "valid/alias.txt").unwrap(),
                    b"alias target".to_vec(),
                ),
            ],
            &[
                ("test", "../outside"),
                ("test", "../pack-evil"),
                ("test", "/outside"),
                ("test", absolute.to_str().unwrap()),
                ("..", "valid"),
            ],
        );
    }

    #[test]
    fn supplier_new_rejects_escape_targets() {
        let (temp, _pack) = fixture();
        let alias = ResourceId::from_parts("test", "valid/alias.txt").unwrap();
        assert!(
            MappedResourcesSupplier::new(
                temp.0.join("pack"),
                "test".into(),
                BTreeMap::from([(alias.clone(), PathBuf::from("physical/target.txt"))]),
            )
            .is_ok()
        );
        for target in [
            PathBuf::from("../outside/sentinel.txt"),
            temp.0.join("outside/sentinel.txt"),
            PathBuf::from(r"outside\sentinel.txt"),
            PathBuf::from("C:/outside/sentinel.txt"),
        ] {
            match MappedResourcesSupplier::new(
                temp.0.join("pack"),
                "test".into(),
                BTreeMap::from([(alias.clone(), target.clone())]),
            ) {
                Err(error) => assert_eq!(
                    error.kind(),
                    io::ErrorKind::InvalidInput,
                    "target {target:?}"
                ),
                Ok(_) => panic!("constructor accepted escaping target {target:?}"),
            }
        }
    }
}
