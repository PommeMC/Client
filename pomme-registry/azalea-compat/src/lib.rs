//! `pomme-registry` under azalea-registry's name and paths, patched in for
//! the azalea crates still in the tree (see the workspace `Cargo.toml`).
//! Pomme's own code depends on `pomme-registry` directly; this goes with
//! the last of those crates.

// azalea-entity names it at upstream's old root path.
pub use pomme_registry::builtin::EntityKind;
pub use pomme_registry::*;
