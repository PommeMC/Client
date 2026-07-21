#version 450

#include "fog.glsl"

#include "camera_ubo.glsl"

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 uv;
layout(location = 2) in vec4 color;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;
layout(location = 2) out float v_fog;
layout(location = 3) out vec3 v_fog_color;

void main() {
    // Positions are absolute world-space; render camera-relative for precision
    // (matches item_entity.vert).
    vec3 rel = position - camera_pos.xyz;
    gl_Position = view_proj * vec4(rel, 1.0);
    v_uv = uv;
    v_color = color;
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    v_fog_color = fog_color.rgb;
}
