// Distant terrain's GPU records (renderer/distant/gpu.rs): the scene
// uniform, a section's metadata, and the buffers the command generation
// passes share, each declared by the shader that defines its binding.

// The scene uniform: the distant view projection moved by the camera's
// position within its section, that section, the frame, and whether the
// occlusion pass was skipped (cmdgen then keeps visibility itself); then
// the traversal's: the Hi-Z's base size (width << 16 | height), the screen
// area a node splits at, the render list's and the frame's request queue's
// room, the render distance squared in blocks, and the frustum planes.
#ifdef SCENE_BINDING
layout(set = DISTANT_SET, binding = SCENE_BINDING, std140) uniform SceneUniform {
    mat4 distant_view_proj;
    ivec3 camera_section;
    uint frame_id;
    vec3 camera_sub_pos;
    uint occlusion_skipped;
    uint hiz_size;
    float min_sss;
    uint render_list_max;
    uint request_limit;
    float render_distance;
    vec4 frustum[6];
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

// The draw list's regions, in draws.
const uint OPAQUE_DRAWS = 400000u;
const uint TRANSLUCENT_DRAWS = 100000u;
const uint TRANSLUCENT_OFFSET = OPAQUE_DRAWS;
const uint TEMPORAL_OFFSET = OPAQUE_DRAWS + TRANSLUCENT_DRAWS;
// Translucent draws are bucketed by distance before their list is built;
// the bucket counts lead the list.
const uint TRANSLUCENT_BUCKETS = 1024u;
// The shared index buffer: this many quads' indices, then the cube's.
const uint QUADS_PER_DRAW = 65536u;

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
#endif

// cmdgen's dispatch size, the three regions' draw counts, and the occlusion
// pass's draw.
#ifdef DRAW_COUNTS_BINDING
layout(set = DISTANT_SET, binding = DRAW_COUNTS_BINDING, std430) restrict buffer DrawCounts {
    uvec3 cmdgen_dispatch;
    uint opaque_draw_count;
    uint translucent_draw_count;
    uint temporal_draw_count;
    DrawCommand cull_draw;
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
void write_draw(uint index, uint instance, uint offset, uint quad_count) {
    DrawCommand cmd;
    cmd.count = quad_count * 6u;
    cmd.instance_count = 1u;
    cmd.first_index = 0u;
    cmd.base_vertex = int(offset) << 2;
    cmd.base_instance = instance;
    cmd_buffer[index] = cmd;
}
#endif
