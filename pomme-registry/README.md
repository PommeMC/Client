# pomme-registry

The game's registries as Rust types: the builtin registries as enums in
protocol-id order (`ItemKind`, `EntityKind`, ...), the registries a server
sends as ids with named keys, the data pack's tags, and `Identifier`,
`Holder` and `HolderSet`.

The entry lists (`src/builtin.rs`, `src/data/generated.rs`, `src/tags/`) are
generated from a reference version's data-generator output by `tools/typegen`
(`just typegen <version>`). Everything else is hand-written.
