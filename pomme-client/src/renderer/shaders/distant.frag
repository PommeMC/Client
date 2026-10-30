#version 450

#include "fog.glsl"

// One bit per chunk column, ring-indexed as `world/chunk.rs` `ring_slot`:
// set where a real chunk is loaded, which draws there instead.
layout(set = 1, binding = 0) readonly buffer LoadedColumns {
    uint loaded[];
};

layout(location = 0) in vec3 v_color;
layout(location = 1) in vec3 v_fog_color;
layout(location = 2) in float v_fog;
layout(location = 3) in vec2 v_column_pos;
layout(location = 4) flat in ivec2 v_anchor;

layout(location = 0) out vec4 out_color;

// `world/chunk.rs` RING_SIDE.
const int RING_SIDE = 257;

void main() {
    ivec2 block = v_anchor + ivec2(floor(v_column_pos));
    ivec2 chunk = block >> 4;
    ivec2 ring = ((chunk % RING_SIDE) + RING_SIDE) % RING_SIDE;
    uint slot = uint(ring.y * RING_SIDE + ring.x);
    if ((loaded[slot >> 5] & (1u << (slot & 31u))) != 0u) {
        discard;
    }
    out_color = vec4(apply_fog(v_color, v_fog, v_fog_color), 1.0);
}
