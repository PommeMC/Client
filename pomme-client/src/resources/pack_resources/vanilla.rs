//! Minecraft's built-in full metadata view and ordered resource layers,
//! corresponding to Vanilla 26.3's VanillaPackResources.asResourcesSupplier.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::{Map, Value};

use super::path::{
    directory_namespaces, file_supplier, read_metadata, validate_namespace, validate_path,
    validate_segment,
};
use super::{
    IoSupplier, PACK_META, PackMetadataResources, PackResources, PackType, PathPackResources,
    ResourceOutput,
};
use crate::resources::packs::{PackLocationInfo, PackMetadata, ResourcesSupplier};
use crate::resources::resource::{ReadStream, ResourceId};

/// One Minecraft pack, not the separate Pomme embedded pack. The index is the
/// higher-priority client/root layer; extracted assets/data are the fallback.
/// Native object paths replace Java's LinkFileSystem, without a public index
/// abstraction.
pub(crate) struct VanillaPackResources {
    extracted: PathPackResources,
    prepared: Rc<PreparedVanillaResources>,
    metadata: RefCell<Option<Map<String, Value>>>,
}

/// Prepared once per logical source load, not once per metadata/resource open.
/// No live view or mutable metadata cache is retained by this supplier.
pub(crate) struct VanillaResourcesSupplier {
    prepared: Rc<PreparedVanillaResources>,
}

impl VanillaResourcesSupplier {
    /// Anchors roots, reads the explicit index, and snapshots namespaces once.
    /// extracted_root is the parent of assets/data. Fallbacks are named JSON
    /// sections supplied by the logical source, not a synthetic root file.
    pub(crate) fn new(
        extracted_root: PathBuf,
        index_file: &Path,
        objects_dir: &Path,
        built_in_metadata: Map<String, Value>,
    ) -> io::Result<Self> {
        Ok(Self {
            prepared: Rc::new(PreparedVanillaResources::new(
                extracted_root,
                index_file,
                objects_dir,
                built_in_metadata,
            )?),
        })
    }
}

impl ResourcesSupplier for VanillaResourcesSupplier {
    fn open_metadata(
        &self,
        location: &PackLocationInfo,
    ) -> io::Result<Rc<dyn PackMetadataResources>> {
        Ok(Rc::new(VanillaPackResources::from_prepared(
            location.clone(),
            self.prepared.clone(),
        )))
    }

    fn open_resources(
        &self,
        location: &PackLocationInfo,
        _metadata: &PackMetadata,
    ) -> io::Result<Vec<Rc<dyn PackResources>>> {
        // Like Java, fullResources is for metadata, not an extra resource layer.
        // Each physical entry reads only its own root metadata, then the shared
        // fallbacks. Filters can therefore act between these two built-in layers.
        Ok(vec![
            Rc::new(ExtractedResources {
                root: PathPackResources::new(
                    location.clone(),
                    self.prepared.extracted_root.clone(),
                ),
                prepared: self.prepared.clone(),
            }),
            Rc::new(IndexResources {
                location: location.clone(),
                prepared: self.prepared.clone(),
                metadata: RefCell::new(None),
            }),
        ])
    }
}

struct PreparedVanillaResources {
    extracted_root: PathBuf,
    indexed: HashMap<String, IndexedEntry>,
    extracted_client_namespaces: HashSet<String>,
    extracted_server_namespaces: HashSet<String>,
    indexed_namespaces: HashSet<String>,
    /// The useful combined view preserves its existing cross-domain union.
    namespaces: HashSet<String>,
    built_in_metadata: Map<String, Value>,
}

struct IndexedEntry {
    path: PathBuf,
    /// Root-only keys (for example pack.mcmeta) have no namespaced ID.
    id: Option<ResourceId>,
}

impl VanillaPackResources {
    /// Setup opens the explicit index once and validates/canonicalizes both
    /// native directory roots once. extracted_root is the PARENT of assets,
    /// not assets itself. No client JAR, downloads or configuration mutation.
    ///
    /// built_in_metadata contains named JSON sections, not an invented root
    /// file. An empty map means no fallback sections. Namespaces are captured
    /// from both extracted domains and valid indexed IDs at construction.
    pub(crate) fn new(
        location: PackLocationInfo,
        extracted_root: PathBuf,
        index_file: &Path,
        objects_dir: &Path,
        built_in_metadata: Map<String, Value>,
    ) -> io::Result<Self> {
        Ok(Self::from_prepared(
            location,
            Rc::new(PreparedVanillaResources::new(
                extracted_root,
                index_file,
                objects_dir,
                built_in_metadata,
            )?),
        ))
    }

    fn from_prepared(location: PackLocationInfo, prepared: Rc<PreparedVanillaResources>) -> Self {
        Self {
            extracted: PathPackResources::new(location, prepared.extracted_root.clone()),
            prepared,
            metadata: RefCell::new(None),
        }
    }
}

impl PreparedVanillaResources {
    fn new(
        extracted_root: PathBuf,
        index_file: &Path,
        objects_dir: &Path,
        built_in_metadata: Map<String, Value>,
    ) -> io::Result<Self> {
        let extracted_root = canonical_directory(&extracted_root)?;
        let objects_dir = canonical_directory(objects_dir)?;
        let index: Value = serde_json::from_reader(BufReader::new(File::open(index_file)?))
            .map_err(|error| {
                io::Error::new(
                    error.io_error_kind().unwrap_or(io::ErrorKind::InvalidData),
                    format!("cannot read asset index {}: {error}", index_file.display()),
                )
            })?;
        let objects = index
            .get("objects")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "asset index objects must be an object",
                )
            })?;
        let extracted_client_namespaces = directory_namespaces(&extracted_root.join("assets"))?;
        let extracted_server_namespaces = directory_namespaces(&extracted_root.join("data"))?;
        let mut indexed_namespaces = HashSet::new();
        let mut indexed = HashMap::with_capacity(objects.len());
        for (key, object) in objects {
            let hash = object
                .get("hash")
                .and_then(Value::as_str)
                .filter(|hash| {
                    hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid asset-index hash for {key}"),
                    )
                })?;
            // Logical keys never become native paths. Still reject traversal
            // components to match the directory view's lexical contract.
            if validate_path(key).is_err() {
                tracing::warn!(%key, "Invalid asset-index path; ignoring");
                continue;
            }
            let id = match key.split_once('/') {
                Some((namespace, path)) => match ResourceId::from_parts(namespace, path) {
                    Ok(id) => Some(id),
                    Err(error) => {
                        // Preserve the literal root key, but do not advertise an
                        // unrepresentable namespaced entry during enumeration.
                        tracing::warn!(%key, %error, "Invalid asset-index resource ID; retaining root key only");
                        None
                    }
                },
                None => None,
            };
            if let Some(id) = &id {
                indexed_namespaces.insert(id.namespace().to_owned());
            }
            indexed.insert(
                key.clone(),
                IndexedEntry {
                    path: objects_dir.join(&hash[..2]).join(hash),
                    id,
                },
            );
        }
        // COMPAT: the legacy panorama accepts a flat extracted-root override
        // before the indexed/canonical face, independently for each face.
        for face in 0..6 {
            let path = extracted_root.join(format!("assets/panorama_{face}.png"));
            if path.exists() {
                let key = format!("minecraft/textures/gui/title/background/panorama_{face}.png");
                let id = ResourceId::from_parts(
                    "minecraft",
                    &format!("textures/gui/title/background/panorama_{face}.png"),
                )
                .map_err(io::Error::other)?;
                indexed.insert(key, IndexedEntry { path, id: Some(id) });
                indexed_namespaces.insert("minecraft".to_owned());
            }
        }
        let mut namespaces = extracted_client_namespaces.clone();
        namespaces.extend(extracted_server_namespaces.iter().cloned());
        namespaces.extend(indexed_namespaces.iter().cloned());
        Ok(Self {
            extracted_root,
            indexed,
            extracted_client_namespaces,
            extracted_server_namespaces,
            indexed_namespaces,
            namespaces,
            built_in_metadata,
        })
    }

    fn indexed_resource(&self, key: &str) -> io::Result<Option<IoSupplier<ReadStream>>> {
        self.indexed.get(key).map_or(Ok(None), IndexedEntry::locate)
    }

    fn list_indexed(
        &self,
        namespace: &str,
        directory: &str,
        output: &mut ResourceOutput<'_>,
    ) -> io::Result<()> {
        for entry in self.indexed.values() {
            let Some(id) = &entry.id else { continue };
            if id.namespace() != namespace
                || !(directory.is_empty()
                    || id.path() == directory
                    || id
                        .path()
                        .strip_prefix(directory)
                        .is_some_and(|tail| tail.starts_with('/')))
            {
                continue;
            }
            if let Some(supplier) = entry.locate()? {
                output(id.clone(), supplier);
            }
        }
        Ok(())
    }
}

impl PackMetadataResources for VanillaPackResources {
    fn location(&self) -> &PackLocationInfo {
        self.extracted.location()
    }

    fn get_root_resource(&self, path: &[&str]) -> io::Result<Option<IoSupplier<ReadStream>>> {
        if path.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty root resource path",
            ));
        }
        for segment in path {
            validate_segment(segment)?;
        }
        if let Some(supplier) = self.prepared.indexed_resource(&path.join("/"))? {
            return Ok(Some(supplier));
        }
        self.extracted.get_root_resource(path)
    }

    fn metadata_section_json(&self, name: &str) -> io::Result<Option<Value>> {
        metadata_section(self, &self.metadata, &self.prepared.built_in_metadata, name)
    }

    /// No retained open handles: like FixedPathPackResources, close is a no-op.
    /// Owned suppliers/streams and both successful snapshots remain usable.
    fn close(&self) -> io::Result<()> {
        self.extracted.close()
    }
}

impl PackResources for VanillaPackResources {
    fn get_resource(
        &self,
        pack_type: PackType,
        id: &ResourceId,
    ) -> io::Result<Option<IoSupplier<ReadStream>>> {
        validate_namespace(id.namespace())?;
        validate_path(id.path())?;
        if pack_type == PackType::ClientResources
            && let Some(supplier) =
                self.prepared
                    .indexed_resource(&format!("{}/{}", id.namespace(), id.path()))?
        {
            return Ok(Some(supplier));
        }
        self.extracted.get_resource(pack_type, id)
    }

    fn list_resources(
        &self,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        output: &mut ResourceOutput<'_>,
    ) -> io::Result<()> {
        validate_namespace(namespace)?;
        if !directory.is_empty() {
            validate_path(directory)?;
        }
        let mut resources = HashMap::new();
        if pack_type == PackType::ClientResources {
            self.prepared
                .list_indexed(namespace, directory, &mut |id, supplier| {
                    resources.insert(id, supplier);
                })?;
        }
        // Match FixedPathPackResources' single/empty-upper-layer fast path.
        if resources.is_empty() {
            return self
                .extracted
                .list_resources(pack_type, namespace, directory, output);
        }
        self.extracted
            .list_resources(pack_type, namespace, directory, &mut |id, supplier| {
                resources.entry(id).or_insert(supplier);
            })?;
        for (id, supplier) in resources {
            output(id, supplier);
        }
        Ok(())
    }

    fn get_namespaces(&self, _pack_type: PackType) -> io::Result<HashSet<String>> {
        // Vanilla exposes one fixed set for both types, not a disk rescan.
        Ok(self.prepared.namespaces.clone())
    }
}

/// The extracted layer reuses Path operations but has its own fallback sections
/// and setup-time namespace snapshot, rather than the full view's metadata.
struct ExtractedResources {
    root: PathPackResources,
    prepared: Rc<PreparedVanillaResources>,
}

impl PackMetadataResources for ExtractedResources {
    fn location(&self) -> &PackLocationInfo {
        self.root.location()
    }

    fn get_root_resource(&self, path: &[&str]) -> io::Result<Option<IoSupplier<ReadStream>>> {
        self.root.get_root_resource(path)
    }

    fn metadata_section_json(&self, name: &str) -> io::Result<Option<Value>> {
        Ok(self
            .root
            .metadata_section_json(name)?
            .or_else(|| self.prepared.built_in_metadata.get(name).cloned()))
    }

    fn close(&self) -> io::Result<()> {
        self.root.close()
    }
}

impl PackResources for ExtractedResources {
    fn get_resource(
        &self,
        pack_type: PackType,
        id: &ResourceId,
    ) -> io::Result<Option<IoSupplier<ReadStream>>> {
        self.root.get_resource(pack_type, id)
    }

    fn list_resources(
        &self,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        output: &mut ResourceOutput<'_>,
    ) -> io::Result<()> {
        self.root
            .list_resources(pack_type, namespace, directory, output)
    }

    fn get_namespaces(&self, pack_type: PackType) -> io::Result<HashSet<String>> {
        Ok(match pack_type {
            PackType::ClientResources => self.prepared.extracted_client_namespaces.clone(),
            PackType::ServerData => self.prepared.extracted_server_namespaces.clone(),
        })
    }
}

/// Private production asset-index layer, not a generic public index framework.
/// Root keys remain literal index keys; namespaced client IDs omit assets/.
struct IndexResources {
    location: PackLocationInfo,
    prepared: Rc<PreparedVanillaResources>,
    metadata: RefCell<Option<Map<String, Value>>>,
}

impl PackMetadataResources for IndexResources {
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
        for segment in path {
            validate_segment(segment)?;
        }
        self.prepared.indexed_resource(&path.join("/"))
    }

    fn metadata_section_json(&self, name: &str) -> io::Result<Option<Value>> {
        metadata_section(self, &self.metadata, &self.prepared.built_in_metadata, name)
    }

    /// No retained open files. Existing suppliers and streams remain usable.
    fn close(&self) -> io::Result<()> {
        Ok(())
    }
}

impl PackResources for IndexResources {
    fn get_resource(
        &self,
        pack_type: PackType,
        id: &ResourceId,
    ) -> io::Result<Option<IoSupplier<ReadStream>>> {
        validate_namespace(id.namespace())?;
        validate_path(id.path())?;
        if pack_type != PackType::ClientResources {
            return Ok(None);
        }
        self.prepared
            .indexed_resource(&format!("{}/{}", id.namespace(), id.path()))
    }

    fn list_resources(
        &self,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        output: &mut ResourceOutput<'_>,
    ) -> io::Result<()> {
        validate_namespace(namespace)?;
        if !directory.is_empty() {
            validate_path(directory)?;
        }
        if pack_type == PackType::ClientResources {
            self.prepared.list_indexed(namespace, directory, output)?;
        }
        Ok(())
    }

    fn get_namespaces(&self, pack_type: PackType) -> io::Result<HashSet<String>> {
        Ok(if pack_type == PackType::ClientResources {
            self.prepared.indexed_namespaces.clone()
        } else {
            HashSet::new()
        })
    }
}

/// Each view caches only its own root envelope. Missing sections fall back;
/// I/O/JSON errors propagate, unlike Java's catch-and-log fallback policy.
fn metadata_section(
    resources: &dyn PackMetadataResources,
    cache: &RefCell<Option<Map<String, Value>>>,
    built_in_metadata: &Map<String, Value>,
    name: &str,
) -> io::Result<Option<Value>> {
    if cache.borrow().is_none() {
        let metadata = read_metadata(
            resources.get_root_resource(&[PACK_META])?,
            Path::new(PACK_META),
        )?;
        *cache.borrow_mut() = Some(metadata);
    }
    // Present null stays present and reaches the section decoder unchanged.
    Ok(cache
        .borrow()
        .as_ref()
        .and_then(|metadata| metadata.get(name))
        .or_else(|| built_in_metadata.get(name))
        .cloned())
}

impl IndexedEntry {
    fn locate(&self) -> io::Result<Option<IoSupplier<ReadStream>>> {
        // Only candidate metadata I/O, never root/bucket walks or index reload.
        match fs::metadata(&self.path) {
            Ok(metadata) if metadata.is_file() => Ok(Some(file_supplier(self.path.clone()))),
            Ok(_) => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn canonical_directory(path: &Path) -> io::Result<PathBuf> {
    let root = path.canonicalize()?;
    if !fs::metadata(&root)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("not a resource directory: {}", root.display()),
        ));
    }
    Ok(root)
}

#[cfg(test)]
mod test_fixture {
    use std::path::PathBuf;
    use std::{fs, io};

    use serde_json::Map;

    use super::VanillaPackResources;
    use crate::chat_component::Component;
    use crate::resources::packs::{PackLocationInfo, PackSource};

    pub(super) struct TempDir(pub(super) PathBuf);

    impl TempDir {
        fn new() -> io::Result<Self> {
            for attempt in 0.. {
                let path = std::env::temp_dir().join(format!(
                    "pomme-vanilla-location-{}-{attempt}",
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
            id: "vanilla-api".into(),
            title: Component::text("Vanilla API fixture"),
            source: PackSource::built_in(),
            known_pack_info: None,
        }
    }

    pub(super) fn fixture(location: PackLocationInfo) -> (TempDir, VanillaPackResources) {
        let temp = TempDir::new().unwrap();
        let extracted_root = temp.0.join("extracted");
        let objects_root = temp.0.join("objects");
        let index_file = temp.0.join("index.json");
        fs::create_dir(&extracted_root).unwrap();
        fs::create_dir_all(extracted_root.join("assets/api_test/test/nested")).unwrap();
        fs::create_dir_all(extracted_root.join("data/api_data/test")).unwrap();
        fs::write(
            extracted_root.join("assets/api_test/test/one.txt"),
            b"client one",
        )
        .unwrap();
        fs::write(
            extracted_root.join("assets/api_test/test/nested/two.txt"),
            b"client two",
        )
        .unwrap();
        fs::write(
            extracted_root.join("data/api_data/test/server.txt"),
            b"server data",
        )
        .unwrap();
        fs::create_dir(&objects_root).unwrap();
        fs::write(&index_file, br#"{"objects":{}}"#).unwrap();
        fs::write(extracted_root.join("root.txt"), b"vanilla root").unwrap();
        fs::write(
            extracted_root.join("pack.mcmeta"),
            br#"{"test":{"value":"vanilla"},"other":{"value":"other"}}"#,
        )
        .unwrap();
        let pack = VanillaPackResources::new(
            location,
            extracted_root,
            &index_file,
            &objects_root,
            Map::new(),
        )
        .unwrap();
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
        tests::test_get_root_resource(&pack, &["root.txt"], b"vanilla root");
    }

    #[test]
    fn test_get_root_resource_missing() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_root_resource_missing(&pack, &["missing.txt"]);
    }

    #[test]
    fn test_metadata_section_json() {
        let (_temp, pack) = fixture(default_location());
        tests::test_metadata_section_json(&pack, "test", Some(json!({"value": "vanilla"})));
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
            id: "vanilla-location".into(),
            title: Component::translate("location.vanilla.title", Vec::new()),
            source: PackSource::feature(),
            known_pack_info: Some(KnownPack {
                namespace: "location_test".into(),
                id: "vanilla_known".into(),
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
        tests::test_get_metadata_section(&pack, &section, Some("vanilla".into()));
    }

    #[test]
    fn test_get_metadata_section_missing() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_metadata_section_missing(&pack, "missing");
    }

    #[test]
    fn test_get_metadata_section_null() {
        let (temp, pack) = fixture(default_location());
        fs::write(temp.0.join("extracted/pack.mcmeta"), br#"{"null":null}"#).unwrap();
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
            &HashSet::from(["api_test".into(), "api_data".into()]),
        );
    }

    #[test]
    fn test_get_namespaces_server() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_namespaces(
            &pack,
            PackType::ServerData,
            &HashSet::from(["api_test".into(), "api_data".into()]),
        );
    }

    #[test]
    fn test_pack_id() {
        let (_temp, pack) = fixture(default_location());
        tests::test_pack_id(&pack, "vanilla-api");
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
            id: "vanilla_known".into(),
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
    use std::{fs, io};

    use serde_json::Map;

    use super::VanillaResourcesSupplier;
    use crate::chat_component::Component;
    use crate::resources::packs::{
        FeatureFlagSet, PackCompat, PackLocationInfo, PackMetadata, PackSource, ResourcesSupplier,
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> io::Result<Self> {
            for attempt in 0.. {
                let path = std::env::temp_dir().join(format!(
                    "pomme-vanilla-supplier-{}-{attempt}",
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

    fn fixture() -> (TempDir, VanillaResourcesSupplier) {
        let temp = TempDir::new().unwrap();
        let extracted_root = temp.0.join("extracted");
        let objects_root = temp.0.join("objects");
        let index_file = temp.0.join("index.json");
        fs::create_dir(&extracted_root).unwrap();
        fs::create_dir(&objects_root).unwrap();
        fs::write(&index_file, br#"{"objects":{}}"#).unwrap();
        let supplier =
            VanillaResourcesSupplier::new(extracted_root, &index_file, &objects_root, Map::new())
                .unwrap();
        (temp, supplier)
    }

    fn location() -> PackLocationInfo {
        PackLocationInfo {
            id: "vanilla-supplier-api".into(),
            title: Component::text("Supplier API test"),
            source: PackSource::default(),
            known_pack_info: None,
        }
    }

    #[test]
    fn test_open_metadata() {
        let (_temp, supplier) = fixture();
        let location = location();
        <dyn ResourcesSupplier>::test_open_metadata(&supplier, &location, "vanilla-supplier-api");
    }

    #[test]
    fn test_open_resources() {
        let (_temp, supplier) = fixture();
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
            &["vanilla-supplier-api", "vanilla-supplier-api"],
        );
    }
}

#[cfg(test)]
mod path_validation {
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::{fs, io};

    use serde_json::{Map, json};

    use super::test_fixture::default_location;
    use super::{VanillaPackResources, VanillaResourcesSupplier};
    use crate::chat_component::Component;
    use crate::resources::pack_resources::{
        PackResources, PackType, path_validation as assertions,
    };
    use crate::resources::packs::{FeatureFlagSet, PackCompat, PackMetadata, ResourcesSupplier};
    use crate::resources::resource::ResourceId;

    const METADATA: &[u8] = br#"{"pack":{"pack_format":47,"description":"indexed"}}"#;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> io::Result<Self> {
            for attempt in 0.. {
                let path = std::env::temp_dir().join(format!(
                    "pomme-vanilla-validation-{}-{attempt}",
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

    // The trusted installation owns both pack/ and objects/, not pack/ alone.
    fn fixture() -> (TempDir, VanillaPackResources, Rc<dyn PackResources>) {
        let temp = TempDir::new().unwrap();
        for directory in [
            "pack/assets/test/valid",
            "pack/data/test/valid",
            "pack-evil",
            "outside",
            "objects",
        ] {
            fs::create_dir_all(temp.0.join(directory)).unwrap();
        }
        for domain in ["assets", "data"] {
            fs::write(
                temp.0
                    .join(format!("pack/{domain}/test/valid/extracted.txt")),
                b"extracted",
            )
            .unwrap();
        }
        fs::write(temp.0.join("pack/root.txt"), b"extracted root").unwrap();
        fs::write(temp.0.join("pack/pack.mcmeta"), METADATA).unwrap();
        fs::write(temp.0.join("pack-evil/sentinel.txt"), b"sibling sentinel").unwrap();
        fs::write(temp.0.join("outside/sentinel.txt"), b"outside sentinel").unwrap();
        let objects_dir = temp.0.join("objects");
        let mut objects = Map::new();
        for (key, bytes) in [
            ("test/valid/indexed.txt", b"indexed".as_slice()),
            ("pack.mcmeta", METADATA),
        ] {
            let hash = sha1_smol::Sha1::from(bytes).digest().to_string();
            fs::create_dir_all(objects_dir.join(&hash[..2])).unwrap();
            fs::write(objects_dir.join(&hash[..2]).join(&hash), bytes).unwrap();
            objects.insert(key.into(), json!({"hash": hash, "size": bytes.len()}));
        }
        let index_file = temp.0.join("index.json");
        fs::write(
            &index_file,
            serde_json::to_vec(&json!({"objects": objects})).unwrap(),
        )
        .unwrap();
        let location = default_location();
        let full = VanillaPackResources::new(
            location.clone(),
            temp.0.join("pack"),
            &index_file,
            &objects_dir,
            Map::new(),
        )
        .unwrap();
        let supplier = VanillaResourcesSupplier::new(
            temp.0.join("pack"),
            &index_file,
            &objects_dir,
            Map::new(),
        )
        .unwrap();
        let metadata = PackMetadata {
            description: Component::text("fixture"),
            compatibility: PackCompat::Compatible,
            requested_features: FeatureFlagSet::empty(),
            overlays: Vec::new(),
        };
        let layers = supplier.open_resources(&location, &metadata).unwrap();
        assert_eq!(layers.len(), 2);
        (temp, full, layers[1].clone())
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

    fn invalid_roots() -> [&'static [&'static str]; 5] {
        [
            &["..", "outside", "sentinel.txt"],
            &["/outside/sentinel.txt"],
            &["outside/sentinel.txt"],
            &[r"outside\sentinel.txt"],
            &["C:/outside/sentinel.txt"],
        ]
    }

    fn invalid_queries(absolute: &str) -> [(&str, &str); 5] {
        [
            ("test", "../../../outside"),
            ("test", "../../../pack-evil"),
            ("test", "/outside"),
            ("test", absolute),
            ("..", ""),
        ]
    }

    #[test]
    fn full_get_resource_rejects_escape() {
        let (temp, full, _index) = fixture();
        let extracted = ResourceId::from_parts("test", "valid/extracted.txt").unwrap();
        let indexed = ResourceId::from_parts("test", "valid/indexed.txt").unwrap();
        assertions::assert_get_resource_invalid_input(
            &full,
            PackType::ClientResources,
            &[(&extracted, b"extracted"), (&indexed, b"indexed")],
            &invalid_ids(&temp),
        );
        assertions::assert_get_resource_invalid_input(
            &full,
            PackType::ServerData,
            &[(&extracted, b"extracted")],
            &invalid_ids(&temp),
        );
    }

    #[test]
    fn index_get_resource_rejects_escape() {
        let (temp, _full, index) = fixture();
        let indexed = ResourceId::from_parts("test", "valid/indexed.txt").unwrap();
        assertions::assert_get_resource_invalid_input(
            index.as_ref(),
            PackType::ClientResources,
            &[(&indexed, b"indexed")],
            &invalid_ids(&temp),
        );
    }

    #[test]
    fn full_get_root_resource_rejects_escape() {
        let (temp, full, _index) = fixture();
        let absolute = temp.0.join("outside/sentinel.txt");
        let absolute_row = [absolute.to_str().unwrap()];
        let mut invalid = invalid_roots().to_vec();
        invalid.push(&absolute_row);
        assertions::assert_get_root_resource_invalid_input(
            &full,
            &[
                (&["pack.mcmeta"], METADATA),
                (&["root.txt"], b"extracted root"),
            ],
            &invalid,
        );
    }

    #[test]
    fn index_get_root_resource_rejects_escape() {
        let (temp, _full, index) = fixture();
        let absolute = temp.0.join("outside/sentinel.txt");
        let absolute_row = [absolute.to_str().unwrap()];
        let mut invalid = invalid_roots().to_vec();
        invalid.push(&absolute_row);
        assertions::assert_get_root_resource_invalid_input(
            index.as_ref(),
            &[(&["pack.mcmeta"], METADATA)],
            &invalid,
        );
    }

    #[test]
    fn full_list_resources_rejects_escape() {
        let (temp, full, _index) = fixture();
        let absolute = temp.0.join("outside");
        let extracted = (
            ResourceId::from_parts("test", "valid/extracted.txt").unwrap(),
            b"extracted".to_vec(),
        );
        let indexed = (
            ResourceId::from_parts("test", "valid/indexed.txt").unwrap(),
            b"indexed".to_vec(),
        );
        assertions::assert_list_resources_invalid_input(
            &full,
            PackType::ClientResources,
            "test",
            "valid",
            &[extracted.clone(), indexed],
            &invalid_queries(absolute.to_str().unwrap()),
        );
        assertions::assert_list_resources_invalid_input(
            &full,
            PackType::ServerData,
            "test",
            "valid",
            &[extracted],
            &invalid_queries(absolute.to_str().unwrap()),
        );
    }

    #[test]
    fn index_list_resources_rejects_escape() {
        let (temp, _full, index) = fixture();
        let absolute = temp.0.join("outside");
        assertions::assert_list_resources_invalid_input(
            index.as_ref(),
            PackType::ClientResources,
            "test",
            "valid",
            &[(
                ResourceId::from_parts("test", "valid/indexed.txt").unwrap(),
                b"indexed".to_vec(),
            )],
            &invalid_queries(absolute.to_str().unwrap()),
        );
    }
}
