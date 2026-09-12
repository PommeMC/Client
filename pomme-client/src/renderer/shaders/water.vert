#version 450

// chunk.vert variant for the water pass: `draw_water` records one draw per
// section CPU-side, so the section origin and fade arrive as push constants.

#include "fog.glsl"
#include "camera_ubo.glsl"
#include "packing.glsl"

layout(push_constant) uniform SectionPc {
    vec4 origin_fade;
};

layout(location = 0) in vec2 in_pos_xy;
layout(location = 1) in float in_pos_z;
layout(location = 2) in uvec2 in_sprite_uv;
layout(location = 3) in uvec4 in_atlas_rect;
layout(location = 4) in vec4 in_light_tint;

layout(location = 0) out vec2 v_sprite_uv;
layout(location = 1) out float v_light;
layout(location = 2) out vec3 v_tint;
layout(location = 3) flat out float v_visibility;
layout(location = 4) out vec3 v_fog_color;
layout(location = 5) out float v_fog;
layout(location = 6) flat out uvec4 v_atlas_rect;


void main() {
    vec3 local = vec3(in_pos_xy, in_pos_z) * POS_RANGE - POS_BIAS;
    vec3 world = origin_fade.xyz + local;
    vec3 rel = world - camera_pos.xyz;
    gl_Position = view_proj * vec4(rel, 1.0);
    v_sprite_uv = vec2(in_sprite_uv) / TERRAIN_UV_FIXED_SCALE;
    v_light = in_light_tint.r;
    v_tint = in_light_tint.gba;
    v_visibility = origin_fade.w;
    v_fog_color = fog_color.rgb;
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    v_atlas_rect = in_atlas_rect;
}
