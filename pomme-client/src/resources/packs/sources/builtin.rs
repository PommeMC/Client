//! Bootstrap, explicit Minecraft installation and optional mapped theme.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use pomme_protocol::KnownPack;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{RepositorySource, path_error};
use crate::chat_component::Component;
use crate::resources::pack_resources::{
    EmbeddedResourcesSupplier, MappedResourcesSupplier, PackType, VanillaResourcesSupplier,
};
use crate::resources::packs::format::{MAX_COMPONENT, format_component};
use crate::resources::packs::metadata::{PACK_SECTION, last_pre_minor_format};
use crate::resources::packs::{
    FeatureFlagSet, Pack, PackCompat, PackEnablementConfig, PackFormat, PackId, PackLocationInfo,
    PackMetadata, PackPosition, PackSource,
};
use crate::resources::{ResourceError, ResourceId};

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ThemeSpecification {
    Java,
    Pomme { root: PathBuf },
    Bedrock { root: PathBuf },
}

/// Pure mount definition for the built-in repository source.
struct ThemeMount<'a> {
    root: &'a std::path::Path,
    id: &'static str,
    name: &'static str,
    namespace: &'static str,
    remappings: BTreeMap<ResourceId, PathBuf>,
}

impl ThemeSpecification {
    fn mount(&self) -> Result<Option<ThemeMount<'_>>, ResourceError> {
        let (root, id, name, directory) = match self {
            Self::Java => return Ok(None),
            Self::Pomme { root } => (root, "theme/pomme", "Pomme theme assets", "panoramas"),
            Self::Bedrock { root } => {
                (root, "theme/bedrock", "Bedrock theme assets", "textures/ui")
            }
        };
        let remappings = (0..6)
            .map(|face| {
                Ok((
                    panorama_id(face)?,
                    PathBuf::from(directory).join(format!("panorama_{face}.png")),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, ResourceError>>()?;
        Ok(Some(ThemeMount {
            root,
            id,
            name,
            namespace: "minecraft",
            remappings,
        }))
    }
}

fn panorama_id(face: u32) -> Result<ResourceId, ResourceError> {
    ResourceId::new(&format!(
        "minecraft:textures/gui/title/background/panorama_{face}.png"
    ))
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

struct Installation {
    location: InstallationLocation,
    id: PackId,
    verified_format: Option<PackFormat>,
}

struct BuiltinState {
    pomme_id: PackId,
    installation: Option<Installation>,
    theme: Option<(ThemeSpecification, PackId)>,
    target: Option<PackFormat>,
}

/// Box a clone; keep the concrete handle for subsequent installation/theme
/// changes. Only immutable owned supplier definitions escape a load's borrow.
#[derive(Clone)]
pub(crate) struct BuiltinRepositorySource {
    state: Rc<RefCell<BuiltinState>>,
}

impl BuiltinRepositorySource {
    /// I/O-free bootstrap: Pomme needs neither an installation nor a theme.
    pub(crate) fn new(target: Option<PackFormat>) -> Self {
        Self {
            state: Rc::new(RefCell::new(BuiltinState {
                pomme_id: PackId::allocate(),
                installation: None,
                theme: None,
                target,
            })),
        }
    }

    /// Reads only installed version.json on this explicit update, even for an
    /// unchanged location. An unverified version is never advertised as core.
    /// Callers must propagate target_format() to folder/registered handles and
    /// reload the repository; there is deliberately no cross-source controller.
    pub(crate) fn set_installation(&self, location: Option<InstallationLocation>) {
        let format = location.as_ref().and_then(|location| {
            match read_installed_pack_format(&location.client_jar, &location.version) {
                Ok(format) => Some(format),
                Err(error) => {
                    tracing::warn!(%error, "Installed pack format unavailable; compatibility is unknown");
                    None
                }
            }
        });
        let mut state = self.state.borrow_mut();
        state.installation = location.map(|location| {
            let id = state
                .installation
                .as_ref()
                .filter(|old| old.location == location)
                .map_or_else(PackId::allocate, |old| old.id.clone());
            Installation {
                location,
                id,
                verified_format: format,
            }
        });
        state.target = format;
    }

    pub(crate) fn target_format(&self) -> Option<PackFormat> {
        self.state.borrow().target
    }

    /// Explicit override of the comparison target, not installed identity or
    /// fallback metadata. Propagation to other source handles remains explicit.
    pub(crate) fn set_target_format(&self, target: Option<PackFormat>) {
        self.state.borrow_mut().target = target;
    }

    pub(crate) fn set_theme(&self, theme: Option<ThemeSpecification>) {
        let mut state = self.state.borrow_mut();
        state.theme = theme
            .filter(|theme| !matches!(theme, ThemeSpecification::Java))
            .map(|theme| {
                let id = state
                    .theme
                    .as_ref()
                    .filter(|(old, _)| old == &theme)
                    .map_or_else(PackId::allocate, |(_, id)| id.clone());
                (theme, id)
            });
    }
}

impl RepositorySource for BuiltinRepositorySource {
    fn load_packs(&mut self) -> io::Result<Vec<Pack>> {
        let state = self.state.borrow();
        let mut packs = vec![Pack::with_id(
            state.pomme_id.clone(),
            builtin_location("pomme", Component::text("Pomme embedded assets")),
            // Embedded resources ship with this client, not an external format.
            computed_metadata("Pomme embedded assets", PackCompat::Compatible),
            builtin_enablement(true),
            Rc::new(EmbeddedResourcesSupplier::new()),
        )];
        if let Some(installation) = &state.installation {
            let input = &installation.location;
            let load = || -> io::Result<Pack> {
                let assets = input.extracted_root.join("assets");
                std::fs::read_dir(&assets).map_err(|error| {
                    path_error("open installed extracted assets", &assets, error)
                })?;
                let resources = Rc::new(VanillaResourcesSupplier::new(
                    input.extracted_root.clone(),
                    &input.index_file,
                    &input.objects_dir,
                    vanilla_metadata(installation.verified_format),
                )?);
                let mut location = builtin_location(
                    "vanilla",
                    Component::translate("resourcePack.vanilla.name", Vec::new()),
                );
                location.known_pack_info = installation.verified_format.map(|_| KnownPack {
                    namespace: "minecraft".to_owned(),
                    id: "core".to_owned(),
                    version: input.version.clone(),
                });
                let metadata = Pack::read_metadata(
                    &location,
                    resources.as_ref(),
                    PackType::ClientResources,
                    state.target,
                )?
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Minecraft pack metadata missing despite built-in fallback",
                    )
                })?;
                Ok(Pack::with_id(
                    installation.id.clone(),
                    location,
                    metadata,
                    builtin_enablement(true),
                    resources,
                ))
            };
            packs.push(load().map_err(|error| {
                path_error(
                    &format!("load Minecraft installation {:?}", input.version),
                    &input.extracted_root,
                    error,
                )
            })?);
        }
        if let Some((theme, id)) = &state.theme {
            let mount = theme.mount().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("prepare theme mapping: {error}"),
                )
            })?;
            if let Some(mount) = mount {
                match std::fs::read_dir(mount.root) {
                    Ok(_) => {
                        let resources = MappedResourcesSupplier::new(
                            mount.root.to_owned(),
                            mount.namespace.to_owned(),
                            mount.remappings,
                        )
                        .map_err(|error| path_error("prepare theme mapping", mount.root, error))?;
                        packs.push(Pack::with_id(
                            id.clone(),
                            builtin_location(mount.id, Component::text(mount.name)),
                            computed_metadata(mount.name, PackCompat::Unknown),
                            builtin_enablement(false),
                            Rc::new(resources),
                        ));
                    }
                    // Keep the definition ID: later availability isn't a new theme.
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(path_error("open theme directory", mount.root, error));
                    }
                }
            }
        }
        Ok(packs)
    }
}

fn builtin_location(id: &str, title: Component) -> PackLocationInfo {
    PackLocationInfo {
        id: id.to_owned(),
        title,
        source: PackSource::built_in(),
        known_pack_info: None,
    }
}

/// COMPAT: Pomme's intentional fixed bootstrap prefix, not Java's Vanilla
/// fixed flag (false). Sorted required insertion is pomme, theme/*, vanilla;
/// the unfixed theme lets vanilla insert after pomme but before the theme.
/// Explicit repository mutations can still override this default ordering.
fn builtin_enablement(fixed_position: bool) -> PackEnablementConfig {
    PackEnablementConfig {
        required: true,
        default_position: PackPosition::Bottom,
        fixed_position,
    }
}

fn computed_metadata(description: &str, compatibility: PackCompat) -> PackMetadata {
    PackMetadata {
        description: Component::text(description),
        compatibility,
        requested_features: FeatureFlagSet::empty(),
        overlays: Vec::new(),
    }
}

fn vanilla_metadata(format: Option<PackFormat>) -> Map<String, Value> {
    let mut pack = Map::new();
    pack.insert(
        "description".to_owned(),
        json!({"translate": "resourcePack.vanilla.description"}),
    );
    if let Some(format) = format {
        if format.major() < 15 {
            // Modern transition ranges require pack_format >= 15. Standalone
            // legacy declarations encode only minor 0, as read from the legacy
            // version.json resource field; nonzero minors are not representable.
            pack.insert("pack_format".to_owned(), json!(format.major()));
        } else {
            // Vanilla PackFormat.minorRange: [this, (major, Integer.MAX_VALUE)].
            pack.insert(
                "min_format".to_owned(),
                json!([format.major(), format.minor()]),
            );
            pack.insert(
                "max_format".to_owned(),
                json!([format.major(), MAX_COMPONENT]),
            );
            if format.major() <= last_pre_minor_format(PackType::ClientResources) {
                pack.insert("pack_format".to_owned(), json!(format.major()));
                pack.insert("supported_formats".to_owned(), json!(format.major()));
            }
        }
    }
    // Unknown installation: description-only fallback, no invented format.
    // The strict factory still validates present external metadata and overlays.
    Map::from_iter([(PACK_SECTION.to_owned(), Value::Object(pack))])
}

// Installed JAR version.json, distinct from selected-pack metadata.
const VERSION_METADATA_FILE: &str = "version.json";
const VERSION_RESOURCE_FORMAT: &str = "resource";
const VERSION_RESOURCE_MAJOR: &str = "resource_major";
const VERSION_RESOURCE_MINOR: &str = "resource_minor";

// Same startup bound as the installer's readiness check, without coupling APIs.
const MAX_VERSION_JSON_SIZE: u64 = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
#[error("cannot read resource pack format for {version} from {path}: {source}")]
struct InstalledFormatError {
    path: PathBuf,
    version: String,
    #[source]
    source: FormatReadError,
}

#[derive(Debug, thiserror::Error)]
enum FormatReadError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid version.json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("version.json exceeds the {MAX_VERSION_JSON_SIZE}-byte limit")]
    TooLarge,
    #[error("version.json identifies a different version: {0}")]
    VersionMismatch(String),
    #[error("invalid version.json pack_version: {0}")]
    InvalidFormat(&'static str),
}

/// Read the selected installed JAR, never selected packs or protocol metadata.
/// Missing first-run assets and corrupt metadata are both explicit errors.
fn read_installed_pack_format(
    path: &Path,
    expected_version: &str,
) -> Result<PackFormat, InstalledFormatError> {
    read_format(path, expected_version).map_err(|source| InstalledFormatError {
        path: path.to_owned(),
        version: expected_version.to_owned(),
        source,
    })
}

fn read_format(path: &Path, expected_version: &str) -> Result<PackFormat, FormatReadError> {
    #[derive(Deserialize)]
    struct JarVersion {
        id: String,
        pack_version: Value,
    }

    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let entry = archive.by_name(VERSION_METADATA_FILE)?;
    if entry.size() > MAX_VERSION_JSON_SIZE {
        return Err(FormatReadError::TooLarge);
    }
    let mut bytes = Vec::new();
    entry
        .take(MAX_VERSION_JSON_SIZE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_VERSION_JSON_SIZE {
        return Err(FormatReadError::TooLarge);
    }
    let metadata: JarVersion = serde_json::from_slice(&bytes)?;
    if metadata.id != expected_version {
        return Err(FormatReadError::VersionMismatch(metadata.id));
    }
    let version = &metadata.pack_version;
    let component = |key| {
        version
            .get(key)
            .and_then(format_component)
            .ok_or(FormatReadError::InvalidFormat(key))
    };
    let (major, minor) = if version.get(VERSION_RESOURCE_MAJOR).is_some()
        || version.get(VERSION_RESOURCE_MINOR).is_some()
    {
        (
            component(VERSION_RESOURCE_MAJOR)?,
            component(VERSION_RESOURCE_MINOR)?,
        )
    } else {
        (component(VERSION_RESOURCE_FORMAT)?, 0)
    };
    PackFormat::new(major, minor).ok_or(FormatReadError::InvalidFormat("resource components"))
}
