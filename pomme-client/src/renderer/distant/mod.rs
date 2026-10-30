//! Distant terrain: the chunks the server has forgotten, kept as columns and
//! drawn coarsely past the loaded ones, like Distant Horizons' client-only
//! mode. A Pomme-only system; the game has nothing like it.
// TODO: disk cache, generation beyond what's been seen, textures, shader
// packs, and dimensions without sky light (a cave roof would cover them).

pub(crate) mod colors;
pub(crate) mod column;
pub(crate) mod gpu;
pub(crate) mod mesh;
pub(crate) mod store;
pub(crate) mod worker;
