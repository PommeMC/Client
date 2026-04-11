use std::cmp::Ordering;
use std::fmt;
use std::path::{Path, PathBuf};

use azalea_buf::AzBuf;

pub const NAMESPACE_SEPARATOR: char = ':';
pub const DEFAULT_NAMESPACE: &str = "minecraft";
pub const REALMS_NAMESPACE: &str = "realms";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentifierError {
    InvalidNamespace { namespace: String, path: String },
    InvalidPath { namespace: String, path: String },
    EmptyPath,
}

impl fmt::Display for IdentifierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentifierError::InvalidNamespace { namespace, path } => {
                write!(
                    f,
                    "Non [a-z0-9_.-] character in namespace of identifier: {}:{}",
                    namespace, path
                )
            }
            IdentifierError::InvalidPath { namespace, path } => {
                write!(
                    f,
                    "Non [a-z0-9/._-] character in path of identifier: {}:{}",
                    namespace, path
                )
            }
            IdentifierError::EmptyPath => write!(f, "Identifier path cannot be empty"),
        }
    }
}

impl std::error::Error for IdentifierError {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Identifier {
    namespace: String,
    path: String,
}

impl Identifier {
    pub fn new(raw: &str) -> Self {
        Self::parse(raw).expect("invalid identifier")
    }

    pub fn to_azalea_packet(&self) -> azalea_registry::identifier::Identifier {
        azalea_registry::identifier::Identifier::new(self.to_language_key())
    }

    pub fn from_namespace_and_path(
        namespace: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<Self, IdentifierError> {
        let namespace = namespace.into();
        let path = path.into();
        Self::create_untrusted(namespace, path)
    }

    pub fn parse(identifier: &str) -> Result<Self, IdentifierError> {
        Self::by_separator(identifier, NAMESPACE_SEPARATOR)
    }

    pub fn with_default_namespace(path: impl Into<String>) -> Result<Self, IdentifierError> {
        let path = path.into();
        if path.is_empty() {
            return Err(IdentifierError::EmptyPath);
        }

        if !Self::is_valid_path(&path) {
            return Err(IdentifierError::InvalidPath {
                namespace: DEFAULT_NAMESPACE.to_string(),
                path,
            });
        }

        Ok(Self {
            namespace: DEFAULT_NAMESPACE.to_string(),
            path,
        })
    }

    pub fn try_parse(identifier: &str) -> Option<Self> {
        Self::parse(identifier).ok()
    }

    pub fn try_build(namespace: &str, path: &str) -> Option<Self> {
        if Self::is_valid_namespace(namespace) && Self::is_valid_path(path) && !path.is_empty() {
            Some(Self {
                namespace: namespace.to_string(),
                path: path.to_string(),
            })
        } else {
            None
        }
    }

    pub fn by_separator(identifier: &str, separator: char) -> Result<Self, IdentifierError> {
        if let Some(separator_index) = identifier.find(separator) {
            let path = identifier[separator_index + 1..].to_string();
            if path.is_empty() {
                return Err(IdentifierError::EmptyPath);
            }

            if separator_index != 0 {
                let namespace = identifier[..separator_index].to_string();
                Self::create_untrusted(namespace, path)
            } else {
                Self::with_default_namespace(path)
            }
        } else {
            Self::with_default_namespace(identifier.to_string())
        }
    }

    pub fn try_by_separator(identifier: &str, separator: char) -> Option<Self> {
        Self::by_separator(identifier, separator).ok()
    }

    pub fn get_path(&self) -> &str {
        &self.path
    }

    pub fn get_namespace(&self) -> &str {
        &self.namespace
    }

    pub fn with_path(&self, new_path: impl Into<String>) -> Result<Self, IdentifierError> {
        let new_path = new_path.into();
        Self::create_untrusted(self.namespace.clone(), new_path)
    }

    pub fn with_path_fn<F>(&self, modifier: F) -> Result<Self, IdentifierError>
    where
        F: FnOnce(&str) -> String,
    {
        self.with_path(modifier(&self.path))
    }

    pub fn with_prefix(&self, prefix: &str) -> Result<Self, IdentifierError> {
        self.with_path(format!("{prefix}{}", self.path))
    }

    pub fn with_suffix(&self, suffix: &str) -> Result<Self, IdentifierError> {
        self.with_path(format!("{}{suffix}", self.path))
    }

    pub fn resolve_against(&self, root: &Path) -> PathBuf {
        root.join(&self.namespace).join(&self.path)
    }

    pub fn to_debug_file_name(&self) -> String {
        self.to_string().replace('/', "_").replace(':', "_")
    }

    pub fn to_language_key(&self) -> String {
        format!("{}.{}", self.namespace, self.path.replace('/', "."))
    }

    pub fn to_short_language_key(&self) -> String {
        if self.namespace == DEFAULT_NAMESPACE {
            self.path.replace('/', ".")
        } else {
            self.to_language_key()
        }
    }

    pub fn to_short_string(&self) -> String {
        if self.namespace == DEFAULT_NAMESPACE {
            self.path.clone()
        } else {
            self.to_string()
        }
    }

    pub fn to_language_key_with_prefix(&self, prefix: &str) -> String {
        format!("{prefix}.{}", self.to_language_key())
    }

    pub fn to_language_key_with_prefix_suffix(&self, prefix: &str, suffix: &str) -> String {
        format!("{prefix}.{}.{}", self.to_language_key(), suffix)
    }

    pub fn is_allowed_in_identifier(c: char) -> bool {
        c.is_ascii_digit() || c.is_ascii_lowercase() || matches!(c, '_' | ':' | '/' | '.' | '-')
    }

    pub fn is_valid_path(path: &str) -> bool {
        !path.is_empty() && path.chars().all(Self::valid_path_char)
    }

    pub fn is_valid_namespace(namespace: &str) -> bool {
        namespace != ".."
            && !namespace.is_empty()
            && namespace.chars().all(Self::valid_namespace_char)
    }

    pub fn valid_path_char(c: char) -> bool {
        matches!(c, '_' | '-' | '/' | '.') || c.is_ascii_lowercase() || c.is_ascii_digit()
    }

    pub fn valid_namespace_char(c: char) -> bool {
        matches!(c, '_' | '-' | '.') || c.is_ascii_lowercase() || c.is_ascii_digit()
    }

    fn create_untrusted(namespace: String, path: String) -> Result<Self, IdentifierError> {
        if !Self::is_valid_namespace(&namespace) {
            return Err(IdentifierError::InvalidNamespace { namespace, path });
        }
        if !Self::is_valid_path(&path) {
            return Err(IdentifierError::InvalidPath { namespace, path });
        }

        Ok(Self { namespace, path })
    }
}

impl fmt::Display for Identifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.path)
    }
}

impl std::str::FromStr for Identifier {
    type Err = IdentifierError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl PartialOrd for Identifier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(
            self.path
                .cmp(&other.path)
                .then_with(|| self.namespace.cmp(&other.namespace)),
        )
    }
}

impl Ord for Identifier {
    fn cmp(&self, other: &Self) -> Ordering {
        self.path
            .cmp(&other.path)
            .then_with(|| self.namespace.cmp(&other.namespace))
    }
}

impl AzBuf for Identifier {
    fn azalea_read(buf: &mut std::io::Cursor<&[u8]>) -> Result<Self, azalea_buf::BufReadError> {
        let location_string = String::azalea_read(buf)?;
        Ok(Identifier::new(&location_string))
    }
    fn azalea_write(&self, buf: &mut impl std::io::Write) -> std::io::Result<()> {
        self.to_string().azalea_write(buf)
    }
}
