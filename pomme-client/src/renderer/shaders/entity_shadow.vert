#version 450

#include "fog.glsl"

#include "camera_ubo.glsl"

// Vanilla `VIEW_OFFSET_Z_LAYERING`: the view scaled toward the camera so the
// shadow wins the depth test against the block face it lies on.
const float LAYERING_SCALE = 1.0 - 1.0 / 4096.0;

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec4 in_color;
layout(location = 2) in vec2 in_uv;

layout(location = 0) out vec4 v_color;
layout(location = 1) out vec2 v_uv;
layout(location = 2) out float v_fog;
layout(location = 3) out vec3 v_fog_color;

void main() {
    vec3 rel = in_position - camera_pos.xyz;
    gl_Position = view_proj * vec4(rel * LAYERING_SCALE, 1.0);
    v_color = in_color;
    v_uv = in_uv;
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    v_fog_color = fog_color.rgb;
}
