#version 450

// Each listed section's quad bounds as a box, grown by a block and brought a
// hair closer, depth-tested against the distant image's depth; the fragment
// stage records the sections that show.

#define DISTANT_SET 0
#define SCENE_BINDING 0
#define SECTION_METADATA_BINDING 1
#define VISIBILITY_BINDING 2
#define VISIBILITY_ACCESS readonly
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
    offset += cube_corner(gl_VertexIndex) * (vec3(section_aabb_size(section)) + 2.0 * EXPANSION);
    gl_Position = distant_view_proj * vec4(vec3(pos) + offset * float(1 << detail), 1.0);
    gl_Position.z -= 0.000001 * gl_Position.w;

    v_id = sid;
    // Whether the pass saw it last frame, for the temporal draws.
    v_value = seen_now(visibility_data[sid]);
}
