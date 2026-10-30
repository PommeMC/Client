use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use super::{
    PackResourceProvider, ReadStream, ResourceOrigin, ThreadSafePackResourceProvider,
    canonical_directory, check_directory, enumeration_error, is_file, lookup_error, matches_prefix,
    open_file, overlay_directory, valid_namespace, validate_query, validate_storage_path,
};
use crate::resources::resource::{ResourceContent, ResourceError, ResourceId};

/// Pack assets or a native namespace-root directory with sparse translations.
#[derive(Clone)]
pub(in crate::resources::packs) struct DirectoryPackResourceProvider {
    // Missing assets at setup are valid for metadata/filter-only packs.
    root: Option<PathBuf>,
    layout: DirectoryState,
    origin: ResourceOrigin,
}

pub(in crate::resources::packs) enum DirectoryLayout {
    /// Pack root containing `assets/<namespace>`, optionally under an overlay.
    ResourcePack {
        root: PathBuf,
        overlay: Option<String>,
    },
    /// Native root for one namespace; paths without remappings are
    /// root-relative.
    Mapped {
        root: PathBuf,
        namespace: String,
        remappings: Option<Arc<BTreeMap<ResourceId, PathBuf>>>,
    },
}

#[derive(Clone)]
enum DirectoryState {
    StandardAssets,
    NamespaceRoot {
        namespace: String,
        remappings: Option<Arc<BTreeMap<ResourceId, PathBuf>>>,
    },
}

impl DirectoryPackResourceProvider {
    pub(in crate::resources::packs) fn open(
        layout: DirectoryLayout,
        origin: ResourceOrigin,
    ) -> Result<Self, ResourceError> {
        match layout {
            DirectoryLayout::ResourcePack {
                root: pack_root,
                overlay,
            } => {
                let pack_root = canonical_directory(&pack_root).map_err(|source| {
                    enumeration_error(&origin, "open source directory", source)
                })?;
                let directory = overlay
                    .as_deref()
                    .map(overlay_directory)
                    .transpose()
                    .map_err(|source| {
                        enumeration_error(&origin, "open overlay directory", source)
                    })?;
                let root = pack_root.join(directory.unwrap_or_default()).join("assets");
                let root = Self::prepare_assets_root(&pack_root, &root).map_err(|source| {
                    enumeration_error(&origin, "open pack assets directory", source)
                })?;
                Ok(Self {
                    root,
                    layout: DirectoryState::StandardAssets,
                    origin,
                })
            }
            DirectoryLayout::Mapped {
                root,
                namespace,
                remappings,
            } => {
                validate_query(Some(&namespace), "")?;
                validate_storage_path(&namespace).map_err(|source| {
                    enumeration_error(&origin, "validate mapped namespace", source)
                })?;
                if let Some(remappings) = &remappings {
                    for (id, path) in remappings.iter() {
                        if id.namespace() != namespace
                            || validate_storage_path(id.path()).is_err()
                            || !path
                                .to_str()
                                .is_some_and(|path| validate_storage_path(path).is_ok())
                        {
                            return Err(enumeration_error(
                                &origin,
                                "validate directory remapping",
                                io::Error::new(
                                    io::ErrorKind::InvalidInput,
                                    format!(
                                        "invalid directory remapping {id} -> {}",
                                        path.display()
                                    ),
                                ),
                            ));
                        }
                    }
                }
                let root = canonical_directory(&root).map_err(|source| {
                    enumeration_error(&origin, "open source directory", source)
                })?;
                Ok(Self {
                    root: Some(root),
                    layout: DirectoryState::NamespaceRoot {
                        namespace,
                        remappings,
                    },
                    origin,
                })
            }
        }
    }

    fn prepare_assets_root(pack_root: &Path, root: &Path) -> io::Result<Option<PathBuf>> {
        let relative = root.strip_prefix(pack_root).map_err(io::Error::other)?;
        let mut current = pack_root.to_owned();
        // Validate overlay/assets components only at setup. The pack root was
        // already validated and anchored; later accesses trust this layout.
        for component in relative.components() {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            }
            check_directory(&current)?;
        }
        Ok(Some(current))
    }

    fn accepts_namespace(&self, namespace: &str) -> bool {
        match &self.layout {
            DirectoryState::StandardAssets => true,
            DirectoryState::NamespaceRoot {
                namespace: configured,
                ..
            } => namespace == configured,
        }
    }

    /// Exact mappings win over derived companions; translation is single-step.
    fn remapped_path(&self, id: &ResourceId) -> Option<PathBuf> {
        let DirectoryState::NamespaceRoot {
            remappings: Some(remappings),
            ..
        } = &self.layout
        else {
            return None;
        };
        if let Some(path) = remappings.get(id) {
            return Some(path.clone());
        }
        let base = id.path().strip_suffix(".mcmeta")?;
        let base = ResourceId::new(&format!("{}:{base}", id.namespace())).ok()?;
        let mut path = remappings.get(&base)?.as_os_str().to_os_string();
        path.push(".mcmeta");
        Some(PathBuf::from(path))
    }

    /// Resolve the prepared layout without filesystem I/O.
    fn resource_path(&self, id: &ResourceId) -> io::Result<Option<PathBuf>> {
        validate_storage_path(id.namespace())?;
        validate_storage_path(id.path())?;
        if !self.accepts_namespace(id.namespace()) {
            return Ok(None);
        }
        let Some(root) = &self.root else {
            return Ok(None);
        };
        let relative = match &self.layout {
            DirectoryState::StandardAssets => PathBuf::from(id.namespace()).join(id.path()),
            DirectoryState::NamespaceRoot { .. } => self
                .remapped_path(id)
                .unwrap_or_else(|| PathBuf::from(id.path())),
        };
        Ok(Some(root.join(relative)))
    }

    fn locate(&self, id: &ResourceId) -> io::Result<Option<PathBuf>> {
        let Some(path) = self.resource_path(id)? else {
            return Ok(None);
        };
        Ok(is_file(&path)?.then_some(path))
    }

    fn collect_resources(
        &self,
        directory: &Path,
        namespace: &str,
        prefix: &str,
        resources: &mut BTreeSet<ResourceId>,
    ) -> io::Result<()> {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let path = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            let Ok(id) = ResourceId::new(&format!("{namespace}:{path}")) else {
                continue;
            };
            // Like Files.find's supplied attributes, classify each entry once
            // without following links encountered during the walk.
            let kind = entry.file_type()?;
            if self.remapped_path(&id).is_some() && !kind.is_dir() {
                // A shadowed file is irrelevant; directories can still contain
                // ordinary descendants whose IDs are not remapped.
                continue;
            }
            if kind.is_dir() {
                self.collect_resources(&entry.path(), namespace, &path, resources)?;
            } else if kind.is_file() {
                resources.insert(id);
            }
        }
        Ok(())
    }
}

impl PackResourceProvider for DirectoryPackResourceProvider {
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
        let path = self
            .locate(id)
            .map_err(|source| lookup_error(id, &self.origin, source))?;
        Ok(path.map(|_| ResourceContent::new(id.clone(), self)))
    }

    fn open(&self, id: &ResourceId) -> io::Result<ReadStream> {
        let path = self.resource_path(id)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("missing resource {id:?}"))
        })?;
        open_file(&path)
    }

    fn namespaces(&self) -> Result<BTreeSet<String>, ResourceError> {
        let enumerate = || -> io::Result<_> {
            let mut namespaces = BTreeSet::new();
            let Some(root) = self.root.as_deref() else {
                return Ok(namespaces);
            };
            if let DirectoryState::NamespaceRoot { namespace, .. } = &self.layout {
                namespaces.insert(namespace.clone());
                return Ok(namespaces);
            }
            let entries = match std::fs::read_dir(root) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(namespaces),
                Err(error) => return Err(error),
            };
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name();
                let Some(namespace) = name.to_str().filter(|name| valid_namespace(name)) else {
                    continue;
                };
                // Vanilla getNamespaces follows directory links. Keep I/O
                // failures typed rather than hiding them in Path::is_dir.
                match std::fs::metadata(entry.path()) {
                    Ok(metadata) if metadata.is_dir() => {
                        namespaces.insert(namespace.to_owned());
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
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
        if !self.accepts_namespace(namespace) {
            return Ok(BTreeSet::new());
        }
        let enumerate = || -> io::Result<_> {
            validate_storage_path(namespace)?;
            if !prefix.is_empty() {
                validate_storage_path(prefix)?;
            }
            let mut resources = BTreeSet::new();
            let Some(root) = self.root.as_deref() else {
                return Ok(resources);
            };
            let relative = match &self.layout {
                DirectoryState::StandardAssets => PathBuf::from(namespace).join(prefix),
                DirectoryState::NamespaceRoot { .. } => PathBuf::from(prefix),
            };
            let start = root.join(relative);
            // Files.find does not follow a link at the walk's start either;
            // ancestors of the requested prefix resolve normally.
            match std::fs::symlink_metadata(&start) {
                Ok(metadata) if metadata.is_dir() => {
                    self.collect_resources(&start, namespace, prefix, &mut resources)?;
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            if let DirectoryState::NamespaceRoot {
                remappings: Some(remappings),
                ..
            } = &self.layout
            {
                for id in remappings.keys() {
                    let companion =
                        ResourceId::new(&format!("{id}.mcmeta")).map_err(io::Error::other)?;
                    for candidate in [id, &companion] {
                        if matches_prefix(candidate.path(), prefix)
                            && let Some(relative) = self.remapped_path(candidate)
                        {
                            // A standalone translation is an ordinary file
                            // candidate, not a directory-walk entry.
                            match std::fs::metadata(root.join(relative)) {
                                Ok(metadata) if metadata.is_file() => {
                                    resources.insert(candidate.clone());
                                }
                                Ok(_) => {}
                                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                                Err(error) => return Err(error),
                            }
                        }
                    }
                }
            }
            Ok(resources)
        };
        enumerate().map_err(|source| enumeration_error(&self.origin, "list resources", source))
    }
}
