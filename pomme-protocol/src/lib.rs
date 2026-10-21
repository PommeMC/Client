//! Pomme-owned Minecraft protocol data and wire encoding.
//!
//! The client always speaks the native version (`version::NATIVE`)
//! internally; this crate is the seam where per-version protocol knowledge
//! (packet ids, wire formats, registry ids) lives so other versions, older
//! or newer, can be supported by translation. Depends on no azalea crates by
//! design — azalea cross-checks live in pomme-client's tests.

pub mod block;
pub mod chunk;
pub mod codec;
pub mod container;
pub mod entity;
pub mod entity_data;
pub mod hud;
pub mod item;
pub mod known_packs;
pub mod lifecycle;
pub mod packets;
pub mod player;
pub mod player_info;
pub mod registries;
pub mod scoreboard;
pub mod section;
pub mod sound;
#[cfg(any(test, feature = "vanilla-fixtures"))]
pub mod vanilla_fixtures;
pub mod version;
pub mod wire;

pub use known_packs::{KnownPack, KnownPackTable};
pub use packets::{Direction, PacketTable, Phase};
pub use registries::{ClientRegistry, RegistryRemaps, RegistryTable};
pub use version::ProtocolVersion;
