pub mod fs;
pub mod registry;

use rand::RngExt;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::commands::fetch_versions;

const MAX_NAME_LENGTH: usize = 35;
const MAX_DIRNAME_LENGTH: usize = 60;
#[cfg(target_os = "windows")]
const RESERVED_DIRNAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
#[cfg(target_os = "windows")]
const FORBIDDEN_CHAR: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "detail")]
pub enum InstallationError {
    #[error("Invalid name")]
    InvalidName,
    #[error("Name too long, max {0} characters")]
    NameTooLong(usize),
    #[error("Invalid directory")]
    InvalidDirectory,
    #[error("Directory too long, max {0} characters")]
    DirectoryTooLong(usize),
    #[error("Invalid character in directory: {0}")]
    InvalidCharacter(char),
    #[cfg(target_os = "windows")]
    #[error("Reserved name: {0}")]
    ReservedName(String),
    #[error("Trailing space or dot")]
    TrailingDot,
    #[error("Directory already exists")]
    DirectoryAlreadyExists,
    #[error("IO error: {0}")]
    Io(String),
    #[error("JSON error: {0}")]
    Json(String),
    #[error("Error: {0}")]
    Other(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
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
        Id(format!("latest-release"))
    }

    pub fn latest_snapshot() -> Self {
        Id(format!("latest-snapshot"))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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
        Name(format!("Latest Release"))
    }
    pub fn latest_snapshot() -> Self {
        Name(format!("Latest Snapshot"))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
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

#[derive(Serialize, Deserialize, Debug, Clone)]
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Directory(String);
impl TryFrom<String> for Directory {
    type Error = InstallationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() || value == "." || value == ".." {
            return Err(InstallationError::InvalidDirectory);
        }
        if value.len() > MAX_DIRNAME_LENGTH {
            return Err(InstallationError::DirectoryTooLong(MAX_DIRNAME_LENGTH));
        }
        if let Some(c) = value
            .chars()
            .find(|c| *c == '\0' || *c == '/' || *c == '\\')
        {
            return Err(InstallationError::InvalidCharacter(c));
        }
        if value.ends_with('.') {
            return Err(InstallationError::TrailingDot);
        }
        #[cfg(target_os = "windows")]
        Self::validate_directory_os(&value)?;
        Ok(Directory(value))
    }
}
impl Directory {
    pub fn latest() -> Self {
        Directory("default".into())
    }

    #[cfg(target_os = "windows")]
    pub fn validate_directory_os(dir: &str) -> Result<(), InstallationError> {
        let stem = dir.split('.').next().unwrap_or("").to_uppercase();
        if RESERVED_DIRNAMES.contains(&stem.as_str()) {
            return Err(InstallationError::ReservedName(dir.to_string()));
        }

        if let Some(c) = dir.chars().find(|c| FORBIDDEN_CHAR.contains(c)) {
            return Err(InstallationError::InvalidCharacter(c));
        }

        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
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

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NewInstallPayload {
    pub name: String,
    pub version: String,
    pub directory: String,
    pub width: u32,
    pub height: u32,
}

impl TryFrom<NewInstallPayload> for Installation {
    type Error = InstallationError;

    fn try_from(value: NewInstallPayload) -> Result<Self, Self::Error> {
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

impl From<String> for Id {
    fn from(value: String) -> Self {
        Id(value)
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

impl AsRef<Path> for Directory {
    fn as_ref(&self) -> &Path {
        self.0.as_ref()
    }
}
impl AsRef<str> for Directory {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
