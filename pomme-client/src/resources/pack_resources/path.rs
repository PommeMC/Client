//! Directory-backed resources following Vanilla's PathPackResources.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

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
    Rc::new(move || File::open(&path).map(|file| Box::new(file) as ReadStream))
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
    let mut id = String::from(namespace);
    id.push(':');
    for (index, component) in relative.iter().enumerate() {
        let Some(part) = component.to_str() else {
            tracing::warn!(path = %path.display(), "Non-UTF-8 resource path; ignoring");
            return Ok(());
        };
        if index > 0 {
            id.push('/');
        }
        id.push_str(part);
    }
    match ResourceId::new(&id) {
        Ok(id) => output(id, file_supplier(path)),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "Invalid resource path; ignoring")
        }
    }
    Ok(())
}
