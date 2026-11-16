# pomme-inventory

Pomme's fork of [azalea-inventory](https://github.com/azalea-rs/azalea) (MIT,
see `THIRD_PARTY_LICENSES.md`), substituted for it via the `[patch]` entries
in the workspace root `Cargo.toml`.

The client holds its stacks as `pomme-item`'s types and no longer uses this
crate's. What it still takes from here, through
`pomme-client/src/net/item_codec.rs`: most component structs, the component
hashes, the tooltip JSON and the click operation types. Each goes as pomme
ports it; the azalea crates still in the tree build against the rest.
