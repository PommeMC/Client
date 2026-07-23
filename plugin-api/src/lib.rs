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

impl<T: Plugin> SPlugin for T {
    extern "C" fn on_client_started(&mut self) {
        <Self as Plugin>::on_client_started(self);
    }
    extern "C" fn on_client_stopping(&mut self) {
        <Self as Plugin>::on_client_stopping(self);
    }

    extern "C" fn on_client_tick_start(&mut self) {
        <Self as Plugin>::on_client_tick_start(self);
    }
    extern "C" fn on_client_tick_end(&mut self) {
        <Self as Plugin>::on_client_tick_end(self);
    }
}

#[stabby::stabby]
pub struct PluginModule {
    pub name: Str<'static>,
    pub version: Version,
    pub plugin: dynptr!(Box<dyn SPlugin>),
}
