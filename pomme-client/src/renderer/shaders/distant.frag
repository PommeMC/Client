#version 450

#include "fog.glsl"

// One bit per chunk column, ring-indexed as `world/chunk.rs` `ring_slot`:
// set where a real chunk draws instead.
layout(set = 1, binding = 0) readonly buffer DrawnColumns {
    uint drawn[];
};

layout(location = 0) in vec4 v_color;
layout(location = 1) in vec3 v_fog_color;
layout(location = 2) in float v_fog;
layout(location = 3) in vec2 v_column_pos;
layout(location = 4) flat in ivec2 v_anchor;
layout(location = 5) flat in vec2 v_outward;
layout(location = 6) flat in uint v_seam;

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
    // Nothing over a real chunk, nor a side facing one: the chunk draws its
    // own edge, and through its water this one would show as a wall. A
    // seam is the opposite, drawn only facing one.
    bool faces_drawn = v_outward != vec2(0.0) && drawn_at(v_column_pos + v_outward);
    if (drawn_at(v_column_pos) || faces_drawn == (v_seam == 0u)) {
        discard;
    }
    out_color = vec4(apply_fog(v_color.rgb, v_fog, v_fog_color), v_color.a);
}
