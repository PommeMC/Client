# pomme-inventory

Pomme's fork of [azalea-inventory](https://github.com/azalea-rs/azalea) (MIT,
see `THIRD_PARTY_LICENSES.md`), substituted for it via the `[patch]` entries in the
workspace root `Cargo.toml`, beside `pomme-registry`. It holds the in-memory
item types: stacks, component patches, the component values and each item's
default components.

`src/default_components/generated.rs` is generated from the reference's item
component reports by `tools/typegen` (`just typegen <version>`).
Everything else is hand-written and edited here like any other crate.

Stacks and components are framed on the wire by `pomme_protocol::item`, not by
the codecs here; `pomme-client/src/net/item_codec.rs` bridges the two.
