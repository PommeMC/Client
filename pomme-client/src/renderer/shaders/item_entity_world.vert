#version 450

#include "fog.glsl"

#include "camera_ubo.glsl"

layout(set = 2, binding = 0, std430) readonly buffer ItemTintPalette {
    uint item_tints[];
};

layout(push_constant) uniform PushConstants {
    mat4 model;
    layout(offset = 68) float nether_lighting;
    layout(offset = 72) uint item_tint_base;
    layout(offset = 76) uint item_tint_count;
    layout(offset = 80) mat3 normal_matrix;
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;
layout(location = 2) in vec4 light_tint;
layout(location = 3) in vec4 normal_packed;
layout(location = 4) in uint tint_index;

layout(location = 0) out vec2 v_tex_coords;
layout(location = 1) out float v_light;
layout(location = 2) out vec3 v_tint;
layout(location = 3) out float v_fog;
layout(location = 4) out vec3 v_fog_color;

float vanilla_level_diffuse(vec3 normal) {
    vec3 light0 = normalize(vec3(0.2, 1.0, -0.7));
    vec3 light1 = normalize(
        nether_lighting > 0.5
            ? vec3(-0.2, -1.0, 0.7)
            : vec3(-0.2, 1.0, 0.7)
    );
    vec2 light = max(vec2(0.0), vec2(dot(light0, normal), dot(light1, normal)));
    return min(1.0, (light.x + light.y) * 0.6 + 0.4);
}

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

    // Vanilla's VertexConsumer transforms the baked quad direction by the
    // item's normal matrix before item.vsh applies Lighting.Entry.LEVEL.
    vec3 world_normal = normalize(normal_matrix * normal_packed.xyz);

    v_tex_coords = tex_coords;
    v_light = vanilla_level_diffuse(world_normal);
    v_tint = tint_index < item_tint_count
        ? decode_item_tint(item_tints[item_tint_base + tint_index])
        : vec3(1.0);
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    v_fog_color = fog_color.rgb;
}
