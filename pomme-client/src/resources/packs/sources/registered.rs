//! Explicit physical definitions, independent of discovery and enablement.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::symlinks::prepare_pack_path;
use super::{RepositorySource, SymlinkPolicy, path_error};
use crate::resources::pack_resources::{FileResourcesSupplier, PackType, PathResourcesSupplier};
use crate::resources::packs::{
    Pack, PackEnablementConfig, PackFormat, PackId, PackLocationInfo, ResourcesSupplier,
};

pub(crate) enum PhysicalPack {
    Directory(PathBuf),
    Zip(PathBuf),
}

impl PhysicalPack {
    fn path(&self) -> &Path {
        match self {
            Self::Directory(path) | Self::Zip(path) => path,
        }
    }

    fn prepare(&self, policy: &SymlinkPolicy) -> io::Result<Rc<dyn ResourcesSupplier>> {
        let kind = std::fs::symlink_metadata(self.path())?.file_type();
        let (path, kind) = prepare_pack_path(self.path(), kind, policy)?;
        match self {
            Self::Directory(_) if kind.is_dir() => Ok(Rc::new(PathResourcesSupplier::new(path))),
            Self::Zip(_) if kind.is_file() => Ok(Rc::new(FileResourcesSupplier::new(path))),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "pack root does not match its declared storage type",
            )),
        }
    }
}

struct Definition {
    id: PackId,
    location: PackLocationInfo,
    physical: PhysicalPack,
    enablement: PackEnablementConfig,
}

struct RegisteredState {
    definitions: Vec<Definition>,
    pack_type: PackType,
    target: Option<PackFormat>,
    allowed_symlinks: SymlinkPolicy,
}

/// Retain a concrete clone when boxing a source into the repository.
#[derive(Clone)]
pub(crate) struct RegisteredRepositorySource {
    state: Rc<RefCell<RegisteredState>>,
}

impl RegisteredRepositorySource {
    pub(crate) fn new(pack_type: PackType, target: Option<PackFormat>) -> Self {
        Self {
            state: Rc::new(RefCell::new(RegisteredState {
                definitions: Vec::new(),
                pack_type,
                target,
                allowed_symlinks: SymlinkPolicy::default(),
            })),
        }
    }

    /// Definition only, no I/O. Duplicate paths receive distinct opaque IDs.
    /// Validation is deferred to reload, which must succeed before enabling ID.
    pub(crate) fn register(
        &self,
        location: PackLocationInfo,
        physical: PhysicalPack,
        enablement: PackEnablementConfig,
    ) -> PackId {
        let id = PackId::allocate();
        self.state.borrow_mut().definitions.push(Definition {
            id: id.clone(),
            location,
            physical,
            enablement,
        });
        id
    }

    /// Forget only the definition: no deletion or closure of existing views.
    /// A later failed reload leaves the repository's old active catalog intact;
    /// it does not restore this removed definition.
    pub(crate) fn unregister(&self, id: &PackId) -> bool {
        let mut state = self.state.borrow_mut();
        let before = state.definitions.len();
        state.definitions.retain(|definition| &definition.id != id);
        before != state.definitions.len()
    }

    pub(crate) fn set_target_format(&self, target: Option<PackFormat>) {
        self.state.borrow_mut().target = target;
    }

    pub(crate) fn set_symlink_policy(&self, policy: SymlinkPolicy) {
        self.state.borrow_mut().allowed_symlinks = policy;
    }
}

impl RepositorySource for RegisteredRepositorySource {
    fn load_packs(&mut self) -> io::Result<Vec<Pack>> {
        let state = self.state.borrow();
        state
            .definitions
            .iter()
            .map(|definition| {
                let load = || -> io::Result<Pack> {
                    let resources = definition.physical.prepare(&state.allowed_symlinks)?;
                    let metadata = Pack::read_metadata(
                        &definition.location,
                        resources.as_ref(),
                        state.pack_type,
                        state.target,
                    )?
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "missing pack metadata section")
                    })?;
                    Ok(Pack::with_id(
                        definition.id.clone(),
                        definition.location.clone(),
                        metadata,
                        definition.enablement,
                        resources,
                    ))
                };
                // Explicit definitions are all-or-error, never silently dropped.
                load().map_err(|error| {
                    path_error(
                        &format!(
                            "load registered pack {:?} ({:?})",
                            definition.id, definition.location.id
                        ),
                        definition.physical.path(),
                        error,
                    )
                })
            })
            .collect()
    }
}
