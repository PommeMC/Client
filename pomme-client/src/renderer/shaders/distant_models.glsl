// Distant terrain's models (renderer/distant/models.rs): the atlas, a tile
// per model with its six face textures two rows of three, and the record of
// each.
const uint ATLAS_COLUMNS = 64u;
const vec2 TILE_SIZE = vec2(48.0, 32.0);
const vec2 ATLAS_SIZE = vec2(ATLAS_COLUMNS, 32u) * TILE_SIZE;
const float FACE_SIZE = 16.0;

// Per face: the drawn texels' min x, max x, min y, max y in 4 bits each,
// the inset from its side in 64ths from bit 16, cutout at 22 and
// cutout-once-merged at 23, the tint state in 24-25.
struct BlockModel {
    uint face_data[6];
    uint flags;
    // ABGR with MODEL_BIOME_TINT clear, else the tint kind.
    uint tint;
};

const uint MODEL_BIOME_TINT = 2u;
const uint MODEL_TRANSLUCENT = 4u;
const uint MODEL_SHADED = 8u;
const uint TINT_BIOMES = 512u;
