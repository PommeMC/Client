// Distant terrain's model atlas (renderer/distant/models.rs): a tile per
// model, its six face textures two rows of three.
const uint ATLAS_COLUMNS = 64u;
const vec2 TILE_SIZE = vec2(48.0, 32.0);
const vec2 ATLAS_SIZE = vec2(ATLAS_COLUMNS, 32u) * TILE_SIZE;
const float FACE_SIZE = 16.0;

// models.rs face flags, and the one distant.vert adds for a textured model.
const uint FACE_DISCARD = 1u;
const uint FACE_TRANSLUCENT = 0x10u;
const uint TEXTURED = 0x100u;
