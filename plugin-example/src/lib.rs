use plugin_api::Plugin;

// #[plugin]
struct ExamplePlugin {
    total_ticks: u64,
}

impl Plugin for ExamplePlugin {
    fn new() -> Self {
        Self { total_ticks: 0 }
    }

    fn on_client_started(&mut self) {
        let span = tracing::info_span!("on_client_started");
        let _enter = span.enter();
        tracing::info!("Client is about to tick.");
    }

    fn on_client_stopping(&mut self) {
        let span = tracing::info_span!("on_client_stopping");
        let _enter = span.enter();
        tracing::info!("Client is stopping. Total ticks: {}", self.total_ticks);
    }

    fn on_client_tick_start(&mut self) {
        let span = tracing::info_span!("on_client_tick_start");
        let _enter = span.enter();
        tracing::info!("Client tick started.");
    }

    fn on_client_tick_end(&mut self) {
        let span = tracing::info_span!("on_client_tick_end");
        let _enter = span.enter();
        tracing::info!("Client tick ended");
        self.total_ticks += 1;
    }
}

#[::stabby::export]
pub extern "C" fn load_plugin() -> ::plugin_api::PluginModule {
    ::plugin_api::PluginModule {
        name: env!("CARGO_PKG_NAME").into(),
        version: ::plugin_api::meta::Version {
            major: ::plugin_api::meta::parse_u32(env!("CARGO_PKG_VERSION_MAJOR")),
            minor: ::plugin_api::meta::parse_u32(env!("CARGO_PKG_VERSION_MINOR")),
            patch: ::plugin_api::meta::parse_u32(env!("CARGO_PKG_VERSION_PATCH")),
        },
        plugin: ::stabby::boxed::Box::new(<ExamplePlugin as Plugin>::new()).into(),
    }
}

#[unsafe(no_mangle)]
pub static PLUGIN_MARKER: ::plugin_api::meta::PluginMarker =
    ::plugin_api::meta::PLUGIN_MARKER_VALUE;
#[unsafe(no_mangle)]
pub static PLUGIN_API_VERSION: ::plugin_api::meta::PluginApiVersion =
    ::plugin_api::meta::PLUGIN_API_VERSION_VALUE;

pub use ::plugin_api::meta::setup_shared_logger_ref;
