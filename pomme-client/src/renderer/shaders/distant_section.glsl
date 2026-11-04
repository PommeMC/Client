// Distant terrain's GPU records (renderer/distant/gpu.rs): the scene
// uniform, a section's metadata, and the buffers the command generation
// passes share, each declared by the shader that defines its binding.

#include "distant_consts.glsl"

// The scene uniform: the distant view projection moved by the camera's
// position within its section, that section, the frame, and whether the
// occlusion pass was skipped (cmdgen then keeps visibility itself); then
// the traversal's: the Hi-Z's base size (width << 16 | height), the screen
// area a node splits at, the frame's request queue's room, the render
// distance squared in blocks, the frustum planes, and those
// of the wider view meshes are requested within; last the dimension's shade
// per face (down, up, north, south, then west, east).
#ifdef SCENE_BINDING
layout(set = DISTANT_SET, binding = SCENE_BINDING, std140) uniform SceneUniform {
    mat4 distant_view_proj;
    ivec3 camera_section;
    uint frame_id;
    vec3 camera_sub_pos;
    uint occlusion_skipped;
    uint hiz_size;
    float min_sss;
    uint request_limit;
    float render_distance;
    vec4 frustum[6];
    vec4 request_frustum[6];
    vec4 face_shade[2];
};
#endif

// A section's packed key (section.rs `SectionKey::pack`, high word first):
// the level in the top nibble, then y (8 bits), z and x (24 each).
uint lod_level(uvec2 pos) {
    return pos.x >> 28u;
}

ivec3 lod_position(uvec2 pos) {
    int y = (int(pos.x) << 4) >> 24;
    int x = (int(pos.y) << 4) >> 8;
    int z = int((pos.x & ((1u << 20u) - 1u)) << 4u);
    z |= int(pos.y >> 28u);
    z <<= 8;
    z >>= 8;
    return ivec3(x, y, z);
}

// A section's record: the key, the quads' bounds and their start; then the
// eight groups' quad counts in `u16` pairs.
struct SectionMeta {
    uvec4 a;
    uvec4 b;
};

uint section_detail(SectionMeta meta) {
    return lod_level(meta.a.xy);
}

ivec3 section_position(SectionMeta meta) {
    return lod_position(meta.a.xy);
}

uint section_quad_start(SectionMeta meta) {
    return meta.a.w;
}

ivec3 section_aabb_offset(SectionMeta meta) {
    return (ivec3(meta.a.z) >> ivec3(0, 5, 10)) & 31;
}

// At least 1x1x1.
ivec3 section_aabb_size(SectionMeta meta) {
    return ((ivec3(meta.a.z) >> ivec3(15, 20, 25)) & 31) + 1;
}

// `VkDrawIndexedIndirectCommand`.
struct DrawCommand {
    uint count;
    uint instance_count;
    uint first_index;
    int base_vertex;
    uint base_instance;
};

// The draw list's regions' ends.
const uint OPAQUE_END = OPAQUE_DRAWS;
const uint TRANSLUCENT_END = TRANSLUCENT_OFFSET + TRANSLUCENT_DRAWS;
const uint TEMPORAL_END = TEMPORAL_OFFSET + TEMPORAL_DRAWS;

#ifdef SCENE_BINDING
// A section's visibility word for being seen this frame: the frame, and
// whether `previous` was last frame.
uint seen_now(uint previous) {
    bool before = (previous & ~SEEN_BEFORE_BIT) == frame_id - 1u;
    return (frame_id & ~SEEN_BEFORE_BIT) | (before ? SEEN_BEFORE_BIT : 0u);
}
#endif

// The shared cube indices give a box corner's bits: x, y, z.
vec3 cube_corner(uint vertex) {
    return vec3(vertex & 1u, vertex >> 1u & 1u, vertex >> 2u);
}

#ifdef SECTION_METADATA_BINDING
layout(set = DISTANT_SET, binding = SECTION_METADATA_BINDING, std430) readonly restrict buffer SectionBuffer {
    SectionMeta section_data[];
};
#endif

// The sections the traversal picked this frame (`RENDER_LIST_ACCESS` empty
// for the traversal, which writes it).
#ifdef RENDER_LIST_BINDING
#ifndef RENDER_LIST_ACCESS
#define RENDER_LIST_ACCESS readonly
#endif
layout(set = DISTANT_SET, binding = RENDER_LIST_BINDING, std430) RENDER_LIST_ACCESS restrict buffer RenderList {
    uint section_count;
    uint render_list[];
};

// The traversal's count can overshoot the list as it races to fill it.
uint listed_count() {
    return min(section_count, MAX_LISTED);
}
#endif

// cmdgen's dispatch size, the three regions' draw counts, and the occlusion
// pass's draw.
#ifdef DRAW_COUNTS_BINDING
layout(set = DISTANT_SET, binding = DRAW_COUNTS_BINDING, std430) restrict buffer DrawCounts {
    uvec3 cmdgen_dispatch;
    layout(offset = COUNTS_OPAQUE) uint opaque_draw_count;
    layout(offset = COUNTS_TRANSLUCENT) uint translucent_draw_count;
    layout(offset = COUNTS_TEMPORAL) uint temporal_draw_count;
    layout(offset = COUNTS_CULL_DRAW) DrawCommand cull_draw;
};
#endif

#ifdef DRAW_BUFFER_BINDING
layout(set = DISTANT_SET, binding = DRAW_BUFFER_BINDING, std430) writeonly restrict buffer DrawBuffer {
    DrawCommand cmd_buffer[];
};
#endif

// Per section, the last frame the occlusion pass saw it (low 31 bits) and
// whether it saw it the frame before (bit 31); `VISIBILITY_ACCESS` is
// `readonly` for the occlusion pass's vertex stage, which only reads it.
#ifdef VISIBILITY_BINDING
#ifndef VISIBILITY_ACCESS
#define VISIBILITY_ACCESS
#endif
layout(set = DISTANT_SET, binding = VISIBILITY_BINDING, std430) VISIBILITY_ACCESS restrict buffer VisibilityBuffer {
    uint visibility_data[];
};
#endif

// Per draw, its section's packed key.
#ifdef POSITION_BINDING
layout(set = DISTANT_SET, binding = POSITION_BINDING, std430) restrict buffer PositionBuffer {
    uvec2 position_buffer[];
};
#endif

#ifdef TRANSLUCENT_DATA_BINDING
layout(set = DISTANT_SET, binding = TRANSLUCENT_DATA_BINDING, std430) restrict buffer TranslucentData {
    uint translucent_data[];
};
#endif

#ifdef DRAW_BUFFER_BINDING
// Dropped at or past `end`, its region's end: the counts run on past a full
// region, which the draws clamp to.
void write_draw(uint index, uint end, uint instance, uint offset, uint quad_count) {
    if (index >= end) {
        return;
    }
    DrawCommand cmd;
    cmd.count = quad_count * 6u;
    cmd.instance_count = 1u;
    cmd.first_index = 0u;
    cmd.base_vertex = int(offset) << 2;
    cmd.base_instance = instance;
    cmd_buffer[index] = cmd;
}
#endif
