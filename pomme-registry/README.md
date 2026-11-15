# pomme-registry

The game's registries as Rust types: the builtin registries as enums in
protocol-id order (`ItemKind`, `EntityKind`, ...), the registries a server
sends as ids with named keys, the data pack's tags, and `Identifier`,
`Holder` and `HolderSet`.

The entry lists (`src/builtin.rs`, `src/data/generated.rs`, `src/tags/`) are
generated from a reference version's data-generator output by `tools/typegen`
(`just typegen <version>`). Everything else is hand-written.

`azalea-compat/` is this crate under the name `azalea-registry`, patched in
via the workspace root `Cargo.toml` for the azalea crates still in the tree
(-core, -chat, -entity) and `pomme-inventory`. It turns on the `azalea`
feature, which implements azalea's wire, serde and NBT traits for these types
(`src/azalea.rs`). Both go with the last of those crates.
