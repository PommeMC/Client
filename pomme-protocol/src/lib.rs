//! Pomme-owned Minecraft protocol data and wire encoding.
//!
//! The client always speaks the latest supported version internally; this
//! crate is the seam where per-version protocol knowledge (packet ids, wire
//! formats, registry ids) lives so older servers can be supported by
//! translation. Depends on no azalea crates by design — azalea cross-checks
//! live in pomme-client's tests.

pub mod packets;
pub mod registries;
pub mod version;
pub mod wire;

pub use packets::{Direction, PacketTable, Phase};
pub use registries::{ClientRegistry, RegistryRemaps, RegistryTable};
pub use version::ProtocolVersion;
