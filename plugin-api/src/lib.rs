use std::fmt;

use stabby::boxed::Box;
use stabby::dynptr;
use stabby::str::Str;

pub mod meta;

pub use plugin_macros::plugin;
pub use stabby;

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

pub trait Plugin {
    fn new() -> Self;

    #[inline]
    fn on_init(&mut self) {}
}

#[stabby::stabby]
pub trait SPlugin {
    extern "C" fn on_init(&mut self);
}

impl<T: Plugin> SPlugin for T {
    extern "C" fn on_init(&mut self) {
        self.on_init()
    }
}

#[stabby::stabby]
pub struct PluginModule {
    pub name: Str<'static>,
    pub version: Version,
    pub plugin: dynptr!(Box<dyn SPlugin>),
}
