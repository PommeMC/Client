# pomme-registry

Pomme's fork of [azalea-registry](https://github.com/azalea-rs/azalea) (MIT,
see `THIRD_PARTY_LICENSES.md`), substituted for it via the `[patch]` entries in the
workspace root `Cargo.toml`, as `pomme-block` is for azalea-block. It keeps
the package name so the azalea crates still in the tree (-core, -chat,
-entity) build against it.

Upstream's tables follow azalea's release schedule, which lags the game
version this client tracks. Here they are generated from the reference's own
data-generator output by `tools/typegen` (`just typegen <version>`):

- `src/builtin.rs`: the static registries, in protocol-id order, from
  `reports/registries.json`
- `src/data.rs`: the keys of the registries a server sends, from the data
  pack
- `src/tags/`: the block, item, fluid and entity-type tags, from the data pack

Everything else is hand-written and edited here like any other crate.
