//! Distant terrain: every chunk seen, kept as sections at five levels of
//! detail (on disk too, per server or world) and drawn past the loaded
//! ones from models baked per block state, the GPU choosing each frame
//! which sections to draw. A Pomme-only system; the game has
//! nothing like it.
// TODO: generation beyond what's been seen, shader packs, and dimensions
// without sky light (a cave roof would cover them).

pub(crate) mod bakery;
pub(crate) mod build;
pub(crate) mod colors;
pub(crate) mod gpu;
pub(crate) mod models;
pub(crate) mod nodes;
pub(crate) mod scan;
pub(crate) mod section;
pub(crate) mod voxel;
pub(crate) mod worker;
pub(crate) mod world;
