#version 450

// Voxy's `quads.frag`: the quad's face texture repeated per block from the
// model atlas, cut out, tinted and lit.

#include "fog.glsl"
#include "distant_models.glsl"

// Every model's face textures (models.rs).
layout(set = 1, binding = 3) uniform texture2D model_atlas;
layout(set = 1, binding = 4) uniform sampler model_sampler;
// One bit per chunk column, ring-indexed as `world/chunk.rs` `ring_slot`:
// set where a real chunk draws instead.
layout(set = 1, binding = 5) readonly buffer DrawnColumns {
    uint drawn[];
};

layout(location = 0) flat in uvec3 v_data;
layout(location = 1) in vec2 v_uv;
layout(location = 2) in vec3 v_fog_color;
layout(location = 3) in float v_fog;
layout(location = 4) in vec2 v_column_pos;
layout(location = 5) flat in ivec2 v_anchor;

layout(location = 0) out vec4 out_color;

// `world/chunk.rs` RING_SIDE.
const int RING_SIDE = 257;

// Whether a real chunk draws under anchor-relative `pos`.
bool drawn_at(vec2 pos) {
    ivec2 chunk = (v_anchor + ivec2(floor(pos))) >> 4;
    ivec2 ring = ((chunk % RING_SIDE) + RING_SIDE) % RING_SIDE;
    uint slot = uint(ring.y * RING_SIDE + ring.x);
    return (drawn[slot >> 5] & (1u << (slot & 31u))) != 0u;
}

void main() {
    uint flags = v_data.x;
    uint face = flags >> 4u & 7u;
    uint model_id = flags >> 16u;
    vec2 size = vec2(flags >> 8u & 0xFu, flags >> 12u & 0xFu);
    // The block within the quad, and the texel within its face.
    vec2 tile;
    vec2 texel = modf(v_uv, tile);
    vec2 face_origin = vec2(model_id % ATLAS_COLUMNS, model_id / ATLAS_COLUMNS) * TILE_SIZE
        + vec2(face >> 1u, face & 1u) * FACE_SIZE;
    vec2 pos = (face_origin + texel * FACE_SIZE) / ATLAS_SIZE;
    // Mipped by how fast the unwrapped coordinate moves, so the wrap from
    // block to block doesn't pick the smallest mip.
    vec2 unwrapped = v_uv * (FACE_SIZE / ATLAS_SIZE);
    vec4 color = textureGrad(sampler2D(model_atlas, model_sampler), pos, dFdx(unwrapped), dFdy(unwrapped));
    if (any(notEqual(clamp(tile, vec2(0.0), size), tile))) {
        discard;
    }
    if (drawn_at(v_column_pos)) {
        discard;
    }
    // Cut out by the unmipped alpha, which mips would smear.
    vec4 exact = textureLod(sampler2D(model_atlas, model_sampler), pos, 0.0);
    if ((flags & 2u) != 0u) {
        if (exact.a == 0.0) {
            discard;
        }
    } else {
        color.a = 1.0;
        if ((flags & 1u) != 0u && exact.a <= 0.1) {
            discard;
        }
    }
    // `computeColour`: tint state 2 everywhere, 1 only on grey texels, as a
    // grass block's side.
    uint tint = flags >> 2u & 3u;
    bool grey = abs(exact.r - exact.g) < 0.02 && abs(exact.g - exact.b) < 0.02;
    if (tint == 2u || (tint == 1u && grey)) {
        color.rgb *= unpackUnorm4x8(v_data.z).rgb;
    }
    color.rgb *= unpackUnorm4x8(v_data.y).rgb;
    out_color = vec4(apply_fog(color.rgb, v_fog, v_fog_color), color.a);
}
