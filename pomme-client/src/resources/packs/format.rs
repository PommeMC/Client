use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use super::metadata;

pub(super) const MAX_COMPONENT: u32 = i32::MAX as u32;

/// Lexicographically ordered Java non-negative integer components.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PackFormat {
    major: u32,
    minor: u32,
}

impl PackFormat {
    pub(super) fn new(major: u32, minor: u32) -> Option<Self> {
        (major <= MAX_COMPONENT && minor <= MAX_COMPONENT).then_some(Self { major, minor })
    }

    pub(super) fn major(self) -> u32 {
        self.major
    }

    pub(super) fn minor(self) -> u32 {
        self.minor
    }
}

impl fmt::Display for PackFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Inclusive, ordered bounds; constructed only from validated metadata.
#[derive(Clone, Copy, Debug)]
pub(super) struct PackFormatRange {
    min: PackFormat,
    max: PackFormat,
}

impl PackFormatRange {
    pub(super) fn min(self) -> PackFormat {
        self.min
    }

    pub(super) fn max(self) -> PackFormat {
        self.max
    }

    pub(super) fn new(min: PackFormat, max: PackFormat) -> Option<Self> {
        (min <= max).then_some(Self { min, max })
    }
}

/// Shared validation for installed version.json and selected pack metadata.
pub(super) fn format_component(value: &Value) -> Option<u32> {
    let value = u32::try_from(value.as_u64()?).ok()?;
    (value <= MAX_COMPONENT).then_some(value)
}

// Same startup bound as the installer's readiness check, without coupling APIs.
const MAX_VERSION_JSON_SIZE: u64 = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
#[error("cannot read resource pack format for {version} from {path}: {source}")]
pub(super) struct InstalledFormatError {
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
pub(super) fn read_installed_pack_format(
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
    let entry = archive.by_name(metadata::VERSION_METADATA_FILE)?;
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
    let (major, minor) = if version.get(metadata::VERSION_RESOURCE_MAJOR).is_some()
        || version.get(metadata::VERSION_RESOURCE_MINOR).is_some()
    {
        (
            component(metadata::VERSION_RESOURCE_MAJOR)?,
            component(metadata::VERSION_RESOURCE_MINOR)?,
        )
    } else {
        (component(metadata::VERSION_RESOURCE_FORMAT)?, 0)
    };
    PackFormat::new(major, minor).ok_or(FormatReadError::InvalidFormat("resource components"))
}
