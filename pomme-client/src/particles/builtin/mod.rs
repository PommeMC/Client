//! Builtin particle simulations, providers and sprite-provider factories.

mod flame;
mod smoke;

pub(super) use self::flame::FlameParticleRegistration;
pub use self::flame::{FlameParticle, FlameProvider};
pub(super) use self::smoke::SmokeParticleRegistration;
pub use self::smoke::{SmokeParticle, SmokeProvider};
