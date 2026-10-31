//! Read-only lexical discovery using shared setup-time filesystem policy.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::symlinks::prepare_pack_path;
use super::{RepositorySource, SymlinkPolicy, path_error};
use crate::chat_component::Component;
use crate::resources::pack_resources::{FileResourcesSupplier, PackType, PathResourcesSupplier};
use crate::resources::packs::{
    Pack, PackEnablementConfig, PackFormat, PackId, PackLocationInfo, PackPosition, PackSource,
    ResourcesSupplier,
};

/// Box a clone in the repository and retain this handle for explicit updates.
/// Definitions change immediately; the repository catalog changes only on
/// reload.
#[derive(Clone)]
pub(crate) struct FolderRepositorySource {
    state: Rc<RefCell<FolderState>>,
}

struct FolderState {
    roots: Vec<PathBuf>,
    excluded_cache: Option<PathBuf>,
    pack_type: PackType,
    target: Option<PackFormat>,
    allowed_symlinks: SymlinkPolicy,
    discovered: HashMap<PathBuf, PackId>,
}

impl FolderRepositorySource {
    /// No I/O, directory creation, canonicalization or implicit policy loading.
    pub(crate) fn new(
        roots: Vec<PathBuf>,
        excluded_cache: Option<PathBuf>,
        pack_type: PackType,
        target: Option<PackFormat>,
    ) -> Self {
        Self {
            state: Rc::new(RefCell::new(FolderState {
                roots,
                excluded_cache,
                pack_type,
                target,
                allowed_symlinks: SymlinkPolicy::default(),
                discovered: HashMap::new(),
            })),
        }
    }

    pub(crate) fn set_paths(&self, roots: Vec<PathBuf>, excluded_cache: Option<PathBuf>) {
        let mut state = self.state.borrow_mut();
        state.roots = roots;
        state.excluded_cache = excluded_cache;
    }

    pub(crate) fn set_target_format(&self, target: Option<PackFormat>) {
        self.state.borrow_mut().target = target;
    }

    pub(crate) fn set_symlink_policy(&self, policy: SymlinkPolicy) {
        self.state.borrow_mut().allowed_symlinks = policy;
    }
}

impl RepositorySource for FolderRepositorySource {
    fn load_packs(&mut self) -> io::Result<Vec<Pack>> {
        let (packs, discovered) = {
            // No state borrow escapes into a pack or supplier. Avoid copying the
            // entire ID cache/configuration just to stage a synchronous scan.
            let state = self.state.borrow();
            let mut roots = HashSet::new();
            let mut entries_seen = HashSet::new();
            let mut discovered = HashMap::new();
            let mut packs = Vec::new();
            let excluded = |path: &Path| {
                state
                    .excluded_cache
                    .as_ref()
                    .is_some_and(|cache| path.starts_with(cache))
            };
            for root in &state.roots {
                if !roots.insert(root) || excluded(root) {
                    continue;
                }
                // Like Java, an unlistable optional root contributes no packs.
                // COMPAT: Directory creation belongs to AppCore, not this source.
                let entries = match std::fs::read_dir(root) {
                    Ok(entries) => entries,
                    Err(error) => {
                        tracing::warn!(path = %root.display(), %error, "Cannot scan resource pack discovery root; skipping root");
                        continue;
                    }
                };
                for entry in entries {
                    let entry =
                        entry.map_err(|error| path_error("read discovery entry", root, error))?;
                    let path = entry.path();
                    if excluded(&path) || !entries_seen.insert(path.clone()) {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let location = PackLocationInfo {
                        id: format!("file/{name}"),
                        title: Component::text(name),
                        source: PackSource::default(),
                        known_pack_info: None,
                    };
                    let prepared = (|| -> io::Result<_> {
                        let (effective, kind) =
                            prepare_pack_path(&path, entry.file_type()?, &state.allowed_symlinks)?;
                        let resources: Rc<dyn ResourcesSupplier> = if kind.is_dir() {
                            Rc::new(PathResourcesSupplier::new(effective))
                        } else if kind.is_file()
                            && effective.extension().is_some_and(|ext| ext == "zip")
                        {
                            Rc::new(FileResourcesSupplier::new(effective))
                        } else {
                            return Ok(None);
                        };
                        let metadata = Pack::read_metadata(
                            &location,
                            resources.as_ref(),
                            state.pack_type,
                            state.target,
                        )?;
                        Ok(metadata.map(|metadata| (resources, metadata)))
                    })();
                    let (resources, metadata) = match prepared {
                        Ok(Some(prepared)) => prepared,
                        Ok(None) => {
                            tracing::warn!(path = %path.display(), "Not a resource pack or missing pack metadata; skipping entry");
                            continue;
                        }
                        Err(error) => {
                            tracing::warn!(path = %path.display(), %error, "Cannot prepare resource pack; skipping entry");
                            continue;
                        }
                    };
                    // Allocate only after metadata and its independent close succeed.
                    let id = state
                        .discovered
                        .get(&path)
                        .cloned()
                        .unwrap_or_else(PackId::allocate);
                    discovered.insert(path, id.clone());
                    packs.push(Pack::with_id(
                        id,
                        location,
                        metadata,
                        PackEnablementConfig {
                            required: false,
                            default_position: PackPosition::Top,
                            fixed_position: false,
                        },
                        resources,
                    ));
                }
            }
            (packs, discovered)
        };
        // Entry iteration errors preserve the old associations. A successful
        // scan replaces them, including removal of packs from skipped roots.
        self.state.borrow_mut().discovered = discovered;
        Ok(packs)
    }
}
