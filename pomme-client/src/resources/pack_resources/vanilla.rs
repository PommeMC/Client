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
                Some((namespace, path)) => match ResourceId::new(&format!("{namespace}:{path}")) {
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
