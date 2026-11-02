//! Directory-backed resources following Vanilla's PathPackResources.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{Map, Value};

use super::{
    IoSupplier, OverlayedPackResources, PACK_META, PackMetadataResources, PackResources, PackType,
    ResourceOutput,
};
use crate::resources::packs::{PackLocationInfo, PackMetadata, ResourcesSupplier};
use crate::resources::resource::{
    ReadStream, ResourceId, resource_namespace_chars, resource_path_chars,
};

/// Setup owns root validation and symlink policy. Access trusts that setup;
/// only lexical paths and the requested filesystem entries are inspected.
/// Metadata is a lazy snapshot, but resource suppliers reopen current bytes.
pub(crate) struct PathPackResources {
    location: PackLocationInfo,
    root: PathBuf,
    metadata: RefCell<Option<Map<String, Value>>>,
}

/// Reopenable directory definition. The caller owns setup and symlink policy;
/// construction and opening only store paths, with fresh metadata per view.
pub(crate) struct PathResourcesSupplier {
    root: PathBuf,
}

impl PathResourcesSupplier {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl ResourcesSupplier for PathResourcesSupplier {
    fn open_metadata(
        &self,
        location: &PackLocationInfo,
    ) -> io::Result<Rc<dyn PackMetadataResources>> {
        Ok(Rc::new(PathPackResources::new(
            location.clone(),
            self.root.clone(),
        )))
    }

    fn open_resources(
        &self,
        location: &PackLocationInfo,
        metadata: &PackMetadata,
    ) -> io::Result<Vec<Rc<dyn PackResources>>> {
        let primary = Rc::new(PathPackResources::new(location.clone(), self.root.clone()));
        if metadata.overlays.is_empty() {
            return Ok(vec![primary]);
        }
        // Pack metadata already validated and selected these directories.
        // Composition keeps overlays within one manager entry, in declaration order.
        let overlays = metadata
            .overlays
            .iter()
            .map(|directory| {
                Rc::new(PathPackResources::new(
                    location.clone(),
                    self.root.join(directory),
                )) as Rc<dyn PackResources>
            })
            .collect();
        Ok(vec![Rc::new(OverlayedPackResources::new(
            primary, overlays,
        ))])
    }
}

impl PathPackResources {
    /// Like Vanilla, construction stores the root without filesystem I/O.
    pub(crate) fn new(location: PackLocationInfo, root: PathBuf) -> Self {
        Self {
            location,
            root,
            metadata: RefCell::new(None),
        }
    }

    fn load_metadata(&self) -> io::Result<Map<String, Value>> {
        read_metadata(
            self.get_root_resource(&[PACK_META])?,
            &self.root.join(PACK_META),
        )
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }
}

impl PackMetadataResources for PathPackResources {
    fn location(&self) -> &PackLocationInfo {
        &self.location
    }

    fn get_root_resource(&self, path: &[&str]) -> io::Result<Option<IoSupplier<ReadStream>>> {
        if path.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty root resource path",
            ));
        }
        let mut candidate = self.root.clone();
        for segment in path {
            validate_segment(segment)?;
            candidate.push(segment);
        }
        locate(candidate)
    }

    fn metadata_section_json(&self, name: &str) -> io::Result<Option<Value>> {
        if let Some(metadata) = self.metadata.borrow().as_ref() {
            return Ok(metadata.get(name).cloned());
        }
        // Do not borrow the cache while loading. Failed loads remain retryable;
        // successful objects and missing files are cached for this instance.
        let metadata = self.load_metadata()?;
        let section = metadata.get(name).cloned();
        *self.metadata.borrow_mut() = Some(metadata);
        Ok(section)
    }

    /// No handles are owned by the pack. Repeated close is a no-op; retained
    /// suppliers, open streams and cached metadata remain usable.
    fn close(&self) -> io::Result<()> {
        Ok(())
    }
}

impl PackResources for PathPackResources {
    fn get_resource(
        &self,
        pack_type: PackType,
        id: &ResourceId,
    ) -> io::Result<Option<IoSupplier<ReadStream>>> {
        validate_segment(id.namespace())?;
        validate_path(id.path())?;
        locate(
            self.root
                .join(pack_type.directory())
                .join(id.namespace())
                .join(id.path()),
        )
    }

    fn list_resources(
        &self,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        output: &mut ResourceOutput<'_>,
    ) -> io::Result<()> {
        validate_namespace(namespace)?;
        let namespace_root = self.root.join(pack_type.directory()).join(namespace);
        list_namespace(&namespace_root, namespace, directory, output)
    }

    fn get_namespaces(&self, pack_type: PackType) -> io::Result<HashSet<String>> {
        directory_namespaces(&self.root.join(pack_type.directory()))
    }
}

/// Shared setup scan for built-in snapshots; ordinary Path views remain live.
pub(super) fn directory_namespaces(root: &Path) -> io::Result<HashSet<String>> {
    let mut namespaces = HashSet::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if missing_directory(&error) => return Ok(namespaces),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        // Ordinary directory entries need no additional stat. Namespace
        // discovery follows links, unlike resource tree enumeration.
        let is_directory = if kind.is_symlink() {
            match fs::metadata(entry.path()) {
                Ok(metadata) => metadata.is_dir(),
                Err(error) if missing_directory(&error) => false,
                Err(error) => return Err(error),
            }
        } else {
            kind.is_dir()
        };
        if !is_directory {
            tracing::warn!(path = %entry.path().display(), "Non-directory namespace entry; ignoring");
            continue;
        }
        let name = entry.file_name();
        match name.to_str() {
            Some(namespace) if validate_namespace(namespace).is_ok() => {
                namespaces.insert(namespace.to_owned());
            }
            _ => tracing::warn!(path = %entry.path().display(), "Invalid namespace; ignoring"),
        }
    }
    Ok(namespaces)
}

/// Logical IDs permit dot/parent components; filesystem resolution must not.
/// The existing ASCII alphabet also excludes roots, drive prefixes and NULs,
/// without imposing Java's lowercase-only restriction on resource paths.
pub(super) fn validate_segment(segment: &str) -> io::Result<()> {
    if segment.is_empty()
        || matches!(segment, "." | "..")
        || segment.contains('/')
        || !resource_path_chars(segment)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid filesystem resource segment",
        ));
    }
    Ok(())
}

pub(super) fn validate_path(path: &str) -> io::Result<()> {
    path.split('/').try_for_each(validate_segment)
}

pub(super) fn validate_namespace(namespace: &str) -> io::Result<()> {
    validate_segment(namespace)?;
    if !resource_namespace_chars(namespace) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid resource namespace",
        ));
    }
    Ok(())
}

pub(super) fn missing_directory(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// Vanilla checks existence, not file type. Non-files are left to ordinary
/// stream I/O; missing candidates or non-directory ancestors mean absence.
fn locate(path: PathBuf) -> io::Result<Option<IoSupplier<ReadStream>>> {
    match fs::metadata(&path) {
        Ok(_) => Ok(Some(file_supplier(path))),
        Err(error) if missing_directory(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn file_supplier(path: PathBuf) -> IoSupplier<ReadStream> {
    Arc::new(move || File::open(&path).map(|file| Box::new(file) as ReadStream))
}

/// Shared metadata envelope parsing; each pack view owns its successful cache.
pub(super) fn read_metadata(
    supplier: Option<IoSupplier<ReadStream>>,
    path: &Path,
) -> io::Result<Map<String, Value>> {
    let Some(supplier) = supplier else {
        return Ok(Map::new());
    };
    let value: Value = serde_json::from_reader(supplier()?).map_err(|error| {
        io::Error::new(
            error.io_error_kind().unwrap_or(io::ErrorKind::InvalidData),
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    match value {
        Value::Object(object) => Ok(object),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} must contain a JSON object", path.display()),
        )),
    }
}

/// Enumerate one native namespace root without following discovered links.
/// Callers validate the namespace; prefix validation stays shared here.
pub(super) fn list_namespace(
    namespace_root: &Path,
    namespace: &str,
    directory: &str,
    output: &mut ResourceOutput<'_>,
) -> io::Result<()> {
    // Joining an empty component can add a trailing separator, which would
    // make an OS follow a symlink at the walk's start. Keep the root intact.
    let start = if directory.is_empty() {
        namespace_root.to_owned()
    } else {
        validate_path(directory)?;
        namespace_root.join(directory)
    };
    let kind = match fs::symlink_metadata(&start) {
        Ok(metadata) => metadata.file_type(),
        Err(error) if missing_directory(&error) => return Ok(()),
        Err(error) => return Err(error),
    };
    if kind.is_dir() {
        list_directory(&start, namespace_root, namespace, output)?;
    } else if kind.is_file() {
        emit_resource(start, namespace_root, namespace, output)?;
    }
    Ok(())
}

fn list_directory(
    directory: &Path,
    namespace_root: &Path,
    namespace: &str,
    output: &mut ResourceOutput<'_>,
) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if missing_directory(&error) => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        // Like Files.find's supplied attributes, classify each entry once and
        // never follow links encountered in the walk.
        let kind = entry.file_type()?;
        if kind.is_dir() {
            // Unlike Vanilla's Files.find (which filters file IDs afterward),
            // skip trees that cannot contain representable logical IDs. This
            // avoids unlistable work in both path and mapped enumeration.
            // Native entry names are single components, so '/' cannot occur.
            if !entry.file_name().to_str().is_some_and(resource_path_chars) {
                tracing::warn!(path = %entry.path().display(), "Invalid resource directory name; ignoring subtree");
                continue;
            }
            list_directory(&entry.path(), namespace_root, namespace, output)?;
        } else if kind.is_file() {
            emit_resource(entry.path(), namespace_root, namespace, output)?;
        }
    }
    Ok(())
}

fn emit_resource(
    path: PathBuf,
    namespace_root: &Path,
    namespace: &str,
    output: &mut ResourceOutput<'_>,
) -> io::Result<()> {
    let relative = path
        .strip_prefix(namespace_root)
        .map_err(io::Error::other)?;
    let mut resource_path = String::new();
    for (index, component) in relative.iter().enumerate() {
        let Some(part) = component.to_str() else {
            tracing::warn!(path = %path.display(), "Non-UTF-8 resource path; ignoring");
            return Ok(());
        };
        if index > 0 {
            resource_path.push('/');
        }
        resource_path.push_str(part);
    }
    match ResourceId::from_parts(namespace, &resource_path) {
        Ok(id) => output(id, file_supplier(path)),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "Invalid resource path; ignoring")
        }
    }
    Ok(())
}

#[cfg(test)]
mod test_fixture {
    use std::path::PathBuf;
    use std::{fs, io};

    use super::PathPackResources;
    use crate::chat_component::Component;
    use crate::resources::packs::{PackLocationInfo, PackSource};

    pub(super) struct TempDir(pub(super) PathBuf);

    impl TempDir {
        fn new() -> io::Result<Self> {
            for attempt in 0.. {
                let path = std::env::temp_dir()
                    .join(format!("pomme-path-api-{}-{attempt}", std::process::id()));
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
            id: "path-api".into(),
            title: Component::text("Path API fixture"),
            source: PackSource::built_in(),
            known_pack_info: None,
        }
    }

    pub(super) fn fixture(location: PackLocationInfo) -> (TempDir, PathPackResources) {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.0.join("assets/api_test/test/nested")).unwrap();
        fs::create_dir_all(temp.0.join("data/api_data/test")).unwrap();
        fs::write(temp.0.join("assets/api_test/test/one.txt"), b"client one").unwrap();
        fs::write(
            temp.0.join("assets/api_test/test/nested/two.txt"),
            b"client two",
        )
        .unwrap();
        fs::write(temp.0.join("data/api_data/test/server.txt"), b"server data").unwrap();
        fs::write(temp.0.join("root.txt"), b"path root").unwrap();
        fs::write(
            temp.0.join("pack.mcmeta"),
            br#"{"test":{"value":"path"},"other":{"value":"other"}}"#,
        )
        .unwrap();
        let pack = PathPackResources::new(location, temp.0.clone());
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
        tests::test_get_root_resource(&pack, &["root.txt"], b"path root");
    }

    #[test]
    fn test_get_root_resource_missing() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_root_resource_missing(&pack, &["missing.txt"]);
    }

    #[test]
    fn test_metadata_section_json() {
        let (_temp, pack) = fixture(default_location());
        tests::test_metadata_section_json(&pack, "test", Some(json!({"value": "path"})));
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
            id: "path-location".into(),
            title: Component::translate("location.path.title", Vec::new()),
            source: PackSource::feature(),
            known_pack_info: Some(KnownPack {
                namespace: "location_test".into(),
                id: "path_known".into(),
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
        tests::test_get_metadata_section(&pack, &section, Some("path".into()));
    }

    #[test]
    fn test_get_metadata_section_missing() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_metadata_section_missing(&pack, "missing");
    }

    #[test]
    fn test_get_metadata_section_null() {
        let (temp, pack) = fixture(default_location());
        fs::write(temp.0.join("pack.mcmeta"), br#"{"null":null}"#).unwrap();
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
        tests::test_pack_id(&pack, "path-api");
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
            id: "path_known".into(),
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

#[cfg(test)]
mod resources_supplier_tests {
    use std::path::PathBuf;

    use super::PathResourcesSupplier;
    use crate::chat_component::Component;
    use crate::resources::packs::{
        FeatureFlagSet, PackCompat, PackLocationInfo, PackMetadata, PackSource, ResourcesSupplier,
    };

    fn location() -> PackLocationInfo {
        PackLocationInfo {
            id: "path-supplier-api".into(),
            title: Component::text("Supplier API test"),
            source: PackSource::default(),
            known_pack_info: None,
        }
    }

    #[test]
    fn test_open_metadata() {
        let supplier = PathResourcesSupplier::new(PathBuf::from("unused-supplier-root"));
        let location = location();
        <dyn ResourcesSupplier>::test_open_metadata(&supplier, &location, "path-supplier-api");
    }

    #[test]
    fn test_open_resources() {
        let supplier = PathResourcesSupplier::new(PathBuf::from("unused-supplier-root"));
        let location = location();
        let metadata = PackMetadata {
            description: Component::text("Supplier API test"),
            compatibility: PackCompat::Compatible,
            requested_features: FeatureFlagSet::empty(),
            overlays: Vec::new(),
        };
        <dyn ResourcesSupplier>::test_open_resources(
            &supplier,
            &location,
            &metadata,
            &["path-supplier-api"],
        );
    }
}

#[cfg(test)]
mod path_validation {
    use std::path::PathBuf;
    use std::{fs, io};

    use super::PathPackResources;
    use super::test_fixture::default_location;
    use crate::resources::pack_resources::{PackType, path_validation as assertions};
    use crate::resources::resource::ResourceId;

    const METADATA: &[u8] = br#"{"pack":{"pack_format":47,"description":"inside"}}"#;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> io::Result<Self> {
            for attempt in 0.. {
                let path = std::env::temp_dir().join(format!(
                    "pomme-path-validation-{}-{attempt}",
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

    fn fixture() -> (TempDir, PathPackResources) {
        let temp = TempDir::new().unwrap();
        for directory in [
            "pack/assets/test/valid",
            "pack/data/test/valid",
            "pack-evil",
            "outside",
        ] {
            fs::create_dir_all(temp.0.join(directory)).unwrap();
        }
        for domain in ["assets", "data"] {
            fs::write(
                temp.0.join(format!("pack/{domain}/test/valid/inside.txt")),
                b"inside",
            )
            .unwrap();
        }
        fs::write(temp.0.join("pack/pack.mcmeta"), METADATA).unwrap();
        fs::write(temp.0.join("pack-evil/sentinel.txt"), b"sibling sentinel").unwrap();
        fs::write(temp.0.join("outside/sentinel.txt"), b"outside sentinel").unwrap();
        let pack = PathPackResources::new(default_location(), temp.0.join("pack"));
        (temp, pack)
    }

    fn invalid_ids(temp: &TempDir) -> Vec<ResourceId> {
        let ids = [
            ("test", "../../../outside/sentinel.txt"),
            ("test", "../../../pack-evil/sentinel.txt"),
            ("test", "/outside/sentinel.txt"),
            ("..", "pack.mcmeta"),
        ]
        .into_iter()
        .map(|(namespace, path)| ResourceId::from_parts(namespace, path).unwrap());
        // Native drive prefixes are not ResourceIds; root-resource tests cover them.
        #[cfg(unix)]
        let absolute = Some(
            ResourceId::from_parts(
                "test",
                temp.0.join("outside/sentinel.txt").to_str().unwrap(),
            )
            .unwrap(),
        );
        #[cfg(not(unix))]
        let absolute = {
            let _ = temp;
            None
        };
        ids.chain(absolute).collect()
    }

    #[test]
    fn get_resource_rejects_escape() {
        let (temp, pack) = fixture();
        let id = ResourceId::from_parts("test", "valid/inside.txt").unwrap();
        for pack_type in [PackType::ClientResources, PackType::ServerData] {
            assertions::assert_get_resource_invalid_input(
                &pack,
                pack_type,
                &[(&id, b"inside")],
                &invalid_ids(&temp),
            );
        }
    }

    #[test]
    fn get_root_resource_rejects_escape() {
        let (temp, pack) = fixture();
        let absolute = temp.0.join("outside/sentinel.txt");
        assertions::assert_get_root_resource_invalid_input(
            &pack,
            &[(&["pack.mcmeta"], METADATA)],
            &[
                &["..", "outside", "sentinel.txt"],
                &[absolute.to_str().unwrap()],
                &["outside/sentinel.txt"],
                &[r"outside\sentinel.txt"],
                &["C:/outside/sentinel.txt"],
            ],
        );
    }

    #[test]
    fn list_resources_rejects_escape() {
        let (temp, pack) = fixture();
        let absolute = temp.0.join("outside");
        for pack_type in [PackType::ClientResources, PackType::ServerData] {
            assertions::assert_list_resources_invalid_input(
                &pack,
                pack_type,
                "test",
                "valid",
                &[(
                    ResourceId::from_parts("test", "valid/inside.txt").unwrap(),
                    b"inside".to_vec(),
                )],
                &[
                    ("test", "../../../outside"),
                    ("test", "../../../pack-evil"),
                    ("test", "/outside"),
                    ("test", absolute.to_str().unwrap()),
                    ("..", ""),
                ],
            );
        }
    }
}
