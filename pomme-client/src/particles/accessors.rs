//! Impl-local adapters for generated mechanical field access.
//!
//! No arguments forwards the full default method set, regardless of exclusions.
//! Pass the original struct identifier after its definition in the same module
//! to automatically select only non-excluded helpers, without repeating the
//! exclusion list. Typed forwarding does not resolve aliases or qualified
//! types.

macro_rules! forward_particle_accessors {
    ($($args:tt)*) => {
        pomme_particle_macros::forward_particle_accessors!($($args)*);
    };
}

macro_rules! forward_quad_accessors {
    ($($args:tt)*) => {
        pomme_particle_macros::forward_quad_accessors!($($args)*);
    };
}

pub(super) use forward_particle_accessors;
pub(super) use forward_quad_accessors;
