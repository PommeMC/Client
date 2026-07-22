use std::path::Path;
use std::{fmt, fs};

use libloading::Library;
use plugin_api::meta::{
    LOAD_PLUGIN_FN_NAME, LoadPluginFn, PLUGIN_API_VERSION_SYMBOL_NAME, PLUGIN_API_VERSION_VALUE,
    PLUGIN_MARKER_SYMBOL_NAME, PLUGIN_MARKER_VALUE, PluginApiVersion, PluginMarker,
    SETUP_LOGGER_FN_NAME, SetupLoggerFn,
};
use plugin_api::{SPlugin, SPluginDynMut as _, Version};
use stabby::boxed::Box as SBox;
use stabby::dynptr;
use stabby::libloading::StabbyLibrary;
use tracing_shared::SharedLogger;

#[cfg(target_os = "windows")]
const LIB_EXT: &str = "dll";
#[cfg(target_os = "macos")]
const LIB_EXT: &str = "dylib";
#[cfg(all(unix, not(target_os = "macos")))]
const LIB_EXT: &str = "so";

pub struct LoadedPlugin {
    name: &'static str,
    version: Version,
    plugin: dynptr!(SBox<dyn SPlugin>),
    _library: Library,
}

pub struct Plugins {
    plugins: Vec<LoadedPlugin>,
}

impl Plugins {
    pub fn load(directory: &Path, shared_logger: &SharedLogger) -> Self {
        let mut loaded = Vec::new();

        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(err) => {
                tracing::error!("Failed to read plugin directory {directory:?}: {err}");
                return Self { plugins: loaded };
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    tracing::warn!("Failed to read directory entry: {err}");
                    continue;
                }
            };

            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name();

            let is_lib = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case(LIB_EXT))
                .unwrap_or(false);
            if !is_lib {
                continue;
            }

            let lib = match unsafe { Library::new(&path) } {
                Ok(lib) => lib,
                Err(err) => {
                    tracing::error!("Failed to load plugin {name:?}: {err}");
                    continue;
                }
            };

            let is_plugin = unsafe {
                lib.get::<&PluginMarker>(PLUGIN_MARKER_SYMBOL_NAME.as_bytes())
                    .map(|val| **val == PLUGIN_MARKER_VALUE)
                    .unwrap_or(false)
            };
            if !is_plugin {
                tracing::debug!("Skipping non-plugin library {path:?}");
                continue;
            }

            let plugin_api_version = unsafe {
                match lib.get::<&PluginApiVersion>(PLUGIN_API_VERSION_SYMBOL_NAME.as_bytes()) {
                    Ok(v) => *v,
                    Err(err) => {
                        tracing::error!(
                            "Plugin {name:?} is missing required symbol `{}`: {err}",
                            PLUGIN_API_VERSION_SYMBOL_NAME,
                        );
                        continue;
                    }
                }
            };
            if !plugin_api_version.is_compatible_with(PLUGIN_API_VERSION_VALUE) {
                tracing::error!(
                    "Incompatible plugin API version: {name:?} v{}, client v{}",
                    plugin_api_version,
                    PLUGIN_API_VERSION_VALUE,
                );
                continue;
            }

            let load_plugin =
                match unsafe { lib.get_stabbied::<LoadPluginFn>(LOAD_PLUGIN_FN_NAME.as_bytes()) } {
                    Ok(f) => f,
                    Err(err) => {
                        tracing::error!(
                            "Plugin {name:?} is missing required symbol `{}`: {err}",
                            LOAD_PLUGIN_FN_NAME,
                        );
                        continue;
                    }
                };
            let plugin = load_plugin();

            let setup_logger =
                match unsafe { lib.get::<SetupLoggerFn>(SETUP_LOGGER_FN_NAME.as_bytes()) } {
                    Ok(f) => f,
                    Err(err) => {
                        tracing::error!(
                            "Plugin {name:?} is missing required symbol `{}`: {err}",
                            SETUP_LOGGER_FN_NAME,
                        );
                        continue;
                    }
                };
            setup_logger(shared_logger);

            loaded.push(LoadedPlugin {
                name: plugin.name.as_str(),
                version: plugin.version,
                plugin: plugin.plugin,
                _library: lib,
            });
        }

        let slf = Self { plugins: loaded };

        tracing::info!("Loaded plugins: {}", slf);

        slf
    }

    pub fn init_all(&mut self) {
        for plugin in &mut self.plugins {
            plugin.plugin.on_init();
        }
    }
}

impl fmt::Display for Plugins {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.plugins.is_empty() {
            return write!(f, "<none>");
        }

        writeln!(f)?;
        for (i, plugin) in self.plugins.iter().enumerate() {
            if i != 0 {
                writeln!(f)?;
            }

            write!(f, "- {} v{}", plugin.name, plugin.version,)?;
        }

        Ok(())
    }
}
