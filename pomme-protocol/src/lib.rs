//! Pomme-owned Minecraft protocol data and wire encoding.
//!
//! The client always speaks the native version (`version::NATIVE`)
//! internally; this crate is the seam where per-version protocol knowledge
//! (packet ids, wire formats, registry ids) lives so other versions, older
//! or newer, can be supported by translation. Depends on no azalea crates by
//! design — azalea cross-checks live in pomme-client's tests.

pub mod known_packs;
pub mod packets;
pub mod registries;
pub mod version;
pub mod wire;

pub use known_packs::KnownPack;
pub use packets::{Direction, PacketTable, Phase};
pub use registries::{ClientRegistry, RegistryRemaps, RegistryTable};
pub use version::ProtocolVersion;
