# pomme-block

`BlockState`: a block state id, and nothing else. An id is accepted across the
whole `u16` range and carries no meaning here. What a state is (its block, its
properties, whether it is air or a fluid, its shapes) lives in pomme's
per-version tables under `pomme-client/src/world/block/`, generated from the
game's data-generator reports by `tools/blockgen`.

The type began as azalea-block's `BlockState`; azalea's MIT notice is kept in
`THIRD_PARTY_LICENSES.md`.
