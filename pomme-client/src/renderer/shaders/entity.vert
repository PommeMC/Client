#version 450

#include "fog.glsl"

layout(set = 0, binding = 0) uniform CameraUniform {
    mat4 view_proj;
    vec4 camera_pos;
    vec4 fog_color;
};

layout(push_constant) uniform PushConstants {
    mat4 model;
    vec4 tint;
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;
layout(location = 2) in vec4 light_tint;

layout(location = 0) out vec2 v_tex_coords;
layout(location = 1) out vec4 v_tint;
layout(location = 2) out float v_fog;
layout(location = 3) out vec3 v_fog_color;

void main() {
    vec4 world_pos = model * vec4(position, 1.0);
    vec3 rel = world_pos.xyz - camera_pos.xyz;
    gl_Position = view_proj * vec4(rel, 1.0);
    v_tex_coords = tex_coords;
    v_tint = tint;
    v_fog = fog_factor(rel, camera_pos.w, fog_color.w);
    v_fog_color = fog_color.rgb;
}
