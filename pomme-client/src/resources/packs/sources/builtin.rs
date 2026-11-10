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
    Pack, PackCompat, PackEnablementConfig, PackFormat, PackId, PackLocationInfo, PackMetadata,
    PackPosition,
};
use crate::resources::{ResourceError, ResourceId};

#[derive(PartialEq, Eq)]
struct BedrockUi {
    root: PathBuf,
    aliases: BTreeMap<ResourceId, PathBuf>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ThemeSpecification {
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
    fn mount(&self) -> Result<ThemeMount<'_>, ResourceError> {
        let (root, id, name, directory) = match self {
            Self::Pomme { root } => (root, "theme/pomme", "Pomme theme assets", "panoramas"),
            Self::Bedrock { root } => {
                (root, "theme/bedrock", "Bedrock theme assets", "textures/ui")
            }
        };
        let remappings = (0..6)
            .map(|face| {
                Ok((
                    panorama_id(face)?,
                    // `/` on every platform: mapped targets reject `\`.
                    PathBuf::from(format!("{directory}/panorama_{face}.png")),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, ResourceError>>()?;
        Ok(ThemeMount {
            root,
            id,
            name,
            namespace: "minecraft",
            remappings,
        })
    }
}

const PANORAMA_PATH_PREFIX: &str = "textures/gui/title/background/panorama_";

fn panorama_id(face: u32) -> Result<ResourceId, ResourceError> {
    ResourceId::from_parts("minecraft", &format!("{PANORAMA_PATH_PREFIX}{face}.png"))
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct InstallationLocation {
    pub version: String,
    /// The jar's `version.json`, extracted beside its assets.
    pub version_metadata: PathBuf,
    /// Parent of the extracted `assets` directory.
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
    bedrock_ui: Option<(BedrockUi, PackId)>,
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
                bedrock_ui: None,
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
            match read_installed_pack_format(&location.version_metadata, &location.version) {
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

    pub(crate) fn set_theme(&self, theme: Option<ThemeSpecification>) {
        let mut state = self.state.borrow_mut();
        state.theme = theme.map(|theme| {
            let id = state
                .theme
                .as_ref()
                .filter(|(old, _)| old == &theme)
                .map_or_else(PackId::allocate, |(_, id)| id.clone());
            (theme, id)
        });
    }

    /// The menu preloads these sprites regardless of the selected panorama.
    /// A separate namespace keeps Bedrock's title out of Java's asset paths;
    /// `aliases` name the files whose capitals no id can spell.
    pub(crate) fn set_bedrock_ui(&self, root: PathBuf, aliases: BTreeMap<ResourceId, PathBuf>) {
        let mut state = self.state.borrow_mut();
        let ui = BedrockUi { root, aliases };
        let id = state
            .bedrock_ui
            .as_ref()
            .filter(|(old, _)| old == &ui)
            .map_or_else(PackId::allocate, |(_, id)| id.clone());
        state.bedrock_ui = Some((ui, id));
    }
}

impl RepositorySource for BuiltinRepositorySource {
    /// The embedded pack always loads. An installation or theme that can't be
    /// read is logged and left out, as vanilla's `FixedPathPackResources`
    /// builder skips roots it can't use.
    fn load_packs(&mut self) -> io::Result<Vec<Pack>> {
        let state = self.state.borrow();
        let mut packs = vec![Pack::with_id(
            state.pomme_id.clone(),
            builtin_location("pomme", Component::text("Pomme embedded assets")),
            // Embedded resources ship with this client, not an external format.
            computed_metadata("Pomme embedded assets", PackCompat::Compatible),
            required(PackPosition::Bottom, true),
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
                ));
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
                    // `ClientPackSource.VANILLA_SELECTION_CONFIG`.
                    required(PackPosition::Bottom, false),
                    resources,
                ))
            };
            match load() {
                Ok(pack) => packs.push(pack),
                Err(error) => tracing::warn!(
                    version = %input.version,
                    path = %input.extracted_root.display(),
                    %error,
                    "Failed to load the Minecraft installation's resources"
                ),
            }
        }
        if let Some((BedrockUi { root, aliases }, id)) = &state.bedrock_ui
            && root.is_dir()
        {
            match MappedResourcesSupplier::new(root.clone(), "bedrock".to_owned(), aliases.clone())
            {
                Ok(resources) => packs.push(Pack::with_id(
                    id.clone(),
                    builtin_location("pomme/bedrock-ui", Component::text("Bedrock UI assets")),
                    computed_metadata("Bedrock UI assets", PackCompat::Unknown),
                    required(PackPosition::Bottom, false),
                    Rc::new(resources),
                )),
                Err(error) => tracing::warn!(
                    path = %root.display(),
                    %error,
                    "Failed to load Bedrock UI resources"
                ),
            }
        }
        // TODO: vanilla `ClientPackSource` also offers the index's
        // `minecraft:resourcepacks/*` zips (programmer_art, high_contrast) as
        // optional built-in packs.
        if let Some((theme, id)) = &state.theme {
            match load_theme(theme, id) {
                Ok(Some(pack)) => packs.push(pack),
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "Failed to load theme resources"),
            }
        }
        Ok(packs)
    }
}

/// `None` until all six of the theme's faces are downloaded, so its panorama
/// never mixes with the faces of the packs below; the definition keeps its
/// ID, since later availability isn't a new theme.
fn load_theme(theme: &ThemeSpecification, id: &PackId) -> io::Result<Option<Pack>> {
    let mount = theme
        .mount()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if !mount
        .remappings
        .values()
        .all(|face| mount.root.join(face).is_file())
    {
        return Ok(None);
    }
    let resources = MappedResourcesSupplier::aliases_only(
        mount.root.to_owned(),
        mount.namespace.to_owned(),
        mount.remappings,
    )
    .map_err(|error| path_error("prepare theme mapping", mount.root, error))?;
    Ok(Some(Pack::with_id(
        id.clone(),
        builtin_location(mount.id, Component::text(mount.name)),
        computed_metadata(mount.name, PackCompat::Unknown),
        required(PackPosition::Top, false),
        Rc::new(resources),
    )))
}

fn builtin_location(id: &str, title: Component) -> PackLocationInfo {
    PackLocationInfo {
        id: id.to_owned(),
        title,
        known_pack_info: None,
    }
}

/// The Pomme-only embedded pack is fixed at the bottom, below vanilla, which
/// inserts just above it. The theme goes in at the top, so it lands above
/// vanilla however the enabled list got there, including after a theme switch.
fn required(default_position: PackPosition, fixed_position: bool) -> PackEnablementConfig {
    PackEnablementConfig {
        required: true,
        default_position,
        fixed_position,
    }
}

fn computed_metadata(description: &str, compatibility: PackCompat) -> PackMetadata {
    PackMetadata {
        description: Component::text(description),
        compatibility,
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

// Installed version.json, distinct from selected-pack metadata.
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
    #[error("invalid version.json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("version.json exceeds the {MAX_VERSION_JSON_SIZE}-byte limit")]
    TooLarge,
    #[error("version.json identifies a different version: {0}")]
    VersionMismatch(String),
    #[error("invalid version.json pack_version: {0}")]
    InvalidFormat(&'static str),
}

/// Read the installed version.json, never selected packs or protocol metadata.
/// Missing and corrupt metadata are both explicit errors, which leave the
/// format unknown.
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

    let mut bytes = Vec::new();
    std::fs::File::open(path)?
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

#[cfg(test)]
mod repository_source_tests {
    use super::super::tests;
    use super::{
        BuiltinRepositorySource, MappedResourcesSupplier, PackFormat, ThemeSpecification,
        read_installed_pack_format,
    };

    #[test]
    fn test_load_packs() {
        let mut source = BuiltinRepositorySource::new(None);
        tests::test_load_packs(&mut source, &["pomme"]);
    }

    #[test]
    fn installed_format_reads_either_pack_version_layout() {
        let path =
            std::env::temp_dir().join(format!("pomme-version-json-{}.json", std::process::id()));
        for (pack_version, (major, minor)) in [
            (r#"{"resource": 34}"#, (34, 0)),
            (r#"{"resource_major": 75, "resource_minor": 1}"#, (75, 1)),
        ] {
            let json = format!(r#"{{"id": "26.2", "pack_version": {pack_version}}}"#);
            std::fs::write(&path, json).unwrap();
            assert_eq!(
                read_installed_pack_format(&path, "26.2").unwrap(),
                PackFormat::new(major, minor).unwrap()
            );
        }
        assert!(read_installed_pack_format(&path, "26.1").is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(read_installed_pack_format(&path, "26.2").is_err());
    }

    #[test]
    fn every_theme_mount_prepares_its_mapping() {
        let root = std::env::temp_dir();
        for theme in [
            ThemeSpecification::Pomme { root: root.clone() },
            ThemeSpecification::Bedrock { root: root.clone() },
        ] {
            let mount = theme.mount().unwrap();
            MappedResourcesSupplier::new(
                mount.root.to_owned(),
                mount.namespace.to_owned(),
                mount.remappings,
            )
            .unwrap_or_else(|error| panic!("{}: {error}", mount.id));
        }
    }

    #[test]
    fn a_theme_loads_only_with_all_six_faces() {
        let temp = crate::resources::test_dir::TempDir::new().unwrap();
        let faces = temp.0.join("panoramas");
        std::fs::create_dir_all(&faces).unwrap();
        let theme = ThemeSpecification::Pomme {
            root: temp.0.clone(),
        };
        let id = crate::resources::packs::PackId::allocate();
        for face in 0..5 {
            std::fs::write(faces.join(format!("panorama_{face}.png")), b"face").unwrap();
        }
        assert!(super::load_theme(&theme, &id).unwrap().is_none());
        std::fs::write(faces.join("panorama_5.png"), b"face").unwrap();
        assert!(super::load_theme(&theme, &id).unwrap().is_some());
    }
}
