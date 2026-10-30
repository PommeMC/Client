use std::collections::{HashMap, HashSet};
use std::fs::FileType;
use std::io;
use std::path::{Path, PathBuf};

use super::filter::ResourceFilter;
use super::format::{PackFormat, read_installed_pack_format};
use super::metadata::PackMetadata;
use super::theme::{ConfiguredTheme, ThemeSpecification};
use super::{
    PackCompat, PackId, PackInfo, PackProvenance, PackStorage, ResourcePackDescriptor,
    ResourcePackLocation, ResourcePackSource,
};
use crate::resources::{ResourceError, ResourceOrigin, ResourceSources};

const SYMLINK_RULE_PREFIX: &str = "prefix";
const SYMLINK_RULE_GLOB: &str = "glob";
const SYMLINK_RULE_REGEX: &str = "regex";

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
        symlinks: &[SymlinkRule],
    ) -> Result<Self, ConfigurationError> {
        let pack_id = PackId::allocate();
        let location = prepare_location(&source.location, symlinks)?;
        let metadata = registration_metadata(&location)?;
        Ok(Self::from_prepared_source(
            source, pack_id, target, metadata,
        ))
    }

    /// Consume this operation's validated metadata without reopening the
    /// source. Retain the original location for subsequent setup-time
    /// revalidation.
    fn from_prepared_source(
        source: ResourcePackSource,
        pack_id: PackId,
        target: Option<PackFormat>,
        metadata: Option<PackMetadata>,
    ) -> Self {
        let info = PackInfo::from_metadata(metadata.as_ref(), &source, pack_id, target);
        Self {
            source,
            metadata,
            info,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct InstallationLocation {
    pub version: String,
    pub client_jar: PathBuf,
    /// Parent of the extracted `assets` directory; the JAR is metadata-only.
    pub extracted_root: PathBuf,
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
    allowed_symlinks: Vec<SymlinkRule>,
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
            allowed_symlinks: Vec::new(),
        }
    }

    /// Load one native-filesystem policy snapshot. None, unreadable files and
    /// any invalid rule deny every symlink; refresh is explicit, not per
    /// lookup.
    pub(crate) fn set_allowed_symlinks_file(&mut self, path: Option<PathBuf>) {
        self.allowed_symlinks = match path {
            None => Vec::new(),
            Some(path) => {
                match std::fs::read_to_string(&path).and_then(|text| parse_symlinks(&text)) {
                    Ok(rules) => rules,
                    Err(error) => {
                        tracing::warn!(path = %path.display(), %error, "Cannot load allowed symlinks; denying all symlinks");
                        Vec::new()
                    }
                }
            }
        };
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

    /// Owned snapshot, low-to-high: Pomme, Minecraft (extracted assets then
    /// index), selected theme, selections. No installation is needed to
    /// open the embedded bootstrap stack.
    pub(crate) fn sources(&self) -> Result<ResourceSources, ResourceError> {
        if let Some((location, pack_id)) = &self.installation {
            std::fs::read_dir(location.extracted_root.join("assets")).map_err(|source| {
                ResourceError::Io {
                    operation: "open installed extracted assets",
                    id: None,
                    origin: ResourceOrigin {
                        pack_id: pack_id.clone(),
                        name: "Minecraft built-in assets".to_owned(),
                    },
                    source,
                }
            })?;
        }
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
            let location = prepare_location(&pack.source.location, &self.allowed_symlinks)
                .map_err(|source| ResourceError::Io {
                    operation: "validate pack symlinks",
                    id: None,
                    origin: origin.clone(),
                    source: io::Error::other(source),
                })?;
            let metadata = location
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
            let storage = match &location {
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

    /// Register a new disabled record, even if the same source is already
    /// known.
    pub(crate) fn register_resource_pack(
        &mut self,
        source: ResourcePackSource,
    ) -> Result<PackId, super::ConfigurationError> {
        let pack = RegisteredPack::new(source, self.target_format, &self.allowed_symlinks)?;
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
    /// Roots and entry keys use lexical path identity without absolutization;
    /// rescanning the same entry path reuses its opaque ID, while differently
    /// named aliases can remain separate packs. Cache exclusion is lexical too.
    /// Returns whether discovery-owned active selections were removed.
    pub(crate) fn set_resource_pack_paths(
        &mut self,
        paths: Vec<PathBuf>,
    ) -> Result<bool, ConfigurationError> {
        let mut roots = Vec::new();
        let mut seen_roots = HashSet::new();
        for path in paths {
            if seen_roots.insert(path.clone()) {
                roots.push(path);
            }
        }
        let mut candidates = Vec::new();
        let mut discovered = HashMap::new();
        for root in &roots {
            if root.starts_with(&self.server_cache_dir) {
                continue;
            }
            let entries = std::fs::read_dir(root)
                .map_err(|source| ConfigurationError::io("scan discovery root", root, source))?;
            for entry in entries {
                let entry = entry.map_err(|source| {
                    ConfigurationError::io("read discovery entry", root, source)
                })?;
                let entry_path = entry.path();
                if entry_path.starts_with(&self.server_cache_dir)
                    || discovered.contains_key(&entry_path)
                {
                    continue;
                }
                // Validate before reading pack metadata. Keep the original
                // entry for identity and policy revalidation, not its target.
                let candidate = (|| -> io::Result<_> {
                    let (effective, kind) =
                        prepare_pack_path(&entry_path, entry.file_type()?, &self.allowed_symlinks)?;
                    let (location, prepared) = if kind.is_dir() {
                        (
                            ResourcePackLocation::Directory(entry_path.clone()),
                            ResourcePackLocation::Directory(effective),
                        )
                    } else if kind.is_file()
                        && effective.extension().is_some_and(|ext| ext == "zip")
                    {
                        (
                            ResourcePackLocation::Zip(entry_path.clone()),
                            ResourcePackLocation::Zip(effective),
                        )
                    } else {
                        return Ok(None);
                    };
                    // Missing metadata is not a discovered pack. Read/decode
                    // once after policy validation, then move it into the record.
                    Ok(prepared
                        .read_metadata()?
                        .map(|metadata| (location, metadata)))
                })();
                let (location, metadata) = match candidate {
                    Ok(Some(candidate)) => candidate,
                    Ok(None) => continue,
                    Err(error) => {
                        tracing::warn!(path = %entry_path.display(), %error, "Cannot prepare resource pack; skipping pack");
                        continue;
                    }
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                let source = ResourcePackSource {
                    location,
                    provenance: PackProvenance::Local,
                    fallback_name: name,
                };
                let existing = self.discovered_packs.get(&entry_path).and_then(|id| {
                    self.known_packs
                        .iter()
                        .find(|pack| &pack.info.pack_id == id)
                });
                let pack_id =
                    existing.map_or_else(PackId::allocate, |pack| pack.info.pack_id.clone());
                let mut pack = RegisteredPack::from_prepared_source(
                    source,
                    pack_id,
                    self.target_format,
                    Some(metadata),
                );
                let pack_id = pack.info.pack_id.clone();
                pack.info.enabled = self.active_packs.contains(&pack_id);
                candidates.push(pack);
                discovered.insert(entry_path, pack_id);
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
        let location = prepare_location(&pack.source.location, &self.allowed_symlinks)?;
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
            PackStorage::Directory {
                root: location.extracted_root.clone(),
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
    location
        .read_metadata()
        .map_err(|source| ConfigurationError::io("read pack.mcmeta", location.path(), source))
}

/// Native Rust matching, NOT Java PathMatcher/Pattern parity. Accepted patterns
/// can differ too (glob syntax, Unicode classes, case and platform separators).
/// Full Java compatibility is deferred for a follow-up issue after review.
enum SymlinkRule {
    Prefix(String),
    Glob(glob::Pattern),
    Regex(regex::Regex),
}

fn parse_symlinks(text: &str) -> io::Result<Vec<SymlinkRule>> {
    let mut rules = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let (tag, pattern) = if let Some(tagged) = line.strip_prefix('[') {
            tagged.split_once(']').ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("line {}: unterminated rule type", index + 1),
                )
            })?
        } else {
            (SYMLINK_RULE_PREFIX, line)
        };
        let rule = match tag {
            SYMLINK_RULE_PREFIX => SymlinkRule::Prefix(pattern.to_owned()),
            SYMLINK_RULE_GLOB => {
                SymlinkRule::Glob(glob::Pattern::new(pattern).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("line {}: {error}", index + 1),
                    )
                })?)
            }
            // Grouping keeps alternatives and inline flags inside the
            // whole-string anchors, unlike search or ^/$ line matching.
            SYMLINK_RULE_REGEX => {
                // Reject malformed raw syntax before it can escape the wrapper.
                regex::Regex::new(pattern).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("line {}: {error}", index + 1),
                    )
                })?;
                SymlinkRule::Regex(regex::Regex::new(&format!(r"\A(?:{pattern})\z")).map_err(
                    |error| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("line {}: {error}", index + 1),
                        )
                    },
                )?)
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("line {}: unknown rule type {tag:?}", index + 1),
                ));
            }
        };
        rules.push(rule);
    }
    Ok(rules)
}

fn validate_symlink(path: &Path, rules: &[SymlinkRule]) -> io::Result<PathBuf> {
    let target = std::fs::read_link(path)?;
    // Match the stored, native target, not a canonical destination. Never use
    // lossy UTF-8: unsupported OS strings fail closed. Cross-platform string
    // representation differences remain part of the Java-compatibility gap.
    let allowed = target.to_str().is_some_and(|target| {
        rules.iter().any(|rule| match rule {
            SymlinkRule::Prefix(prefix) => target.starts_with(prefix),
            SymlinkRule::Glob(pattern) => pattern.matches_with(
                target,
                glob::MatchOptions {
                    case_sensitive: true,
                    require_literal_separator: true,
                    require_literal_leading_dot: false,
                },
            ),
            SymlinkRule::Regex(pattern) => pattern.is_match(target),
        })
    });
    if !allowed {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "forbidden symlink {} -> {}",
                path.display(),
                target.display()
            ),
        ));
    }
    Ok(target)
}

/// Management-boundary validation, not a sandbox: ancestors are not walked,
/// descendant link targets are not traversed, and later filesystem edits race
/// access. Explicit installation, index and theme roots do not use this gate.
/// The caller supplies its no-follow classification; only a root link needs a
/// second classification, for the distinct effective target.
fn prepare_pack_path(
    path: &Path,
    mut kind: FileType,
    rules: &[SymlinkRule],
) -> io::Result<(PathBuf, FileType)> {
    let mut effective = path.to_owned();
    if kind.is_symlink() {
        // Preserve PackDetector's single top-link rule, including its odd
        // CWD-relative interpretation of a relative stored target. Do NOT
        // validate that target then open the original path (which would follow
        // relative to the link parent and could open a different, unchecked tree).
        effective = validate_symlink(path, rules)?;
        kind = std::fs::symlink_metadata(&effective)?.file_type();
    }
    if kind.is_dir() {
        validate_pack_tree(&effective, rules)?;
    } else if !kind.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "pack root is not a directory or regular file: {}",
                effective.display()
            ),
        ));
    }
    Ok((effective, kind))
}

fn validate_pack_tree(root: &Path, rules: &[SymlinkRule]) -> io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            validate_symlink(&entry.path(), rules)?;
        } else if kind.is_dir() {
            validate_pack_tree(&entry.path(), rules)?;
        }
    }
    Ok(())
}

fn prepare_location(
    location: &ResourcePackLocation,
    rules: &[SymlinkRule],
) -> Result<ResourcePackLocation, ConfigurationError> {
    let kind = std::fs::symlink_metadata(location.path())
        .map_err(|source| {
            ConfigurationError::io("validate pack symlinks", location.path(), source)
        })?
        .file_type();
    let (path, kind) = prepare_pack_path(location.path(), kind, rules).map_err(|source| {
        ConfigurationError::io("validate pack symlinks", location.path(), source)
    })?;
    // The tree walk already opens directory roots. Check the declared type
    // here instead of probing their readiness again before metadata reads.
    match location {
        ResourcePackLocation::Directory(_) if kind.is_dir() => {
            Ok(ResourcePackLocation::Directory(path))
        }
        ResourcePackLocation::Zip(_) if kind.is_file() => Ok(ResourcePackLocation::Zip(path)),
        _ => Err(ConfigurationError::io(
            "validate pack symlinks",
            location.path(),
            io::Error::new(
                io::ErrorKind::InvalidData,
                "pack root does not match its declared storage type",
            ),
        )),
    }
}
