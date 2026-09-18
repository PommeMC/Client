#version 450

#include "fog.glsl"

#include "camera_ubo.glsl"

layout(set = 2, binding = 0, std430) readonly buffer ItemTintPalette {
    uint item_tints[];
};

layout(push_constant) uniform PushConstants {
    mat4 model;
    layout(offset = 72) uint item_tint_base;
    layout(offset = 76) uint item_tint_count;
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;
layout(location = 2) in vec4 light_tint;
layout(location = 4) in uint tint_index;

layout(location = 0) out vec2 v_tex_coords;
layout(location = 1) out float v_light;
layout(location = 2) out vec3 v_tint;
layout(location = 3) out float v_fog;
layout(location = 4) out vec3 v_fog_color;

vec3 decode_item_tint(uint color) {
    return vec3(
        float((color >> 16) & 255u),
        float((color >> 8) & 255u),
        float(color & 255u)
    ) / 255.0;
}

void main() {
    vec4 world_pos = model * vec4(position, 1.0);
    vec3 rel = world_pos.xyz - camera_pos.xyz;
    gl_Position = view_proj * vec4(rel, 1.0);
    v_tex_coords = tex_coords;
    v_light = light_tint.r;
    v_tint = tint_index < item_tint_count
        ? decode_item_tint(item_tints[item_tint_base + tint_index])
        : vec3(1.0);
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    v_fog_color = fog_color.rgb;
}
