#version 450

#include "fog.glsl"

#include "camera_ubo.glsl"

// Per-draw model matrix via push constants (vanilla's PoseStack), not the
// instance attributes the mob pipeline uses.
layout(push_constant) uniform PushConstants {
    mat4 model;
    vec4 tint;
    vec4 overlay_color;
    // xy = texture-coordinate scroll offset; zw unused.
    vec4 uv_params;
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;
layout(location = 2) in vec4 light_tint;

layout(location = 0) out vec2 v_tex_coords;
layout(location = 1) out vec4 v_tint;
layout(location = 2) out float v_fog;
layout(location = 3) out vec3 v_fog_color;
layout(location = 4) out vec4 v_overlay;

void main() {
    vec4 world_pos = model * vec4(position, 1.0);
    vec3 rel = world_pos.xyz - camera_pos.xyz;
    gl_Position = view_proj * vec4(rel, 1.0);
    v_tex_coords = tex_coords + uv_params.xy;
    v_tint = tint;
    v_overlay = overlay_color;
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    v_fog_color = fog_color.rgb;
}
