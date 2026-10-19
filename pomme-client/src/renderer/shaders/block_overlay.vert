#version 450

#include "camera_ubo.glsl"

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;

layout(location = 0) out vec2 v_uv;

void main() {
    gl_Position = view_proj * vec4(position - camera_pos.xyz, 1.0);
    v_uv = tex_coords;
}
