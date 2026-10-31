//! ZIP-backed resources following Vanilla's FilePackResources.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use self_cell::{MutBorrow, self_cell};
use serde_json::{Map, Value};
use zip::ZipArchive;

use super::{
    IoSupplier, OverlayedPackResources, PACK_META, PackMetadataResources, PackResources, PackType,
    ResourceOutput,
};
use crate::resources::packs::{PackLocationInfo, PackMetadata, ResourcesSupplier};
use crate::resources::resource::{ReadStream, ResourceId, resource_namespace_chars};

/// Setup owns validation of the source and overlay names. ZIP paths are literal
/// keys, never filesystem paths to canonicalize, probe or extract. All views
/// share lazy archive access, but each has its own lazy metadata snapshot.
pub(crate) struct FilePackResources {
    location: PackLocationInfo,
    access: Rc<SharedZipFileAccess>,
    prefix: String,
    metadata: RefCell<Option<Map<String, Value>>>,
}

/// A ZIP pathname, never an opened archive. Each opening owns independent lazy
/// access; only the primary and overlays of that single opening share it.
pub(crate) struct FileResourcesSupplier {
    path: PathBuf,
}

impl FileResourcesSupplier {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl ResourcesSupplier for FileResourcesSupplier {
    fn open_metadata(
        &self,
        location: &PackLocationInfo,
    ) -> io::Result<Rc<dyn PackMetadataResources>> {
        Ok(Rc::new(FilePackResources::new(
            location.clone(),
            self.path.clone(),
        )))
    }

    fn open_resources(
        &self,
        location: &PackLocationInfo,
        metadata: &PackMetadata,
    ) -> io::Result<Vec<Rc<dyn PackResources>>> {
        let primary = Rc::new(FilePackResources::new(location.clone(), self.path.clone()));
        if metadata.overlays.is_empty() {
            return Ok(vec![primary]);
        }
        let overlays = metadata
            .overlays
            .iter()
            .map(|directory| Rc::new(primary.overlay(directory.clone())) as Rc<dyn PackResources>)
            .collect();
        Ok(vec![Rc::new(OverlayedPackResources::new(
            primary, overlays,
        ))])
    }
}

impl FilePackResources {
    /// Stores the source without opening it, like Vanilla's metadata supplier.
    pub(crate) fn new(location: PackLocationInfo, path: PathBuf) -> Self {
        Self {
            location,
            access: Rc::new(SharedZipFileAccess {
                path,
                state: RefCell::new(ZipState::Unopened),
            }),
            prefix: String::new(),
            metadata: RefCell::new(None),
        }
    }

    /// A root-relative overlay, not a prefix appended to this view's prefix.
    /// As in Vanilla's FileResourcesSupplier, shares the primary's lazy access
    /// and identity. Closing any view closes the current shared generation.
    pub(crate) fn overlay(&self, prefix: String) -> Self {
        Self {
            location: self.location.clone(),
            access: self.access.clone(),
            prefix,
            metadata: RefCell::new(None),
        }
    }

    fn add_prefix(&self, path: &str) -> String {
        if self.prefix.is_empty() {
            path.to_owned()
        } else {
            format!("{}/{path}", self.prefix)
        }
    }

    fn locate(&self, path: &str) -> io::Result<Option<IoSupplier<ReadStream>>> {
        let opened = self.access.get_or_open()?;
        let mut name = self.add_prefix(path);
        // Java ZipFile.getEntry also tries a trailing slash for directories.
        // Exact lookup allows them; enumeration below excludes directories.
        let mut index = opened.archive.index_for_name(&name);
        if index.is_none() && !name.ends_with('/') {
            name.push('/');
            index = opened.archive.index_for_name(&name);
        }
        Ok(index.map(|index| entry_supplier(opened, index)))
    }

    fn load_metadata(&self) -> io::Result<Map<String, Value>> {
        let Some(supplier) = self.get_root_resource(&[PACK_META])? else {
            return Ok(Map::new());
        };
        let value: Value = serde_json::from_reader(supplier()?).map_err(|error| {
            io::Error::new(
                error.io_error_kind().unwrap_or(io::ErrorKind::InvalidData),
                format!(
                    "cannot read {} in {}: {error}",
                    self.add_prefix(PACK_META),
                    self.access.path.display()
                ),
            )
        })?;
        match value {
            Value::Object(object) => Ok(object),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} in {} must contain a JSON object",
                    self.add_prefix(PACK_META),
                    self.access.path.display()
                ),
            )),
        }
    }
}

impl PackMetadataResources for FilePackResources {
    fn location(&self) -> &PackLocationInfo {
        &self.location
    }

    fn get_root_resource(&self, path: &[&str]) -> io::Result<Option<IoSupplier<ReadStream>>> {
        // Root files, including metadata, use this view's overlay prefix too.
        self.locate(&path.join("/"))
    }

    fn metadata_section_json(&self, name: &str) -> io::Result<Option<Value>> {
        if let Some(metadata) = self.metadata.borrow().as_ref() {
            return Ok(metadata.get(name).cloned());
        }
        // Match Path: cache successful objects/absence, retry parse/read errors,
        // and preserve a present JSON null for the section decoder.
        let metadata = self.load_metadata()?;
        let section = metadata.get(name).cloned();
        *self.metadata.borrow_mut() = Some(metadata);
        Ok(section)
    }

    /// Invalidates this generation's suppliers and active streams, including
    /// those from sibling views. Later access can open a new generation; old
    /// suppliers never switch sources. Repeated close is harmless. Cached
    /// metadata and a sticky initial-open failure survive close, as in Vanilla.
    /// Rust drops file handles without reporting close errors; actual OS handle
    /// release waits for retained suppliers/streams, which remain memory-safe.
    fn close(&self) -> io::Result<()> {
        let mut state = self.access.state.borrow_mut();
        if let ZipState::Opened(opened) = &*state {
            opened.closed.set(true);
            *state = ZipState::Unopened;
        }
        Ok(())
    }
}

impl PackResources for FilePackResources {
    fn get_resource(
        &self,
        pack_type: PackType,
        id: &ResourceId,
    ) -> io::Result<Option<IoSupplier<ReadStream>>> {
        self.locate(&format!(
            "{}/{}/{}",
            pack_type.directory(),
            id.namespace(),
            id.path()
        ))
    }

    fn list_resources(
        &self,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        output: &mut ResourceOutput<'_>,
    ) -> io::Result<()> {
        if namespace.is_empty() || !resource_namespace_chars(namespace) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid resource namespace",
            ));
        }
        let opened = self.access.get_or_open()?;
        let root = self.add_prefix(&format!("{}/{namespace}/", pack_type.directory()));
        // The Rust contract permits the namespace root as an empty directory.
        // Unlike Java's literal root + directory + "/", do not double that slash.
        let prefix = if directory.is_empty() {
            root.clone()
        } else {
            format!("{root}{directory}/")
        };
        for index in 0..opened.archive.len() {
            opened.ensure_open()?;
            let entry = opened.archive.by_index_data(index)?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name()?;
            if !name.starts_with(&prefix) {
                continue;
            }
            let path = &name[root.len()..];
            match ResourceId::new(&format!("{namespace}:{path}")) {
                Ok(id) => output(id, entry_supplier(opened.clone(), index)),
                Err(error) => {
                    tracing::warn!(path, %error, "Invalid path in ZIP pack; ignoring");
                }
            }
        }
        Ok(())
    }

    fn get_namespaces(&self, pack_type: PackType) -> io::Result<HashSet<String>> {
        let opened = self.access.get_or_open()?;
        let prefix = self.add_prefix(&format!("{}/", pack_type.directory()));
        let mut namespaces = HashSet::new();
        // Vanilla includes explicit directory entries and even a bare name
        // after the type prefix, not just entries containing another slash.
        for name in opened.archive.file_names() {
            let name = name?;
            let Some(relative) = name.strip_prefix(&prefix) else {
                continue;
            };
            let namespace = relative.split('/').next().unwrap_or_default();
            if namespace.is_empty() {
                continue;
            }
            if resource_namespace_chars(namespace) {
                namespaces.insert(namespace.to_owned());
            } else {
                tracing::warn!(namespace, "Invalid namespace in ZIP pack; ignoring");
            }
        }
        Ok(namespaces)
    }
}

/// One opened file, with an independent logical position for each entry stream.
/// Seeking and reading happen under the same lock; File::try_clone alone
/// would share an OS cursor and break interleaved reads.
#[derive(Clone)]
struct SharedFileCursor {
    file: Arc<Mutex<File>>,
    position: u64,
}

impl Read for SharedFileCursor {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = {
            let mut file = self
                .file
                .lock()
                .map_err(|_| io::Error::other("ZIP source file lock poisoned"))?;
            file.seek(SeekFrom::Start(self.position))?;
            file.read(buffer)?
        };
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for SharedFileCursor {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(offset) => self
                .file
                .lock()
                .map_err(|_| io::Error::other("ZIP source file lock poisoned"))?
                .metadata()?
                .len()
                .checked_add_signed(offset),
        };
        self.position = position.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid ZIP cursor position")
        })?;
        Ok(self.position)
    }
}

type Archive = ZipArchive<SharedFileCursor>;
type ZipEntry<'a> = zip::read::ZipFile<'a, SharedFileCursor>;

// MutBorrow grants the builder its one mutable archive borrow. self_cell
// keeps that archive stationary and drops the borrowed entry before its owner.
self_cell! {
    struct OwnedZipEntry {
        owner: MutBorrow<Archive>,

        #[not_covariant]
        dependent: ZipEntry,
    }
}

impl Read for OwnedZipEntry {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.with_dependent_mut(|_, entry| entry.read(buffer))
    }
}

fn open_archive(path: &Path) -> io::Result<Archive> {
    let cursor = SharedFileCursor {
        file: Arc::new(Mutex::new(File::open(path)?)),
        position: 0,
    };
    Ok(ZipArchive::new(cursor)?)
}

/// Clones share central-directory metadata and the opened file, not cursors.
/// No resource buffering or pathname reopening; codecs and integrity errors
/// remain the ZIP reader's responsibility, including during subsequent reads.
fn open_entry(archive: &Archive, index: usize) -> io::Result<OwnedZipEntry> {
    OwnedZipEntry::try_new(MutBorrow::new(archive.clone()), |owner| {
        owner.borrow_mut().by_index(index)
    })
    .map_err(Into::into)
}

struct SharedZipFileAccess {
    path: PathBuf,
    state: RefCell<ZipState>,
}

enum ZipState {
    Unopened,
    Opened(Rc<OpenedZip>),
    Failed(Arc<io::Error>),
}

impl SharedZipFileAccess {
    fn get_or_open(&self) -> io::Result<Rc<OpenedZip>> {
        let mut state = self.state.borrow_mut();
        match &*state {
            ZipState::Opened(opened) => return Ok(opened.clone()),
            ZipState::Failed(error) => {
                return Err(io::Error::new(error.kind(), error.clone()));
            }
            ZipState::Unopened => {}
        }
        match open_archive(&self.path) {
            Ok(archive) => {
                let opened = Rc::new(OpenedZip {
                    archive,
                    closed: Cell::new(false),
                });
                *state = ZipState::Opened(opened.clone());
                Ok(opened)
            }
            Err(error) => {
                // Vanilla latches failures but returns absence. The Rust
                // contract instead propagates the retained error on every call.
                let error = Arc::new(error);
                *state = ZipState::Failed(error.clone());
                Err(io::Error::new(error.kind(), error))
            }
        }
    }
}

struct OpenedZip {
    archive: Archive,
    closed: Cell<bool>,
}

impl OpenedZip {
    fn ensure_open(&self) -> io::Result<()> {
        if self.closed.get() {
            return Err(io::Error::other("ZIP pack is closed"));
        }
        Ok(())
    }
}

fn entry_supplier(opened: Rc<OpenedZip>, index: usize) -> IoSupplier<ReadStream> {
    Rc::new(move || {
        opened.ensure_open()?;
        Ok(Box::new(FilePackStream {
            entry: open_entry(&opened.archive, index)?,
            opened: opened.clone(),
        }) as ReadStream)
    })
}

struct FilePackStream {
    entry: OwnedZipEntry,
    opened: Rc<OpenedZip>,
}

impl Read for FilePackStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.opened.ensure_open()?;
        self.entry.read(buffer)
    }
}
