//! Distant terrain: the chunks the server has forgotten, kept as columns
//! (on disk too, per server or world) and drawn coarsely past the loaded
//! ones, like Distant Horizons' client-only mode. A Pomme-only system; the game
//! has nothing like it.
// TODO: generation beyond what's been seen, textures, shader packs, and
// dimensions without sky light (a cave roof would cover them).

pub(crate) mod bakery;
pub(crate) mod build;
pub(crate) mod colors;
pub(crate) mod gpu;
pub(crate) mod models;
pub(crate) mod scan;
pub(crate) mod section;
pub(crate) mod voxel;
pub(crate) mod worker;
pub(crate) mod world;
