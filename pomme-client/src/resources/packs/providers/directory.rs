use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use super::{
    PackResourceProvider, ReadStream, ResourceOrigin, ThreadSafePackResourceProvider,
    canonical_directory, check_directory, enumeration_error, is_file, lookup_error, matches_prefix,
    open_file, overlay_directory, resolve_within, source_file, valid_namespace, validate_query,
};
use crate::resources::resource::{ResourceContent, ResourceError, ResourceId};

/// Pack assets or a native namespace-root directory with sparse translations.
#[derive(Clone)]
pub(in crate::resources::packs) struct DirectoryPackResourceProvider {
    root: PathBuf,
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
    StandardAssets {
        pack_root: PathBuf,
    },
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
                let directory = overlay
                    .as_deref()
                    .map(overlay_directory)
                    .transpose()
                    .map_err(|source| {
                        enumeration_error(&origin, "open overlay directory", source)
                    })?;
                let root = pack_root.join(directory.unwrap_or_default()).join("assets");
                let source = Self {
                    root,
                    layout: DirectoryState::StandardAssets { pack_root },
                    origin,
                };
                source.assets_root().map_err(|error| {
                    enumeration_error(&source.origin, "open pack assets directory", error)
                })?;
                Ok(source)
            }
            DirectoryLayout::Mapped {
                root,
                namespace,
                remappings,
            } => {
                validate_query(Some(&namespace), "")?;
                if let Some(remappings) = &remappings {
                    for (id, path) in remappings.iter() {
                        if id.namespace() != namespace
                            || path.as_os_str().is_empty()
                            || path
                                .components()
                                .any(|component| !matches!(component, Component::Normal(_)))
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
                    root,
                    layout: DirectoryState::NamespaceRoot {
                        namespace,
                        remappings,
                    },
                    origin,
                })
            }
        }
    }

    fn assets_root(&self) -> io::Result<Option<PathBuf>> {
        let DirectoryState::StandardAssets { pack_root } = &self.layout else {
            let root = resolve_within(&self.root, &self.root)?;
            check_directory(&root)?;
            return Ok(Some(root));
        };
        let boundary = canonical_directory(pack_root)?;
        let relative = self
            .root
            .strip_prefix(pack_root)
            .map_err(io::Error::other)?;
        let mut current = boundary.clone();
        // Validate each ancestor, including overlay roots and dangling links,
        // before accepting an absent assets tree. Recheck on every operation.
        for component in relative.components() {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            }
            current = resolve_within(&boundary, &current)?;
            check_directory(&current)?;
        }
        Ok(Some(current))
    }

    fn accepts_namespace(&self, namespace: &str) -> bool {
        match &self.layout {
            DirectoryState::StandardAssets { .. } => true,
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

    fn locate(&self, id: &ResourceId) -> io::Result<Option<PathBuf>> {
        if !self.accepts_namespace(id.namespace()) {
            return Ok(None);
        }
        let Some(root) = self.assets_root()? else {
            return Ok(None);
        };
        let relative = match &self.layout {
            DirectoryState::StandardAssets { .. } => PathBuf::from(id.namespace()).join(id.path()),
            DirectoryState::NamespaceRoot { .. } => self
                .remapped_path(id)
                .unwrap_or_else(|| PathBuf::from(id.path())),
        };
        // Only the final resolved resource is confined to the source assets root.
        source_file(&root, &root.join(relative))
    }

    fn collect_resources(
        &self,
        root: &Path,
        directory: &Path,
        namespace: &str,
        prefix: &str,
        active: &mut BTreeSet<PathBuf>,
        resources: &mut BTreeSet<ResourceId>,
    ) -> io::Result<()> {
        // Track only the current ancestry: aliases remain visible under each
        // logical name, while symlink cycles cannot recurse indefinitely.
        if !active.insert(directory.to_path_buf()) {
            return Ok(());
        }
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                active.remove(directory);
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
            if self.remapped_path(&id).is_some() {
                // A shadowed file is irrelevant; directories can still contain
                // ordinary descendants whose IDs are not remapped.
                match std::fs::metadata(entry.path()) {
                    Ok(metadata) if metadata.is_dir() => {}
                    Ok(_) => continue,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                }
            }
            let Some((resolved, metadata)) = Self::entry_within(root, &entry.path())? else {
                continue;
            };
            if metadata.is_dir() {
                self.collect_resources(root, &resolved, namespace, &path, active, resources)?;
            } else if is_file(&resolved)? {
                resources.insert(id);
            }
        }
        active.remove(directory);
        Ok(())
    }

    /// Missing descendants can race enumeration; escapes and other failures
    /// are errors, just as in exact lookup.
    fn entry_within(root: &Path, path: &Path) -> io::Result<Option<(PathBuf, std::fs::Metadata)>> {
        let result = resolve_within(root, path)
            .and_then(|path| std::fs::metadata(&path).map(|metadata| (path, metadata)));
        match result {
            Ok(entry) => Ok(Some(entry)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
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
        let path = self.locate(id)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("missing resource {id:?}"))
        })?;
        open_file(&path)
    }

    fn namespaces(&self) -> Result<BTreeSet<String>, ResourceError> {
        let enumerate = || -> io::Result<_> {
            let mut namespaces = BTreeSet::new();
            let Some(root) = self.assets_root()? else {
                return Ok(namespaces);
            };
            if let DirectoryState::NamespaceRoot { namespace, .. } = &self.layout {
                namespaces.insert(namespace.clone());
                return Ok(namespaces);
            }
            for entry in std::fs::read_dir(&root)? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(namespace) = name.to_str().filter(|name| valid_namespace(name)) else {
                    continue;
                };
                if Self::entry_within(&root, &entry.path())?
                    .is_some_and(|(_, metadata)| metadata.is_dir())
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
        if !self.accepts_namespace(namespace) {
            return Ok(BTreeSet::new());
        }
        let enumerate = || -> io::Result<_> {
            let mut resources = BTreeSet::new();
            let Some(root) = self.assets_root()? else {
                return Ok(resources);
            };
            let path = match &self.layout {
                DirectoryState::StandardAssets { .. } => root.join(namespace).join(prefix),
                DirectoryState::NamespaceRoot { .. } => root.join(prefix),
            };
            if let Some((directory, metadata)) = Self::entry_within(&root, &path)?
                && metadata.is_dir()
            {
                self.collect_resources(
                    &root,
                    &directory,
                    namespace,
                    prefix,
                    &mut BTreeSet::new(),
                    &mut resources,
                )?;
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
                            && source_file(&root, &root.join(relative))?.is_some()
                        {
                            resources.insert(candidate.clone());
                        }
                    }
                }
            }
            Ok(resources)
        };
        enumerate().map_err(|source| enumeration_error(&self.origin, "list resources", source))
    }
}
