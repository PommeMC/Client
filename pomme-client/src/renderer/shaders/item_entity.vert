#version 450

#include "fog.glsl"

#include "camera_ubo.glsl"

layout(push_constant) uniform PushConstants {
    mat4 model;
    layout(offset = 72) uint item_tint0;
    layout(offset = 76) uint item_tint1;
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;
layout(location = 2) in vec4 light_tint;

layout(location = 0) out vec2 v_tex_coords;
layout(location = 1) out float v_light;
layout(location = 2) out vec3 v_tint;
layout(location = 3) out float v_fog;
layout(location = 4) out vec3 v_fog_color;

void main() {
    vec4 world_pos = model * vec4(position, 1.0);
    vec3 rel = world_pos.xyz - camera_pos.xyz;
    gl_Position = view_proj * vec4(rel, 1.0);
    v_tex_coords = tex_coords;
    v_light = light_tint.r;
    // Tint-indexed vertices encode index+1 in the middle tint byte, with
    // zeroes in the other two bytes. Stock 26.2 item definitions use at most
    // two tint entries; the actual stack-dependent colors are supplied per draw.
    if (light_tint.g == 0.0 && light_tint.a == 0.0 && light_tint.b > 0.0) {
        uint tint_index = uint(round(light_tint.b * 255.0)) - 1u;
        uint item_tint = tint_index == 0u ? item_tint0 : item_tint1;
        v_tint = vec3(
            float((item_tint >> 16) & 255u),
            float((item_tint >> 8) & 255u),
            float(item_tint & 255u)
        ) / 255.0;
    } else {
        v_tint = light_tint.gba;
    }
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    v_fog_color = fog_color.rgb;
}
