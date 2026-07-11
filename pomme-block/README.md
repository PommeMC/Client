# pomme-block

Data-free shim substituted for [azalea-block](https://github.com/azalea-rs/azalea)
via the `[patch]` entries in the workspace root `Cargo.toml` (both the crates-io
and git-source tables — azalea-block is a path dependency inside the azalea git
workspace).

Upstream azalea-block compiles a full per-version block-state table, which lags
the game version this client tracks; stale tables made state ids decode as the
wrong blocks. This shim carries **no block data at all**: `BlockState` is a plain
id accepted across the whole `u16` range, `property()` is always `None`,
`as_block_kind()` is always `Air`, and `FluidState` is always empty. It exists
only so azalea-world/-protocol/-entity compile and pass state ids through
unharmed. All block semantics (id, properties, air/fluid, behavior, shapes) live
in pomme's per-version tables under `pomme-client/src/world/block/`, generated
from Mojang's data-generator reports by `tools/blockgen`.

Known degradations from the missing data, accepted on purpose:

- azalea-world's heightmap recompute treats every block as air, so rain can fall
  through a freshly placed block until chunk reload (server-sent heightmaps at
  chunk load are unaffected). TODO: compute weather columns from pomme's tables.
- azalea log messages print bare state ids instead of block names.

After bumping the azalea git dependency: run `cargo tree -i azalea-block` (must
resolve to this path only) and rebuild — the compiler flags any new API the
shim needs to grow.
