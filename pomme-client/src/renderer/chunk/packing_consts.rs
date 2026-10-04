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

/// Fixed-point scale for sprite-local terrain UVs. 4095 units per sprite
/// keeps every integer repeat boundary through 16 blocks exactly representable
/// in u16 (16 * 4095 = 65520).
pub const TERRAIN_UV_FIXED_SCALE: f32 = 4095.0;
pub const TERRAIN_UV_MAX_REPEAT: f32 = 16.0;
