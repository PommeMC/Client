#version 450

// Voxy's `gl46/cull/raster.vert`: each listed section's quad bounds as a
// box, grown by a block and brought a hair closer, depth-tested against the
// distant image's depth; the fragment stage records the sections that show.

#define DISTANT_SET 0
#define SCENE_BINDING 0
#define SECTION_METADATA_BINDING 1
#define VISIBILITY_BINDING 2
#define RENDER_LIST_BINDING 3
#include "distant_section.glsl"

layout(location = 0) flat out uint v_id;
layout(location = 1) flat out uint v_value;

void main() {
    uint sid = render_list[gl_InstanceIndex];
    SectionMeta section = section_data[sid];
    uint detail = section_detail(section);
    ivec3 pos = ((section_position(section) << detail) - camera_section) << 5;
    const float EXPANSION = 1.0;
    vec3 offset = vec3(section_aabb_offset(section)) - EXPANSION;
    // The shared cube indices give the corner's bits: x, y, z.
    uint corner = gl_VertexIndex;
    offset += vec3(corner & 1u, corner >> 1u & 1u, corner >> 2u)
        * (vec3(section_aabb_size(section)) + 2.0 * EXPANSION);
    gl_Position = distant_view_proj * vec4(vec3(pos) + offset * float(1 << detail), 1.0);
    gl_Position.z -= 0.000001 * gl_Position.w;

    v_id = sid;
    // Whether the pass saw it last frame, for the temporal draws.
    uint previous = visibility_data[sid] & 0x7FFFFFFFu;
    v_value = (frame_id & 0x7FFFFFFFu) | (uint(previous == frame_id - 1u) << 31u);
}
