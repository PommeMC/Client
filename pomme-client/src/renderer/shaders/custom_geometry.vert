#version 450

#include "fog.glsl"

#include "camera_ubo.glsl"

#include "lightmap.glsl"

#include "light.glsl"

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 uv;
layout(location = 2) in vec4 color;
layout(location = 3) in vec4 normal;
layout(location = 4) in float light;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;
layout(location = 2) out float v_fog;
layout(location = 3) out vec3 v_fog_color;
layout(location = 4) out vec4 v_light;
layout(location = 5) out float v_diffuse;

// `entity.vsh` for geometry already in world space: the positions are
// anchor-relative and the normals world-space.
void main() {
    vec3 rel = position - camera_pos.xyz;
    gl_Position = view_proj * vec4(rel, 1.0);
    v_uv = uv;
    v_color = color;
    v_light = light_lane_color(light);
    v_diffuse = entity_diffuse(light, mat4(1.0), normal.xyz, rel, camera_block.w != 0);
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    v_fog_color = fog_color.rgb;
}
