use stabby::boxed::Box;
use stabby::dynptr;
use stabby::str::Str;

pub mod meta;

pub use plugin_macros::plugin;
pub use stabby;

use crate::meta::Version;

pub trait Plugin {
    fn new() -> Self;

    fn on_client_started(&mut self) {}
    fn on_client_stopping(&mut self) {}

    fn on_client_tick_start(&mut self) {}
    fn on_client_tick_end(&mut self) {}
}

#[stabby::stabby]
pub trait SPlugin {
    extern "C" fn on_client_started(&mut self);
    extern "C" fn on_client_stopping(&mut self);

    extern "C" fn on_client_tick_start(&mut self);
    extern "C" fn on_client_tick_end(&mut self);
}

fn guarded(hook: &str, f: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
        tracing::error!("plugin panicked in {hook}");
    }
}

impl<T: Plugin> SPlugin for T {
    extern "C" fn on_client_started(&mut self) {
        guarded("on_client_started", || {
            <Self as Plugin>::on_client_started(self)
        });
    }
    extern "C" fn on_client_stopping(&mut self) {
        guarded("on_client_stopping", || {
            <Self as Plugin>::on_client_stopping(self)
        });
    }
    extern "C" fn on_client_tick_start(&mut self) {
        guarded("on_client_tick_start", || {
            <Self as Plugin>::on_client_tick_start(self)
        });
    }
    extern "C" fn on_client_tick_end(&mut self) {
        guarded("on_client_tick_end", || {
            <Self as Plugin>::on_client_tick_end(self)
        });
    }
}

#[stabby::stabby]
pub struct PluginModule {
    pub name: Str<'static>,
    pub version: Version,
    pub plugin: dynptr!(Box<dyn SPlugin>),
}
