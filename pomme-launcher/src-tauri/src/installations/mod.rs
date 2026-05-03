pub mod fs;
pub mod registry;

use rand::RngExt;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::num::NonZeroU64;
use std::path::Path;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::commands::fetch_versions;
use crate::installations;
use crate::storage::installations_dir;

const MAX_NAME_LENGTH: usize = 35;
#[cfg(target_os = "windows")]
const RESERVED_DIRNAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
#[cfg(target_os = "windows")]
const FORBIDDEN_CHAR: &[char] = &[':', '*', '?', '"', '<', '>', '|'];

const INSTALLATIONS_LATEST_RELEASE_INDEX: usize = 0;
const INSTALLATIONS_LATEST_SNAPSHOT_INDEX: usize = 1;

#[derive(Debug, thiserror::Error, Serialize, specta::Type)]
#[serde(tag = "kind", content = "detail")]
pub enum InstallationError {
    #[error("Invalid name")]
    InvalidName,
    #[error("Name too long, max {0} characters")]
    NameTooLong(usize),
    #[error("Invalid path")]
    InvalidPath,
    #[error("Invalid character in directory: {0}")]
    InvalidCharacter(char),
    #[error("Reserved name: {0}")]
    ReservedName(String),
    #[error("Directory already exists")]
    DirectoryAlreadyExists,
    #[error("Installation {0} not found")]
    InstallNotFound(Id),
    #[error("IO error: {0}")]
    Io(String),
    #[error("JSON error: {0}")]
    Json(String),
    #[error("Error: {0}")]
    Other(String),
}
impl From<std::io::Error> for InstallationError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}
impl From<serde_json::Error> for InstallationError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e.to_string())
    }
}
impl From<String> for InstallationError {
    fn from(value: String) -> Self {
        Self::Other(value)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, specta::Type)]
pub struct Id(String);
impl Id {
    pub fn new(created_at: u64) -> Self {
        const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut rng = rand::rng();
        let mut suffix = [0u8; 4];
        for b in &mut suffix {
            *b = CHARS[rng.random_range(0..CHARS.len())];
        }
        let suffix = std::str::from_utf8(&suffix).unwrap();
        Id(format!("{created_at}-{suffix}"))
    }

    pub fn latest_release() -> Self {
        Id("latest-release".to_string())
    }

    pub fn latest_snapshot() -> Self {
        Id("latest-snapshot".to_string())
    }
}
impl From<String> for Id {
    fn from(value: String) -> Self {
        Id(value)
    }
}
impl Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, specta::Type)]
pub struct Name(String);
impl TryFrom<String> for Name {
    type Error = InstallationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err(InstallationError::InvalidName);
        }
        if value.len() > MAX_NAME_LENGTH {
            return Err(InstallationError::NameTooLong(MAX_NAME_LENGTH));
        }
        Ok(Name(value))
    }
}
impl Name {
    pub fn latest_release() -> Self {
        Name("Latest Release".to_string())
    }
    pub fn latest_snapshot() -> Self {
        Name("Latest Snapshot".to_string())
    }
}
impl From<Name> for String {
    fn from(value: Name) -> Self {
        value.0
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, specta::Type)]
pub struct Version(String);
impl From<String> for Version {
    fn from(value: String) -> Self {
        Version(value)
    }
}
impl Version {
    pub async fn try_latest_release() -> Result<Self, InstallationError> {
        let latest = &fetch_versions().await?.latest.release;
        Ok(Version(latest.clone()))
    }
    pub async fn try_latest_snapshot() -> Result<Self, InstallationError> {
        let latest = &fetch_versions().await?.latest.snapshot;
        Ok(Version(latest.clone()))
    }
}
impl From<Version> for String {
    fn from(value: Version) -> Self {
        value.0
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, specta::Type)]
pub struct TimeStamp(NonZeroU64);
impl TimeStamp {
    pub fn now() -> Self {
        TimeStamp(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|d| NonZeroU64::new(d.as_millis() as u64))
                .unwrap_or(NonZeroU64::MIN),
        )
    }
}
impl From<TimeStamp> for u64 {
    fn from(value: TimeStamp) -> Self {
        value.0.get()
    }
}
impl From<NonZeroU64> for TimeStamp {
    fn from(value: NonZeroU64) -> Self {
        TimeStamp(value)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, specta::Type)]
pub struct Directory(PathBuf);
impl TryFrom<String> for Directory {
    type Error = InstallationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err(InstallationError::InvalidPath);
        }
        let path = PathBuf::from(&value);
        let path = if path.is_absolute() {
            path
        } else {
            installations_dir().join(path)
        };
        if path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(InstallationError::InvalidPath);
        }
        #[cfg(target_os = "windows")]
        Self::validate_directory_os(&path)?;
        Ok(Directory(path))
    }
}
impl Directory {
    pub fn latest() -> Self {
        Directory(installations_dir().join("default"))
    }

    #[cfg(target_os = "windows")]
    pub fn validate_directory_os(path: &Path) -> Result<(), InstallationError> {
        for component in path.components() {
            if let std::path::Component::Normal(name) = component {
                let name_str = name.to_string_lossy();

                let stem = name_str.split('.').next().unwrap_or("");
                if RESERVED_DIRNAMES
                    .iter()
                    .any(|r| r.eq_ignore_ascii_case(stem))
                {
                    return Err(InstallationError::ReservedName(name_str.into_owned()));
                }

                if name_str.ends_with(' ') || name_str.ends_with('.') {
                    return Err(InstallationError::InvalidPath);
                }

                if let Some(c) = name_str.chars().find(|c| FORBIDDEN_CHAR.contains(c)) {
                    return Err(InstallationError::InvalidCharacter(c));
                }
            }
        }
        Ok(())
    }
}
impl AsRef<Path> for Directory {
    fn as_ref(&self) -> &Path {
        self.0.as_ref()
    }
}
impl From<Directory> for String {
    fn from(value: Directory) -> Self {
        value.0.to_string_lossy().into_owned()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, specta::Type)]
pub struct Installation {
    pub id: Id,
    pub name: Name,
    pub version: Version,
    pub last_played: Option<NonZeroU64>,
    pub created_at: TimeStamp,
    pub directory: Directory,
    pub width: u32,
    pub height: u32,
    pub is_latest: bool,
}
impl Installation {
    pub async fn try_latest_release() -> Result<Self, InstallationError> {
        Ok(Self {
            id: Id::latest_release(),
            name: Name::latest_release(),
            version: Version::try_latest_release().await?,
            last_played: None,
            created_at: TimeStamp::now(),
            directory: Directory::latest(),
            width: 854,
            height: 480,
            is_latest: true,
        })
    }

    pub async fn try_latest_snapshot() -> Result<Self, InstallationError> {
        Ok(Self {
            id: Id::latest_snapshot(),
            name: Name::latest_snapshot(),
            version: Version::try_latest_snapshot().await?,
            last_played: None,
            created_at: TimeStamp::now(),
            directory: Directory::latest(),
            width: 854,
            height: 480,
            is_latest: true,
        })
    }
}
impl TryFrom<InstallationDraft> for Installation {
    type Error = InstallationError;
    fn try_from(value: InstallationDraft) -> Result<Self, Self::Error> {
        let ts = TimeStamp::now();
        let millis: u64 = ts.clone().into();

        Ok(Self {
            id: Id::new(millis),
            last_played: None,
            created_at: ts,
            is_latest: false,

            name: value.name.try_into()?,
            version: value.version.into(),
            directory: value.directory.try_into()?,
            width: value.width,
            height: value.height,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, specta::Type)]
pub struct InstallationDraft {
    pub name: String,
    pub version: String,
    pub directory: String,
    pub width: u32,
    pub height: u32,
}
impl From<Installation> for InstallationDraft {
    fn from(value: Installation) -> Self {
        Self {
            directory: value.directory.into(),
            height: value.height,
            width: value.width,
            name: value.name.into(),
            version: value.version.into(),
        }
    }
}

pub async fn load_installations() -> Result<Vec<Installation>, InstallationError> {
    let installs = {
        let mut installs = registry::load()?;
        if let Ok(latest_release_version) = installations::Version::try_latest_release().await {
            if let Some(existing) = installs.iter_mut().find(|i| i.id == Id::latest_release()) {
                existing.version = latest_release_version;
            } else {
                installs.insert(
                    INSTALLATIONS_LATEST_RELEASE_INDEX,
                    Installation::try_latest_release().await?,
                );
            }
        }

        if let Ok(latest_snapshot_version) = installations::Version::try_latest_snapshot().await {
            if let Some(existing) = installs.iter_mut().find(|i| i.id == Id::latest_snapshot()) {
                existing.version = latest_snapshot_version;
            } else {
                installs.insert(
                    INSTALLATIONS_LATEST_SNAPSHOT_INDEX,
                    Installation::try_latest_snapshot().await?,
                );
            }
        }

        if let Some(err) = registry::save(&installs).err() {
            log::warn!("Failed to save registry after loading installations: {err}");
        }
        installs
    };

    for install in &installs {
        fs::ensure_install_fs(install)?;
    }
    Ok(installs)
}

pub async fn create_installation(
    payload: InstallationDraft,
) -> Result<Installation, InstallationError> {
    let install: Installation = payload.try_into()?;
    registry::register(install.clone())?;
    if let Err(e) = fs::ensure_install_fs(&install) {
        if let Err(rollback_err) = registry::unregister(&install.id) {
            log::warn!(
                "Failed to roll back registry entry for `{}`: {}",
                install.id,
                rollback_err
            );
        }
        return Err(e);
    }
    Ok(install)
}

pub async fn delete_installation(id: String) -> Result<(), InstallationError> {
    let id: Id = id.into();
    let install = {
        let install = registry::find_by_id(&id)?;
        if install.is_latest {
            return Err(InstallationError::Other(
                "Cannot delete a default installation".to_string(),
            ));
        }
        registry::unregister(&id)?;
        install
    };
    if let Err(e) = fs::remove_install_fs(&install.directory) {
        log::warn!(
            "Failed to delete installation directory for `{}`: {}",
            install.id,
            e
        );
    }
    Ok(())
}

pub async fn duplicate_installation(
    old_id: String,
    payload: InstallationDraft,
) -> Result<Installation, InstallationError> {
    let old_id: Id = old_id.into();
    let (old_dir, new_install) = {
        let old_install = registry::find_by_id(&old_id)?;
        let new_install: Installation = payload.try_into()?;
        registry::register(new_install.clone())?;
        (old_install.directory.clone(), new_install)
    };
    if let Err(e) = fs::duplicate_install_fs(&old_dir, &new_install.directory) {
        if let Err(rollback_err) = registry::unregister(&new_install.id) {
            log::warn!(
                "Failed to roll back registry entry for `{}`: {}",
                new_install.id,
                rollback_err
            );
        }
        return Err(e);
    }
    if let Err(e) = fs::ensure_install_fs(&new_install) {
        log::warn!(
            "Failed to ensure fs for duplicated installation `{}`: {}",
            new_install.id,
            e
        );
    }
    Ok(new_install)
}

pub async fn edit_installation(
    id: String,
    payload: InstallationDraft,
) -> Result<Installation, InstallationError> {
    let id: Id = id.into();
    let (old_dir, old_install, new_install) = {
        let old_install = registry::find_by_id(&id)?;
        let old_dir = old_install.directory.clone();
        let payload = if id == Id::latest_release() || id == Id::latest_snapshot() {
            InstallationDraft {
                name: old_install.name.clone().into(),
                ..payload
            }
        } else {
            payload
        };
        let new_install = registry::update(&id, payload)?;
        (old_dir, old_install, new_install)
    };
    if new_install.directory != old_dir
        && let Err(e) = fs::move_install_fs(&old_dir, &new_install.directory)
    {
        if let Err(rollback_err) = registry::update(&id, old_install.into()) {
            log::warn!(
                "Failed to roll back registry entry for `{}`: {}",
                new_install.id,
                rollback_err
            );
        }
        return Err(e);
    }
    Ok(new_install)
}
