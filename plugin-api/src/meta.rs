use std::fmt;

use tracing_shared::SharedLogger;

use crate::PluginModule;

#[stabby::stabby]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}
impl Version {
    pub const fn is_compatible_with(self, host: Self) -> bool {
        if self.major == 0 {
            return self.major == host.major
                && self.minor == host.minor
                && self.patch == host.patch;
        }

        self.major == host.major && self.minor <= host.minor
    }
}
impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

pub const fn parse_u32(s: &str) -> u32 {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        panic!("string empty");
    }
    let mut i = 0;
    let mut n = 0u32;
    while i < bytes.len() {
        let b = bytes[i];
        if b < b'0' || b > b'9' {
            panic!("not a digit");
        }
        let digit = (b - b'0') as u32;
        n = match n.checked_mul(10) {
            Some(v) => v,
            None => panic!("u32 overflow"),
        };
        n = match n.checked_add(digit) {
            Some(v) => v,
            None => panic!("u32 overflow"),
        };
        i += 1;
    }
    n
}

pub const PLUGIN_MARKER_SYMBOL_NAME: &str = "PLUGIN_MARKER";
pub type PluginMarker = u64;
pub const PLUGIN_MARKER_VALUE: PluginMarker = 0x504F_4D4D_4550_4C47;

pub const PLUGIN_API_VERSION_SYMBOL_NAME: &str = "PLUGIN_API_VERSION";
pub type PluginApiVersion = Version;
pub const PLUGIN_API_VERSION_VALUE: PluginApiVersion = PluginApiVersion {
    major: parse_u32(env!("CARGO_PKG_VERSION_MAJOR")),
    minor: parse_u32(env!("CARGO_PKG_VERSION_MINOR")),
    patch: parse_u32(env!("CARGO_PKG_VERSION_PATCH")),
};

// Must match the generated fn in plugin macro
pub const LOAD_PLUGIN_FN_NAME: &str = "load_plugin";
pub type LoadPluginFn = extern "C" fn() -> PluginModule;

pub const SETUP_LOGGER_FN_NAME: &str = "setup_shared_logger_ref";
pub type SetupLoggerFn = extern "C" fn(logger: &SharedLogger);

pub use tracing_shared::setup_shared_logger_ref;
