//! ZIP-backed resources following Vanilla's FilePackResources.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use self_cell::{MutBorrow, self_cell};
use serde_json::{Map, Value};
use zip::ZipArchive;

use super::path::read_metadata;
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
        let Some(opened) = self.access.get_or_open() else {
            return Ok(None);
        };
        let mut name = self.add_prefix(path);
        // Java ZipFile.getEntry also tries a trailing slash for directories.
        // Exact lookup allows them; enumeration below excludes directories.
        // The raw lookup is a hash probe; the decoded one scans every entry on
        // a miss, and misses are most lookups in a pack layered over vanilla.
        let mut index = opened.archive.index_for_name_raw(name.as_bytes());
        if index.is_none() && !name.ends_with('/') {
            name.push('/');
            index = opened.archive.index_for_name_raw(name.as_bytes());
        }
        Ok(index.map(|index| entry_supplier(opened, index)))
    }

    fn load_metadata(&self) -> io::Result<Map<String, Value>> {
        read_metadata(
            self.get_root_resource(&[PACK_META])?,
            &self.access.path.join(self.add_prefix(PACK_META)),
        )
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
            opened.closed.store(true, Ordering::Release);
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
        let Some(opened) = self.access.get_or_open() else {
            return Ok(());
        };
        let root = self.add_prefix(&format!("{}/{namespace}/", pack_type.directory()));
        let prefix = format!("{root}{directory}/");
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
            match ResourceId::from_parts(namespace, path) {
                Ok(id) => output(id, entry_supplier(opened.clone(), index)),
                Err(error) => {
                    tracing::warn!(path, %error, "Invalid path in ZIP pack; ignoring");
                }
            }
        }
        Ok(())
    }

    fn get_namespaces(&self, pack_type: PackType) -> io::Result<HashSet<String>> {
        let Some(opened) = self.access.get_or_open() else {
            return Ok(HashSet::new());
        };
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
    Opened(Arc<OpenedZip>),
    Failed,
}

impl SharedZipFileAccess {
    /// Vanilla `SharedZipFileAccess.getOrCreateZipFile`: a failed open is
    /// logged once and latched, and the pack then has nothing in it.
    fn get_or_open(&self) -> Option<Arc<OpenedZip>> {
        let mut state = self.state.borrow_mut();
        match &*state {
            ZipState::Opened(opened) => return Some(opened.clone()),
            ZipState::Failed => return None,
            ZipState::Unopened => {}
        }
        match open_archive(&self.path) {
            Ok(archive) => {
                let opened = Arc::new(OpenedZip {
                    archive,
                    closed: AtomicBool::new(false),
                });
                *state = ZipState::Opened(opened.clone());
                Some(opened)
            }
            Err(error) => {
                tracing::error!(path = %self.path.display(), %error, "Failed to open pack");
                *state = ZipState::Failed;
                None
            }
        }
    }
}

struct OpenedZip {
    archive: Archive,
    closed: AtomicBool,
}

impl OpenedZip {
    fn ensure_open(&self) -> io::Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(io::Error::other("ZIP pack is closed"));
        }
        Ok(())
    }
}

fn entry_supplier(opened: Arc<OpenedZip>, index: usize) -> IoSupplier<ReadStream> {
    Arc::new(move || {
        opened.ensure_open()?;
        Ok(Box::new(FilePackStream {
            entry: open_entry(&opened.archive, index)?,
            opened: opened.clone(),
        }) as ReadStream)
    })
}

struct FilePackStream {
    entry: OwnedZipEntry,
    opened: Arc<OpenedZip>,
}

impl Read for FilePackStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.opened.ensure_open()?;
        self.entry.read(buffer)
    }
}

#[cfg(test)]
mod test_fixture {
    use std::fs::File;
    use std::io::Write;

    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::FilePackResources;
    use crate::resources::packs::PackLocationInfo;
    use crate::resources::test_dir::TempDir;

    pub(super) fn default_location() -> PackLocationInfo {
        PackLocationInfo::test("file-api")
    }

    pub(super) fn fixture(location: PackLocationInfo) -> (TempDir, FilePackResources) {
        let temp = TempDir::new().unwrap();
        let archive_path = temp.0.join("pack.zip");
        let mut archive = ZipWriter::new(File::create(&archive_path).unwrap());
        archive
            .start_file("root.txt", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"file root").unwrap();
        archive
            .start_file("pack.mcmeta", SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(br#"{"test":{"value":"file"},"other":{"value":"other"}}"#)
            .unwrap();
        archive
            .start_file("assets/api_test/test/one.txt", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"client one").unwrap();
        archive
            .start_file(
                "assets/api_test/test/nested/two.txt",
                SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"client two").unwrap();
        archive
            .start_file(
                "data/api_data/test/server.txt",
                SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"server data").unwrap();
        archive.finish().unwrap();
        let pack = FilePackResources::new(location, archive_path);
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
        tests::test_get_root_resource(&pack, &["root.txt"], b"file root");
    }

    #[test]
    fn test_get_root_resource_missing() {
        let (_temp, pack) = fixture(default_location());
        tests::test_get_root_resource_missing(&pack, &["missing.txt"]);
    }

    #[test]
    fn test_metadata_section_json() {
        let (_temp, pack) = fixture(default_location());
        tests::test_metadata_section_json(&pack, "test", Some(json!({"value": "file"})));
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
            id: "file-location".into(),
            title: Component::translate("location.file.title", Vec::new()),
            known_pack_info: Some(pomme_protocol::KnownPack {
                namespace: "location_test".into(),
                id: "file_known".into(),
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
        // Vanilla FilePackResources uses root + directory + "/", even when empty.
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
        tests::test_pack_id(&pack, "file-api");
    }
}

#[cfg(test)]
mod resources_supplier_tests {
    use std::path::PathBuf;

    use super::FileResourcesSupplier;
    use crate::resources::packs::ResourcesSupplier;

    #[test]
    fn test_open() {
        let supplier = FileResourcesSupplier::new(PathBuf::from("unused-supplier.zip"));
        <dyn ResourcesSupplier>::test_open(&supplier, 1);
    }
}

#[cfg(test)]
mod failed_open_tests {
    use std::fs;

    use super::FilePackResources;
    use super::test_fixture::default_location;
    use crate::resources::pack_resources::{PackMetadataResources, PackResources, PackType};
    use crate::resources::resource_id;
    use crate::resources::test_dir::TempDir;

    #[test]
    fn an_unreadable_archive_is_an_empty_pack() {
        let temp = TempDir::new().unwrap();
        let path = temp.0.join("broken.zip");
        fs::write(&path, b"not a zip").unwrap();
        let pack = FilePackResources::new(default_location(), path);
        for _ in 0..2 {
            let id = resource_id!("minecraft:a.txt");
            assert!(
                pack.get_resource(PackType::ClientResources, &id)
                    .unwrap()
                    .is_none()
            );
            assert!(
                pack.get_namespaces(PackType::ClientResources)
                    .unwrap()
                    .is_empty()
            );
            assert!(pack.metadata_section_json("pack").unwrap().is_none());
        }
    }
}
