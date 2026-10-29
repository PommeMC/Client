use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use super::filter::ResourceFilter;
use super::format::{PackFormat, read_installed_pack_format};
use super::metadata::{self as keys, PackMetadata};
use super::theme::{ConfiguredTheme, ThemeSpecification};
use super::{
    PackCompat, PackId, PackInfo, PackProvenance, PackStorage, ResourcePackDescriptor,
    ResourcePackLocation, ResourcePackSource,
};
use crate::resources::{ResourceError, ResourceOrigin, ResourceSources};

struct RegisteredPack {
    source: ResourcePackSource,
    /// Owned registration snapshot; resource sources reread their own metadata.
    metadata: Option<PackMetadata>,
    info: PackInfo,
}

impl RegisteredPack {
    fn new(
        source: ResourcePackSource,
        target: Option<PackFormat>,
    ) -> Result<Self, ConfigurationError> {
        let pack_id = PackId::allocate();
        Self::from_source(source, pack_id, target)
    }

    fn from_source(
        source: ResourcePackSource,
        pack_id: PackId,
        target: Option<PackFormat>,
    ) -> Result<Self, ConfigurationError> {
        let metadata = registration_metadata(&source.location)?;
        let info = PackInfo::from_metadata(metadata.as_ref(), &source, pack_id, target);
        Ok(Self {
            source,
            metadata,
            info,
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct InstallationLocation {
    pub version: String,
    pub client_jar: PathBuf,
    pub index_file: PathBuf,
    pub objects_dir: PathBuf,
}

pub(crate) struct ResourceConfiguration {
    embedded_pack_id: PackId,
    installation: Option<(InstallationLocation, PackId)>,
    selected_theme: Option<ConfiguredTheme>,
    target_format: Option<PackFormat>,
    resource_pack_paths: Vec<PathBuf>,
    server_cache_dir: PathBuf,
    active_packs: Vec<PackId>,
    known_packs: Vec<RegisteredPack>,
    discovered_packs: HashMap<PathBuf, PackId>,
}

impl ResourceConfiguration {
    pub(crate) fn new(server_cache_dir: PathBuf) -> Self {
        Self {
            embedded_pack_id: PackId::allocate(),
            installation: None,
            selected_theme: None,
            target_format: None,
            resource_pack_paths: Vec::new(),
            server_cache_dir,
            active_packs: Vec::new(),
            known_packs: Vec::new(),
            discovered_packs: HashMap::new(),
        }
    }

    /// Replace the complete installation, refreshing compatibility even when
    /// the same locations are supplied again. Providers open the actual assets.
    pub(crate) fn set_installation(&mut self, installation: Option<InstallationLocation>) {
        let target_format = installation.as_ref().and_then(|location| {
            match read_installed_pack_format(&location.client_jar, &location.version) {
                Ok(format) => Some(format),
                Err(error) => {
                    tracing::debug!(%error, "Installed pack format unavailable; compatibility is unknown");
                    None
                }
            }
        });
        self.installation = installation.map(|location| {
            let id = self
                .installation
                .as_ref()
                .filter(|(previous, _)| previous == &location)
                .map_or_else(PackId::allocate, |(_, id)| id.clone());
            (location, id)
        });
        self.target_format = target_format;
        for pack in &mut self.known_packs {
            pack.info.compat = PackCompat::for_format(
                pack.metadata.as_ref().and_then(PackMetadata::format_range),
                target_format,
            );
        }
    }

    /// None clears theme intent; Minecraft explicitly uses the installation
    /// baseline.
    pub(crate) fn set_theme(&mut self, theme: Option<ThemeSpecification>) {
        self.selected_theme = theme.map(|specification| {
            let pack_id = self
                .selected_theme
                .as_ref()
                .filter(|previous| previous.specification == specification)
                .map_or_else(PackId::allocate, |previous| previous.pack_id.clone());
            ConfiguredTheme {
                specification,
                pack_id,
            }
        });
    }

    /// Owned snapshot, low-to-high: Pomme, Minecraft (JAR then index),
    /// selected theme, selections. No installation is needed to open the
    /// embedded bootstrap stack.
    pub(crate) fn sources(&self) -> Result<ResourceSources, ResourceError> {
        let mut sources =
            builtin_descriptors(self.embedded_pack_id.clone(), self.installation.as_ref());
        if let Some(theme) = &self.selected_theme
            && let Some(source) = theme.source()?
        {
            sources.push(source);
        }
        sources.extend(self.active_pack_descriptors()?);
        Ok(ResourceSources(sources))
    }

    /// Selected logical packs in low-to-high order, including filter-only
    /// packs. Revalidate roots and reread root metadata for each new
    /// resource view, rather than using the discovery UI's possibly stale
    /// metadata.
    fn active_pack_descriptors(&self) -> Result<Vec<ResourcePackDescriptor>, ResourceError> {
        let mut descriptors = Vec::new();
        for pack in self.active_records() {
            let origin = ResourceOrigin {
                pack_id: pack.info.pack_id.clone(),
                name: pack.info.name.clone(),
            };
            if let ResourcePackLocation::Directory(path) = &pack.source.location {
                Self::validate_directory_pack_root(path, &origin)?;
            }
            let metadata = pack
                .source
                .location
                .read_metadata()
                .and_then(|metadata| metadata.map(PackMetadata::require_object).transpose())
                .map_err(|source| ResourceError::Io {
                    operation: "read pack.mcmeta",
                    id: None,
                    origin: origin.clone(),
                    source,
                })?;
            let (filter, overlays) = match metadata {
                Some(metadata) => metadata
                    .resource_sections(self.target_format, &origin)
                    .map_err(|source| ResourceError::Io {
                        operation: "validate pack overlays",
                        id: None,
                        origin: origin.clone(),
                        source,
                    })?,
                None => (ResourceFilter::default(), Vec::new()),
            };
            let storage = match &pack.source.location {
                ResourcePackLocation::Zip(path) => PackStorage::Zip {
                    path: path.clone(),
                    overlays,
                },
                ResourcePackLocation::Directory(path) => PackStorage::Directory {
                    root: path.clone(),
                    overlays,
                },
            };
            descriptors.push(ResourcePackDescriptor {
                storage: vec![storage],
                filter,
                origin,
            });
        }
        Ok(descriptors)
    }

    fn validate_directory_pack_root(
        root: &Path,
        origin: &ResourceOrigin,
    ) -> Result<(), ResourceError> {
        std::fs::read_dir(root)
            .map(|_| ())
            .map_err(|source| ResourceError::Io {
                operation: "open source directory",
                id: None,
                origin: origin.clone(),
                source,
            })
    }

    /// Register a new disabled record, even if the same source is already
    /// known.
    pub(crate) fn register_resource_pack(
        &mut self,
        source: ResourcePackSource,
    ) -> Result<PackId, super::ConfigurationError> {
        let pack = RegisteredPack::new(source, self.target_format)?;
        let id = pack.info.pack_id.clone();
        self.known_packs.push(pack);
        Ok(id)
    }

    /// Forget the registration, never its files or previously opened providers.
    pub(crate) fn unregister_resource_pack(&mut self, pack_id: &PackId) -> bool {
        let before = self.known_packs.len();
        self.known_packs
            .retain(|pack| &pack.info.pack_id != pack_id);
        self.active_packs.retain(|id| id != pack_id);
        self.discovered_packs.retain(|_, id| id != pack_id);
        before != self.known_packs.len()
    }

    pub(crate) fn disable_resource_pack(&mut self, pack_id: &PackId) -> bool {
        let before = self.active_packs.len();
        self.active_packs.retain(|id| id != pack_id);
        if let Some(pack) = self
            .known_packs
            .iter_mut()
            .find(|pack| &pack.info.pack_id == pack_id)
        {
            pack.info.enabled = false;
        }
        before != self.active_packs.len()
    }

    pub(crate) fn disable_all_server_packs(&mut self) -> bool {
        let mut changed = false;
        for pack in &mut self.known_packs {
            if pack.info.provenance == PackProvenance::Server {
                changed |= pack.info.enabled;
                pack.info.enabled = false;
                self.active_packs.retain(|id| id != &pack.info.pack_id);
            }
        }
        changed
    }

    /// Transactionally discover the proposed roots, without enabling packs.
    /// Returns whether discovery-owned active selections were removed.
    pub(crate) fn set_resource_pack_paths(
        &mut self,
        paths: Vec<PathBuf>,
    ) -> Result<bool, ConfigurationError> {
        let mut roots = Vec::new();
        let mut seen_roots = HashSet::new();
        for path in paths {
            let root = path.canonicalize().map_err(|source| {
                ConfigurationError::io("canonicalize discovery root", &path, source)
            })?;
            if seen_roots.insert(root.clone()) {
                roots.push(root);
            }
        }
        let cache = if roots.is_empty() {
            None
        } else {
            match self.server_cache_dir.canonicalize() {
                Ok(path) => Some(path),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(source) => {
                    return Err(ConfigurationError::io(
                        "canonicalize server cache",
                        &self.server_cache_dir,
                        source,
                    ));
                }
            }
        };
        let mut candidates = Vec::new();
        let mut discovered = HashMap::new();
        for root in &roots {
            if cache.as_ref().is_some_and(|cache| root.starts_with(cache)) {
                continue;
            }
            let entries = std::fs::read_dir(root)
                .map_err(|source| ConfigurationError::io("scan discovery root", root, source))?;
            for entry in entries {
                let entry = entry.map_err(|source| {
                    ConfigurationError::io("read discovery entry", root, source)
                })?;
                let entry_path = entry.path();
                let path = entry_path.canonicalize().map_err(|source| {
                    ConfigurationError::io("canonicalize pack", &entry_path, source)
                })?;
                if entry_path == self.server_cache_dir
                    || cache.as_ref().is_some_and(|cache| path.starts_with(cache))
                {
                    continue;
                }
                let metadata = std::fs::metadata(&path)
                    .map_err(|source| ConfigurationError::io("inspect pack", &path, source))?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if discovered.contains_key(&path) {
                    continue;
                }
                let location = if metadata.is_dir() {
                    let metadata_path = path.join(keys::PACK_METADATA_FILE);
                    match std::fs::metadata(&metadata_path) {
                        Ok(_) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(source) => {
                            return Err(ConfigurationError::io(
                                "inspect pack metadata",
                                &metadata_path,
                                source,
                            ));
                        }
                    }
                    ResourcePackLocation::Directory(path.clone())
                } else if entry_path.extension().is_some_and(|ext| ext == "zip") {
                    ResourcePackLocation::Zip(path.clone())
                } else {
                    continue;
                };
                let source = ResourcePackSource {
                    location,
                    provenance: PackProvenance::Local,
                    fallback_name: name,
                };
                let existing = self.discovered_packs.get(&path).and_then(|id| {
                    self.known_packs
                        .iter()
                        .find(|pack| &pack.info.pack_id == id)
                });
                let prepared = if let Some(existing) = existing {
                    RegisteredPack::from_source(
                        source,
                        existing.info.pack_id.clone(),
                        self.target_format,
                    )
                } else {
                    RegisteredPack::new(source, self.target_format)
                };
                let mut pack = match prepared {
                    Ok(pack) => pack,
                    Err(
                        error @ ConfigurationError::Io {
                            operation: "read pack.mcmeta",
                            ..
                        },
                    ) => {
                        tracing::warn!(path = %path.display(), %error, "Failed to read resource pack metadata; skipping pack");
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if pack.metadata.is_none() {
                    continue;
                }
                let pack_id = pack.info.pack_id.clone();
                pack.info.enabled = self.active_packs.contains(&pack_id);
                candidates.push(pack);
                discovered.insert(path, pack_id);
            }
        }
        let removed: HashSet<_> = self
            .discovered_packs
            .iter()
            .filter(|(path, _)| !discovered.contains_key(*path))
            .map(|(_, id)| id.clone())
            .collect();
        // All fallible preparation is complete. Replace discovery records in
        // place, leaving unrelated registrations and their metadata untouched.
        self.known_packs.retain_mut(|pack| {
            if removed.contains(&pack.info.pack_id) {
                return false;
            }
            if let Some(index) = candidates
                .iter()
                .position(|candidate| candidate.info.pack_id == pack.info.pack_id)
            {
                *pack = candidates.remove(index);
            }
            true
        });
        // Unmatched candidates retain first-seen discovery order.
        self.known_packs.extend(candidates);
        let before = self.active_packs.len();
        self.active_packs.retain(|id| !removed.contains(id));
        self.resource_pack_paths = roots;
        self.discovered_packs = discovered;
        Ok(before != self.active_packs.len())
    }

    /// Rescan committed roots; true means selected local packs were removed.
    pub(crate) fn refresh_local_packs(&mut self) -> Result<bool, ConfigurationError> {
        self.set_resource_pack_paths(self.resource_pack_paths.clone())
    }

    /// Prepare before changing activation; re-enabling moves to highest
    /// priority.
    pub(crate) fn enable_resource_pack(
        &mut self,
        pack_id: &PackId,
    ) -> Result<(), ConfigurationError> {
        let pack = self
            .known_packs
            .iter_mut()
            .find(|pack| &pack.info.pack_id == pack_id)
            .ok_or_else(|| ConfigurationError::UnknownPack(pack_id.clone()))?;
        let location = &pack.source.location;
        if let ResourcePackLocation::Directory(path) = location {
            std::fs::read_dir(path)
                .map_err(|source| ConfigurationError::io("open pack directory", path, source))?;
        }
        location
            .read_metadata()
            .and_then(|metadata| metadata.map(PackMetadata::require_object).transpose())
            .map_err(|source| {
                ConfigurationError::io("read pack.mcmeta", location.path(), source)
            })?;
        self.active_packs.retain(|id| id != pack_id);
        self.active_packs.push(pack_id.clone());
        pack.info.enabled = true;
        Ok(())
    }

    fn active_records(&self) -> impl Iterator<Item = &RegisteredPack> {
        self.active_packs.iter().filter_map(|id| {
            self.known_packs
                .iter()
                .find(|pack| &pack.info.pack_id == id)
        })
    }

    pub fn active_pack_info(&self) -> Vec<PackInfo> {
        self.active_records()
            .map(|pack| pack.info.clone())
            .collect()
    }

    pub(crate) fn resource_packs(&self) -> impl Iterator<Item = &PackInfo> {
        self.known_packs.iter().map(|pack| &pack.info)
    }

    pub fn server_cache_dir(&self) -> &Path {
        &self.server_cache_dir
    }

    /// Existing font fixtures intentionally omit the embedded bootstrap layer.
    #[cfg(test)]
    pub(crate) fn fixture_sources(
        &self,
        client_jar: &Path,
    ) -> Result<ResourceSources, ResourceError> {
        let mut sources = vec![ResourcePackDescriptor {
            storage: vec![PackStorage::Zip {
                path: client_jar.to_owned(),
                overlays: Vec::new(),
            }],
            origin: ResourceOrigin {
                pack_id: PackId::allocate(),
                name: "Fixture".to_owned(),
            },
            filter: Default::default(),
        }];
        sources.extend(self.active_pack_descriptors()?);
        Ok(ResourceSources(sources))
    }

    #[cfg(test)]
    pub(crate) fn jar_fixture_sources(client_jar: &Path) -> ResourceSources {
        let mut sources = builtin_descriptors(PackId::allocate(), None);
        sources.push(ResourcePackDescriptor {
            storage: vec![PackStorage::Zip {
                path: client_jar.to_owned(),
                overlays: Vec::new(),
            }],
            origin: ResourceOrigin {
                pack_id: PackId::allocate(),
                name: "Minecraft built-in assets".to_owned(),
            },
            filter: Default::default(),
        });
        ResourceSources(sources)
    }
}

/// Common low-to-high registration for bootstrap, GUI and vanilla-only modes.
fn builtin_descriptors(
    embedded_pack_id: PackId,
    installation: Option<&(InstallationLocation, PackId)>,
) -> Vec<ResourcePackDescriptor> {
    let mut sources = vec![ResourcePackDescriptor {
        storage: vec![PackStorage::Embedded],
        origin: ResourceOrigin {
            pack_id: embedded_pack_id,
            name: "Pomme embedded assets".to_owned(),
        },
        filter: Default::default(),
    }];
    if let Some((location, pack_id)) = installation {
        let storage = vec![
            PackStorage::Zip {
                path: location.client_jar.clone(),
                overlays: Vec::new(),
            },
            PackStorage::Indexed {
                index_file: location.index_file.clone(),
                objects_dir: location.objects_dir.clone(),
            },
        ];
        sources.push(ResourcePackDescriptor {
            storage,
            origin: ResourceOrigin {
                pack_id: pack_id.clone(),
                name: "Minecraft built-in assets".to_owned(),
            },
            filter: Default::default(),
        });
    }
    sources
}

#[derive(Debug)]
pub(crate) enum ConfigurationError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    UnknownPack(PackId),
}

impl ConfigurationError {
    fn io(operation: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.to_owned(),
            source,
        }
    }
}

impl std::fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(f, "{operation} at {}: {source}", path.display()),
            Self::UnknownPack(id) => write!(f, "unknown resource pack: {id:?}"),
        }
    }
}

impl std::error::Error for ConfigurationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::UnknownPack(_) => None,
        }
    }
}

fn registration_metadata(
    location: &ResourcePackLocation,
) -> Result<Option<PackMetadata>, ConfigurationError> {
    if let ResourcePackLocation::Directory(path) = location {
        std::fs::read_dir(path)
            .map_err(|source| ConfigurationError::io("open pack directory", path, source))?;
    }
    location
        .read_metadata()
        .map_err(|source| ConfigurationError::io("read pack.mcmeta", location.path(), source))
}
