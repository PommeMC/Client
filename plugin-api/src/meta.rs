use tracing_shared::SharedLogger;

use crate::{PluginModule, Version, parse_u32};

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
