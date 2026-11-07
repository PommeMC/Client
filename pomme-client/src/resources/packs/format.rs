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

/// DFU's `Codec.INT`: any JSON number through Gson's `Number.intValue`,
/// which truncates fractions and keeps the low 32 bits of larger integers.
pub(super) fn java_int(value: &Value) -> Option<i32> {
    if let Some(value) = value.as_i64() {
        Some(value as i32)
    } else if let Some(value) = value.as_u64() {
        Some(value as i32)
    } else {
        Some(value.as_f64()?.trunc() as i128 as i32)
    }
}

/// `ExtraCodecs.NON_NEGATIVE_INT`, shared by installed version.json and
/// selected pack metadata.
pub(super) fn format_component(value: &Value) -> Option<u32> {
    u32::try_from(java_int(value)?).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{format_component, java_int};

    #[test]
    fn numbers_read_like_codec_int() {
        assert_eq!(java_int(&json!(46)), Some(46));
        assert_eq!(java_int(&json!(46.0)), Some(46));
        assert_eq!(java_int(&json!(46.9)), Some(46));
        assert_eq!(java_int(&json!(-3.5)), Some(-3));
        assert_eq!(java_int(&json!(4_294_967_297_i64)), Some(1));
        assert_eq!(java_int(&json!("46")), None);
        assert_eq!(java_int(&json!(true)), None);
        assert_eq!(format_component(&json!(-1)), None);
        assert_eq!(format_component(&json!(97.0)), Some(97));
    }
}
