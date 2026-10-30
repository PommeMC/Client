#version 450

// Distant terrain (renderer/distant): section-local boxes, coloured and
// shaded on the CPU, lit through the lightmap and fogged like terrain.

#include "fog.glsl"
#include "camera_ubo.glsl"
#include "lightmap.glsl"

layout(push_constant) uniform DistantPc {
    // The camera's view projection with the distant near and far planes.
    mat4 distant_view_proj;
    // The section's corner (at the dimension's min_y), relative to the
    // camera anchor.
    vec4 origin;
};

layout(location = 0) in uint in_xz;
layout(location = 1) in uint in_y;
layout(location = 2) in uvec2 in_light_face;
layout(location = 3) in vec4 in_color;

layout(location = 0) out vec3 v_color;
layout(location = 1) out vec3 v_fog_color;
layout(location = 2) out float v_fog;
// Anchor-relative x/z, nudged half a block into the face's own column, and
// the anchor: what distant.frag finds the column's chunk from.
layout(location = 3) out vec2 v_column_pos;
layout(location = 4) flat out ivec2 v_anchor;
// A side face's step from v_column_pos into the column it faces; zero for
// tops and bottoms.
layout(location = 5) flat out vec2 v_outward;

// mesh.rs `Face`: up, down, north, south, west, east.
const vec2 FACE_NUDGE[6] = vec2[](
    vec2(0.0), vec2(0.0),
    vec2(0.0, 0.5), vec2(0.0, -0.5),
    vec2(0.5, 0.0), vec2(-0.5, 0.0)
);

void main() {
    vec3 local = vec3(float(in_xz & 0xFFFFu), float(in_y), float(in_xz >> 16));
    vec3 world = origin.xyz + local;
    vec3 rel = world - camera_pos.xyz;
    gl_Position = distant_view_proj * vec4(rel, 1.0);

    uint light = in_light_face.x;
    vec2 lightmap_uv = vec2(float(light & 0xFu), float(light >> 4)) * 16.0;
    v_color = in_color.rgb * sample_lightmap(lightmap_uv).rgb;
    v_fog_color = fog_color.rgb;
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    vec2 nudge = FACE_NUDGE[min(in_light_face.y, 5u)];
    v_column_pos = world.xz + nudge;
    v_anchor = camera_block.xz;
    v_outward = -2.0 * nudge;
}
