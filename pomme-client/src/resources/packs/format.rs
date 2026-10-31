use std::fmt;

use serde_json::Value;

pub(super) const MAX_COMPONENT: u32 = i32::MAX as u32;

/// Lexicographically ordered Java non-negative integer components.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PackFormat {
    major: u32,
    minor: u32,
}

impl PackFormat {
    pub(crate) fn new(major: u32, minor: u32) -> Option<Self> {
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
