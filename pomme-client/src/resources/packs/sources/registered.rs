//! Explicit physical definitions, independent of discovery and enablement.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::symlinks::prepare_pack_path;
use super::{RepositorySource, SymlinkPolicy};
use crate::resources::pack_resources::{PackType, PathResourcesSupplier};
use crate::resources::packs::{
    Pack, PackEnablementConfig, PackFormat, PackId, PackLocationInfo, ResourcesSupplier,
};

pub(crate) enum PhysicalPack {
    Directory(PathBuf),
}

impl PhysicalPack {
    fn path(&self) -> &Path {
        match self {
            Self::Directory(path) => path,
        }
    }

    fn prepare(&self, policy: &SymlinkPolicy) -> io::Result<Rc<dyn ResourcesSupplier>> {
        let kind = std::fs::symlink_metadata(self.path())?.file_type();
        let (path, kind) = prepare_pack_path(self.path(), kind, policy)?;
        if !kind.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "pack root is not a directory",
            ));
        }
        Ok(Rc::new(PathResourcesSupplier::new(path)))
    }
}

struct Definition {
    id: PackId,
    location: PackLocationInfo,
    physical: PhysicalPack,
    enablement: PackEnablementConfig,
}

/// A detached definition, retaining its identity and source-local position.
pub(crate) struct RemovedRegistration {
    definition: Definition,
    position: usize,
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
        self.take_registration(id).is_some()
    }

    /// Temporarily remove one definition without changing retained views.
    pub(crate) fn take_registration(&self, id: &PackId) -> Option<RemovedRegistration> {
        let mut state = self.state.borrow_mut();
        let position = state
            .definitions
            .iter()
            .position(|definition| &definition.id == id)?;
        Some(RemovedRegistration {
            definition: state.definitions.remove(position),
            position,
        })
    }

    /// Restore to the same source without reallocating an ID or reading
    /// metadata.
    pub(crate) fn restore_registration(&self, removed: RemovedRegistration) {
        let mut state = self.state.borrow_mut();
        let position = removed.position.min(state.definitions.len());
        state.definitions.insert(position, removed.definition);
    }

    pub(crate) fn set_target_format(&self, target: Option<PackFormat>) {
        self.state.borrow_mut().target = target;
    }

    pub(crate) fn set_symlink_policy(&self, policy: SymlinkPolicy) {
        self.state.borrow_mut().allowed_symlinks = policy;
    }
}

impl RepositorySource for RegisteredRepositorySource {
    /// A definition that fails to load is logged and left out, as vanilla's
    /// `Pack.readMetaAndCreate` returns null for a pack it can't read.
    fn load_packs(&mut self) -> io::Result<Vec<Pack>> {
        let state = self.state.borrow();
        Ok(state
            .definitions
            .iter()
            .filter_map(|definition| {
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
                load()
                    .inspect_err(|error| {
                        tracing::warn!(
                            pack = %definition.location.id,
                            path = %definition.physical.path().display(),
                            %error,
                            "Failed to load registered pack; skipping"
                        );
                    })
                    .ok()
            })
            .collect())
    }
}

#[cfg(test)]
mod repository_source_tests {

    use std::fs;

    use super::super::tests;
    use super::{PhysicalPack, RegisteredRepositorySource};
    use crate::resources::pack_resources::PackType;
    use crate::resources::packs::{PackEnablementConfig, PackLocationInfo};
    use crate::resources::test_dir::TempDir;

    const METADATA: &[u8] = br#"{"pack":{"pack_format":47,"description":"fixture"}}"#;

    fn fixture() -> (TempDir, RegisteredRepositorySource) {
        let temp = TempDir::new().unwrap();
        let source = RegisteredRepositorySource::new(PackType::ClientResources, None);
        (temp, source)
    }

    #[test]
    fn test_load_packs_empty() {
        let (_temp, mut source) = fixture();
        tests::test_load_packs_empty(&mut source);
    }

    #[test]
    fn test_load_packs_directory() {
        let (temp, mut source) = fixture();
        let path = temp.0.join("directory-pack");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("pack.mcmeta"), METADATA).unwrap();
        source.register(
            PackLocationInfo::test("registered-directory"),
            PhysicalPack::Directory(path),
            PackEnablementConfig::DISCOVERED,
        );
        tests::test_load_packs(&mut source, &["registered-directory"]);
    }
}

// Windows symlink admission needs a separately enabled symlink-capable
// environment.
#[cfg(all(test, unix))]
mod path_validation {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    use super::super::path_validation::assert_load_packs_catalog;
    use super::{PhysicalPack, RegisteredRepositorySource, RepositorySource};
    use crate::resources::pack_resources::PackType;
    use crate::resources::packs::{PackEnablementConfig, PackLocationInfo};
    use crate::resources::test_dir::TempDir;

    const METADATA: &[u8] = br#"{"pack":{"pack_format":47,"description":"fixture"}}"#;

    fn source(path: PathBuf, id: &str) -> RegisteredRepositorySource {
        let source = RegisteredRepositorySource::new(PackType::ClientResources, None);
        source.register(
            PackLocationInfo::test(id),
            PhysicalPack::Directory(path),
            PackEnablementConfig::DISCOVERED,
        );
        source
    }

    #[test]
    fn load_packs_skips_forbidden_root_symlink() {
        let temp = TempDir::new().unwrap();
        let good = temp.0.join("pack");
        let outside = temp.0.join("outside");
        for pack in [&good, &outside] {
            fs::create_dir(pack).unwrap();
            fs::write(pack.join("pack.mcmeta"), METADATA).unwrap();
        }
        fs::write(outside.join("sentinel.txt"), b"outside sentinel").unwrap();
        let link = temp.0.join("pack-evil");
        symlink(outside, &link).unwrap();
        assert_eq!(fs::read(link.join("pack.mcmeta")).unwrap(), METADATA);
        let mut forbidden = source(link, "forbidden");
        assert!(forbidden.load_packs().unwrap().is_empty());
        let mut clean = source(good, "good");
        assert_load_packs_catalog(&mut clean, &["good"]);
    }
}
