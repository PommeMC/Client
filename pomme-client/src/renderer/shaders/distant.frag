#version 450

#include "fog.glsl"
#include "distant_models.glsl"

// One bit per chunk column, ring-indexed as `world/chunk.rs` `ring_slot`:
// set where a real chunk draws instead.
layout(set = 1, binding = 0) readonly buffer DrawnColumns {
    uint drawn[];
};
// Every model's face textures (models.rs).
layout(set = 1, binding = 1) uniform texture2D model_atlas;
layout(set = 1, binding = 2) uniform sampler model_sampler;

layout(location = 0) in vec4 v_color;
layout(location = 1) in vec3 v_fog_color;
layout(location = 2) in float v_fog;
layout(location = 3) in vec2 v_column_pos;
layout(location = 4) flat in ivec2 v_anchor;
layout(location = 5) flat in vec2 v_outward;
layout(location = 6) flat in uint v_seam;
layout(location = 7) in vec3 v_light;
layout(location = 8) in vec2 v_uv;
layout(location = 9) flat in vec2 v_tile;
layout(location = 10) flat in uint v_flags;

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

// Voxy's `quads.frag`: the face's texel, mipped by how fast the unwrapped
// texture coordinate moves so the wrap from column to column doesn't pick
// the smallest mip; cut out by its unmipped alpha; tinted per the face's
// tint state.
vec4 model_color(vec2 dx, vec2 dy) {
    vec2 pos = (v_tile + fract(v_uv) * FACE_SIZE) / ATLAS_SIZE;
    vec4 color = textureGrad(sampler2D(model_atlas, model_sampler), pos, dx, dy);
    vec4 exact = textureLod(sampler2D(model_atlas, model_sampler), pos, 0.0);
    if ((v_flags & FACE_TRANSLUCENT) != 0u) {
        if (exact.a == 0.0) {
            discard;
        }
    } else {
        color.a = 1.0;
        if ((v_flags & FACE_DISCARD) != 0u && exact.a <= 0.1) {
            discard;
        }
    }
    uint tint = (v_flags >> 2) & 3u;
    // Partly tinted, as a grass block's side: only its grey texels.
    bool grey = abs(exact.r - exact.g) < 0.02 && abs(exact.g - exact.b) < 0.02;
    if (tint == 2u || (tint == 1u && grey)) {
        color.rgb *= v_color.rgb;
    }
    return color;
}

void main() {
    // Before anything discards.
    vec2 unwrapped = v_uv * (FACE_SIZE / ATLAS_SIZE);
    vec2 dx = dFdx(unwrapped);
    vec2 dy = dFdy(unwrapped);
    // Nothing over a real chunk, nor a side facing one: the chunk draws its
    // own edge, and through its water this one would show as a wall. A
    // seam is the opposite, drawn only facing one.
    bool faces_drawn = v_outward != vec2(0.0) && drawn_at(v_column_pos + v_outward);
    if (drawn_at(v_column_pos) || faces_drawn == (v_seam == 0u)) {
        discard;
    }
    vec4 color = (v_flags & TEXTURED) != 0u ? model_color(dx, dy) : v_color;
    out_color = vec4(apply_fog(color.rgb * v_light, v_fog, v_fog_color), color.a);
}
