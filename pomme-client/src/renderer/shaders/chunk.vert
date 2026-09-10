#version 450

#include "fog.glsl"
#include "camera_ubo.glsl"
#include "packing.glsl"

// Binding 0: packed section-local terrain vertex.
layout(location = 0) in vec2 in_pos_xy;
layout(location = 1) in float in_pos_z;
layout(location = 2) in uvec2 in_sprite_uv;
layout(location = 3) in uvec4 in_atlas_rect;
layout(location = 4) in vec4 in_light_tint;
// Binding 1: per-section instance meta for the indirect terrain passes.
layout(location = 5) in ivec3 in_origin;
layout(location = 6) in float in_visibility;

layout(location = 0) out vec2 v_sprite_uv;
layout(location = 1) out float v_light;
layout(location = 2) out vec3 v_tint;
layout(location = 3) flat out float v_visibility;
layout(location = 4) out vec3 v_fog_color;
layout(location = 5) out float v_fog;
layout(location = 6) flat out uvec4 v_atlas_rect;


void main() {
    vec3 local = vec3(in_pos_xy, in_pos_z) * POS_RANGE - POS_BIAS;
    vec3 rel = vec3(in_origin - camera_block.xyz) - camera_pos.xyz + local;
    gl_Position = view_proj * vec4(rel, 1.0);
    v_sprite_uv = vec2(in_sprite_uv) / TERRAIN_UV_FIXED_SCALE;
    v_light = in_light_tint.r;
    v_tint = in_light_tint.gba;
    v_visibility = in_visibility;
    v_fog_color = fog_color.rgb;
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    v_atlas_rect = in_atlas_rect;
}
