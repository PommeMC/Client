// Section-local position quantization: local coords (block 0..16 plus model
// overhang) map into `[-POS_BIAS, POS_RANGE - POS_BIAS]` across a u16. Chosen
// so a 16-block shift is an exact integer number of u16 steps (16/24*65535 =
// 43690), so the same world position encodes identically in adjacent sections
// — no seams.
//
// Single source of truth: `include!`d by mesher.rs and by build.rs, which
// generates the matching `packing.glsl` for the vertex shaders.
pub const POS_RANGE: f32 = 24.0;
pub const POS_BIAS: f32 = 4.0;

/// Sprite-local terrain UVs: one sprite spans 0..=4095, in 12 bits per axis.
pub const TERRAIN_UV_FIXED_SCALE: f32 = 4095.0;
pub const TERRAIN_UV_BITS: u32 = 12;

/// Bits of `PackedVertex::sprite` holding the sprite id; the 3 above hold the
/// quad's face (`Direction` + 1, 0 when it has none), for shader packs.
pub const SPRITE_BITS: u32 = 13;
