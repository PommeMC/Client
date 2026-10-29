use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use self_cell::{MutBorrow, self_cell};
use zip::ZipArchive;

use super::{
    PackResourceProvider, ReadStream, ResourceOrigin, ThreadSafePackResourceProvider,
    enumeration_error, id_from_key, lookup_error, matches_prefix, overlay_directory,
    valid_namespace, validate_query,
};
use crate::resources::resource::{ResourceContent, ResourceError, ResourceId};

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

type ZipEntry<'a> = zip::read::ZipFile<'a, SharedFileCursor>;

// MutBorrow grants the builder its one mutable archive borrow. self_cell
// keeps that archive stationary and drops the borrowed entry before its owner.
self_cell! {
    struct OwnedZipEntry {
        owner: MutBorrow<ZipArchive<SharedFileCursor>>,

        #[not_covariant]
        dependent: ZipEntry,
    }
}

impl Read for OwnedZipEntry {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.with_dependent_mut(|_, entry| entry.read(buffer))
    }
}

/// Resources in an opened ZIP with a parsed central directory. Handles
/// retain this source; streams retain their archive and the opened file
/// independently. Replacing/deleting the pathname does not reopen it. In-place
/// edits are not protected: this is an opened source view, not an immutable
/// byte snapshot.
#[derive(Clone)]
pub(in crate::resources::packs) struct ZipPackResourceProvider {
    origin: ResourceOrigin,
    archive: Arc<ZipArchive<SharedFileCursor>>,
    prefix: String,
}

impl ZipPackResourceProvider {
    pub(in crate::resources::packs) fn open(
        path: PathBuf,
        origin: ResourceOrigin,
    ) -> Result<Self, ResourceError> {
        let archive = (|| -> io::Result<_> {
            let cursor = SharedFileCursor {
                file: Arc::new(Mutex::new(File::open(path)?)),
                position: 0,
            };
            Ok(ZipArchive::new(cursor)?)
        })()
        .map_err(|source| ResourceError::Io {
            operation: "open ZIP source",
            id: None,
            origin: origin.clone(),
            source,
        })?;
        Ok(Self {
            origin,
            archive: Arc::new(archive),
            prefix: "assets/".to_owned(),
        })
    }

    pub(in crate::resources::packs) fn overlay(
        &self,
        directory: &str,
    ) -> Result<Self, ResourceError> {
        let directory = overlay_directory(directory)
            .map_err(|source| enumeration_error(&self.origin, "open ZIP overlay", source))?;
        let prefix = if directory.is_empty() {
            "assets/".to_owned()
        } else {
            format!("{directory}/assets/")
        };
        let view = Self {
            origin: self.origin.clone(),
            archive: self.archive.clone(),
            prefix,
        };
        view.check_pack_root()?;
        Ok(view)
    }

    pub(in crate::resources::packs) fn check_pack_root(&self) -> Result<(), ResourceError> {
        let check = || -> io::Result<()> {
            let mut path = String::new();
            for component in self.prefix.trim_end_matches('/').split('/') {
                path.push_str(component);
                for name in [&path, &format!("{path}/")] {
                    if let Some(index) = self.archive.index_for_name(name) {
                        let entry = self.archive.by_index_data(index)?;
                        if !entry.is_dir()
                            || !matches!(entry.unix_mode().unwrap_or(0) & 0o170000, 0 | 0o040000)
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("ZIP pack root is not a directory: {name}"),
                            ));
                        }
                    }
                }
                path.push('/');
            }
            Ok(())
        };
        check().map_err(|source| enumeration_error(&self.origin, "open ZIP pack root", source))
    }

    fn entry_name(&self, id: &ResourceId) -> String {
        format!("{}{}/{}", self.prefix, id.namespace(), id.path())
    }

    fn open_entry(&self, name: &str) -> io::Result<Option<OwnedZipEntry>> {
        let archive = self.archive.as_ref();
        let Some(index) = archive.index_for_name(name) else {
            // Check explicit directory entries through the cached name index.
            // Implicit prefixes alone are not entries; do not scan descendants.
            if archive.index_for_name(&format!("{name}/")).is_some()
                || archive.index_for_name(&format!("{name}\\")).is_some()
            {
                return Err(Self::non_file(name));
            }
            return Ok(None);
        };
        let metadata = archive.by_index_data(index)?;
        Self::check_file(name, metadata.is_dir(), metadata.unix_mode())?;
        // No content is decoded here. zip owns codec/integrity checks during
        // Read. Archive-borrowed entries do not drain remaining bytes on drop.
        // Clones share central-directory metadata and the file, but own cursors.
        OwnedZipEntry::try_new(MutBorrow::new(archive.clone()), |owner| {
            owner.borrow_mut().by_index(index)
        })
        .map(Some)
        .map_err(Into::into)
    }

    fn non_file(name: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ZIP resource is not a file: {name}"),
        )
    }

    fn check_file(name: &str, is_dir: bool, mode: Option<u32>) -> io::Result<()> {
        // Unspecified file types are common in ZIPs; otherwise require S_IFREG.
        if is_dir || !matches!(mode.unwrap_or(0) & 0o170000, 0 | 0o100000) {
            return Err(Self::non_file(name));
        }
        Ok(())
    }
}

impl PackResourceProvider for ZipPackResourceProvider {
    fn thread_safe_source(&self) -> io::Result<Arc<dyn ThreadSafePackResourceProvider>> {
        Ok(Arc::new(self.clone()))
    }

    fn origin(&self) -> &ResourceOrigin {
        &self.origin
    }

    fn get_resource(
        self: Rc<Self>,
        id: &ResourceId,
    ) -> Result<Option<ResourceContent>, ResourceError> {
        let present = self
            .open_entry(&self.entry_name(id))
            .map_err(|source| lookup_error(id, &self.origin, source))?
            .is_some();
        Ok(present.then(|| ResourceContent::new(id.clone(), self)))
    }

    fn open(&self, id: &ResourceId) -> io::Result<ReadStream> {
        self.open_entry(&self.entry_name(id))?
            .map(|entry| Box::new(entry) as ReadStream)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "ZIP resource is absent"))
    }

    fn namespaces(&self) -> Result<BTreeSet<String>, ResourceError> {
        let enumerate = || -> io::Result<_> {
            let mut namespaces = BTreeSet::new();
            for name in self.archive.file_names() {
                let name = name?;
                if let Some(key) = name.strip_prefix(&self.prefix)
                    && let Some((namespace, _)) = key.split_once('/')
                    && valid_namespace(namespace)
                {
                    namespaces.insert(namespace.to_owned());
                }
            }
            Ok(namespaces)
        };
        enumerate()
            .map_err(|source| enumeration_error(&self.origin, "enumerate namespaces", source))
    }

    fn list_resources(
        &self,
        namespace: &str,
        prefix: &str,
    ) -> Result<BTreeSet<ResourceId>, ResourceError> {
        validate_query(Some(namespace), prefix)?;
        let enumerate = || -> io::Result<_> {
            let mut resources = BTreeSet::new();
            for name in self.archive.file_names() {
                let name = name?;
                let Some(id) = name.strip_prefix(&self.prefix).and_then(id_from_key) else {
                    continue;
                };
                if id.namespace() != namespace || !matches_prefix(id.path(), prefix) {
                    continue;
                }
                // Use the same name index as exact lookup, including duplicate
                // names. Only central-directory metadata is read, not contents.
                let Some(index) = self.archive.index_for_name(&name) else {
                    continue;
                };
                let metadata = self.archive.by_index_data(index)?;
                if metadata.is_dir() {
                    continue;
                }
                // Symlinks and special entries are errors, not an opportunity
                // to silently fall back to a lower-priority resource.
                Self::check_file(&name, false, metadata.unix_mode())?;
                resources.insert(id);
            }
            Ok(resources)
        };
        enumerate().map_err(|source| enumeration_error(&self.origin, "list resources", source))
    }
}
